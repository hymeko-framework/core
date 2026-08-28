// APPROVED-CORE-EDIT: ir-stream-wire-codec (2026-08-28, Phase 10A brief).

//! The four-component snapshot version.
//!
//! # Why four counters
//!
//! They have different invalidation radii and different transport costs.
//! Collapsing them into one would force a full structural rebuild for a weight
//! tweak — measured in the Phase 9 baseline at 32.5 ms to rebuild a view,
//! 91.1 ms from IR and 724 ms for the explicit clique adjacency at large scale.
//!
//! # Why there is no total order
//!
//! The components are independent, so two versions can be mutually
//! incomparable: `(t=2, w=1)` and `(t=1, w=2)` neither dominates the other.
//! This type therefore implements `PartialEq` but **not** `Ord` or
//! `PartialOrd`, and offers [`SnapshotVersion::dominates`] instead. Deriving an
//! order would invite `a < b` to compile and mean something no consumer should
//! rely on.
//!
//! Note the deliberate asymmetry documented in
//! `docs/hypergraph-synchronization.md` §8: `topology` is dominant, and a
//! transaction that bumps it must carry new `weights` and `features` in the
//! same transaction, because those are indexed by structure. That rule belongs
//! to the transaction layer, which Phase 10A does not implement; this type only
//! provides the comparison it will need.

/// Bytes this occupies on the wire: four little-endian `u64`.
pub const VERSION_WIRE_BYTES: usize = 32;

/// What a snapshot is a version *of*, component by component.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SnapshotVersion {
    /// Incidence structure: which node belongs to which edge.
    pub topology: u64,
    /// Incidence weights. Structure unchanged.
    pub weights: u64,
    /// Node and edge feature tensors.
    pub features: u64,
    /// Model parameters, indexed by feature dimension rather than topology.
    pub params: u64,
}

impl SnapshotVersion {
    /// The version a consumer holds before it has received anything.
    pub const ZERO: Self = Self {
        topology: 0,
        weights: 0,
        features: 0,
        params: 0,
    };

    #[must_use]
    pub const fn new(topology: u64, weights: u64, features: u64, params: u64) -> Self {
        Self {
            topology,
            weights,
            features,
            params,
        }
    }

    /// Whether `self` is at least `other` in **every** component.
    ///
    /// This is the idempotence predicate: a transaction whose target version is
    /// dominated by what the consumer already holds is a no-op, which is what
    /// makes retransmission safe without a dedup table.
    #[must_use]
    pub const fn dominates(&self, other: &Self) -> bool {
        self.topology >= other.topology
            && self.weights >= other.weights
            && self.features >= other.features
            && self.params >= other.params
    }

    /// Whether the two are ordered at all. False for genuinely divergent
    /// versions, which is a producer bug rather than a transport failure.
    #[must_use]
    pub const fn is_comparable_to(&self, other: &Self) -> bool {
        self.dominates(other) || other.dominates(self)
    }

    /// Little-endian wire image. Explicit per component; never a struct cast.
    #[must_use]
    pub fn to_le_bytes(self) -> [u8; VERSION_WIRE_BYTES] {
        let mut out = [0u8; VERSION_WIRE_BYTES];
        out[0..8].copy_from_slice(&self.topology.to_le_bytes());
        out[8..16].copy_from_slice(&self.weights.to_le_bytes());
        out[16..24].copy_from_slice(&self.features.to_le_bytes());
        out[24..32].copy_from_slice(&self.params.to_le_bytes());
        out
    }

    /// Read from a 32-byte little-endian image.
    ///
    /// # Panics
    ///
    /// Never: the parameter is a fixed-size array, so the slicing below cannot
    /// be out of range.
    #[must_use]
    pub fn from_le_bytes(bytes: &[u8; VERSION_WIRE_BYTES]) -> Self {
        let rd = |o: usize| -> u64 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&bytes[o..o + 8]);
            u64::from_le_bytes(b)
        };
        Self {
            topology: rd(0),
            weights: rd(8),
            features: rd(16),
            params: rd(24),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_dominated_by_everything() {
        let v = SnapshotVersion::new(1, 2, 3, 4);
        assert!(v.dominates(&SnapshotVersion::ZERO));
        assert!(!SnapshotVersion::ZERO.dominates(&v));
        assert!(SnapshotVersion::ZERO.dominates(&SnapshotVersion::ZERO));
    }

    #[test]
    fn domination_requires_every_component() {
        let a = SnapshotVersion::new(2, 2, 2, 2);
        let b = SnapshotVersion::new(2, 2, 2, 3);
        assert!(!a.dominates(&b), "one lagging component must defeat it");
        assert!(b.dominates(&a));
    }

    #[test]
    fn divergent_versions_are_incomparable() {
        // The case that makes a total order wrong.
        let a = SnapshotVersion::new(2, 1, 0, 0);
        let b = SnapshotVersion::new(1, 2, 0, 0);
        assert!(!a.dominates(&b));
        assert!(!b.dominates(&a));
        assert!(!a.is_comparable_to(&b));
    }

    #[test]
    fn wire_image_round_trips_and_is_little_endian() {
        let v = SnapshotVersion::new(1, 0x0102_0304_0506_0708, u64::MAX, 0);
        let bytes = v.to_le_bytes();
        assert_eq!(bytes.len(), VERSION_WIRE_BYTES);
        // Least significant byte first, asserted on a known value.
        assert_eq!(
            &bytes[8..16],
            &[0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]
        );
        assert_eq!(SnapshotVersion::from_le_bytes(&bytes), v);
    }
}
