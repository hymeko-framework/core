//! Dense, view-local coordinates for a single [`HyperGraphView`].
//!
//! M2c.1 (`APPROVED-CORE-EDIT: dense-view-index-compute-degree`) separates two
//! concepts that previously shared `NodeId`/`EdgeId`:
//!
//! * **canonical entity identity** (`hymeko_core::ir::{NodeId, EdgeId}`) — owned
//!   by the `Ir`, later stable/generational, meaningful to endpoints, serialized;
//! * **dense derived-view coordinate** (this module) — a compact, contiguous
//!   `u32` valid *only* inside the one `HyperGraphView` that produced it, used in
//!   numerical hot loops and CSR construction.
//!
//! A dense index is analogous to a CSR row/column index. It is **not** an id:
//!
//! > A `DenseNodeIndex` / `DenseEdgeIndex` is meaningful only inside the
//! > `HyperGraphView` that produced it. It must **never** be persisted,
//! > transmitted, or used to address the canonical `Ir`. It carries no
//! > generation, no global identity, no transaction, no serialization, and no
//! > independent ownership.
//!
//! `#[repr(transparent)]` over a `u32`: exactly 4 bytes, same ABI as `u32`. Node
//! and edge coordinates are distinct types, so a node coordinate cannot be used
//! where an edge coordinate is expected (or vice versa) by accident.

/// A dense node coordinate inside one `HyperGraphView` (a `u32` index into the
/// view's node-space arrays). Not a canonical `NodeId`. See the module docs.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct DenseNodeIndex(u32);

/// A dense edge coordinate inside one `HyperGraphView` (a `u32` index into the
/// view's edge-space arrays). Not a canonical `EdgeId`. See the module docs.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct DenseEdgeIndex(u32);

/// A view coordinate did not fit the dense `u32` domain.
///
/// Carries bounded numeric metadata only (the offending count / value), never
/// arbitrary source text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DenseIndexError {
    /// A view had more nodes than a `u32` dense coordinate can address.
    NodeCountOverflow {
        /// The node count that did not fit `u32`.
        count: usize,
    },
    /// A view had more edges than a `u32` dense coordinate can address.
    EdgeCountOverflow {
        /// The edge count that did not fit `u32`.
        count: usize,
    },
}

impl core::fmt::Display for DenseIndexError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NodeCountOverflow { count } => {
                write!(f, "node count {count} exceeds the u32 dense-index domain")
            }
            Self::EdgeCountOverflow { count } => {
                write!(f, "edge count {count} exceeds the u32 dense-index domain")
            }
        }
    }
}

impl std::error::Error for DenseIndexError {}

macro_rules! dense_index_impl {
    ($ty:ident, $overflow:ident) => {
        impl $ty {
            /// Reconstruct a dense coordinate from a raw `u32`. Const, inline,
            /// allocation-free. The value is not validated against any view here
            /// — it is a view-local coordinate; range is checked at construction
            /// ([`Self::new_checked`]) and by the view builder.
            #[inline(always)]
            #[must_use]
            pub const fn from_u32(raw: u32) -> Self {
                Self(raw)
            }

            /// Checked construction from a `usize`. Fails (no silent truncation)
            /// when the value does not fit `u32`.
            ///
            /// # Errors
            /// [`DenseIndexError`] when `value > u32::MAX`.
            #[inline]
            pub fn new_checked(value: usize) -> Result<Self, DenseIndexError> {
                match u32::try_from(value) {
                    Ok(v) => Ok(Self(v)),
                    Err(_) => Err(DenseIndexError::$overflow { count: value }),
                }
            }

            /// The raw `u32` coordinate.
            #[inline(always)]
            #[must_use]
            pub const fn as_u32(self) -> u32 {
                self.0
            }

            /// The coordinate as a `usize` array index — inline, allocation-free.
            /// This is the sanctioned replacement for the former
            /// `canonical_id.raw()`-as-array-index pattern.
            #[inline(always)]
            #[must_use]
            pub const fn as_usize(self) -> usize {
                self.0 as usize
            }
        }
    };
}

dense_index_impl!(DenseNodeIndex, NodeCountOverflow);
dense_index_impl!(DenseEdgeIndex, EdgeCountOverflow);

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn dense_indices_are_exactly_four_bytes() {
        assert_eq!(size_of::<DenseNodeIndex>(), 4);
        assert_eq!(size_of::<DenseEdgeIndex>(), 4);
        // Recorded, not a target: plain u32 has no niche, so Option is 8 bytes.
        // The hot incidence arrays store the bare index (never an Option), so this
        // does not cost anything; a NonZeroU32 niche is deferred (no NONE here).
        assert_eq!(size_of::<Option<DenseNodeIndex>>(), 8);
    }

    #[test]
    fn checked_construction_rejects_overflow_without_truncation() {
        assert_eq!(DenseNodeIndex::new_checked(0).unwrap().as_usize(), 0);
        assert_eq!(
            DenseNodeIndex::new_checked(u32::MAX as usize)
                .unwrap()
                .as_u32(),
            u32::MAX
        );
        let over = (u32::MAX as usize) + 1;
        assert_eq!(
            DenseNodeIndex::new_checked(over),
            Err(DenseIndexError::NodeCountOverflow { count: over })
        );
        assert_eq!(
            DenseEdgeIndex::new_checked(over),
            Err(DenseIndexError::EdgeCountOverflow { count: over })
        );
    }

    #[test]
    fn zero_and_max_round_trip() {
        for v in [0u32, 1, 7, u32::MAX] {
            assert_eq!(DenseNodeIndex::from_u32(v).as_u32(), v);
            assert_eq!(DenseNodeIndex::from_u32(v).as_usize(), v as usize);
        }
    }

    #[test]
    fn node_and_edge_coordinates_are_distinct_types() {
        // These are separate types; the following would not compile (asserted by
        // construction — there is no `From`/`Into` between them and no shared
        // supertype). This test documents the intent and exercises both.
        let n = DenseNodeIndex::from_u32(3);
        let e = DenseEdgeIndex::from_u32(3);
        assert_eq!(n.as_u32(), e.as_u32()); // same raw value...
        // ...but n and e cannot be compared or swapped: distinct types.
    }
}
