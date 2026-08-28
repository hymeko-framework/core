// APPROVED-CORE-EDIT: ir-stream-wire-codec (2026-08-28, Phase 10A brief).

//! Resource ceilings, checked before anything is allocated.
//!
//! # The attack this exists to stop
//!
//! A 144-byte header can declare a payload of `u64::MAX` and a section count of
//! `u32::MAX`. A validator that allocates from either number before checking it
//! turns a tiny hostile message into an out-of-memory abort. Every limit below
//! is therefore consulted **before** the allocation it bounds, and
//! `validator_allocates_nothing_for_a_hostile_section_count` proves it for the
//! one allocation this crate's validator performs.
//!
//! # Why these defaults
//!
//! They are sized against the workloads this repository actually measures
//! rather than picked round. `hymeko_hnn::workloads`'s `large` spec has
//! 1 642 260 incidences; a full snapshot of it carries roughly
//! `4 * nnz` bytes of column indices plus `4 * nnz` of weights, so ~13 MiB.
//! The 256 MiB default leaves an order of magnitude of headroom for feature and
//! parameter sections without permitting an unbounded request.
//!
//! Defaults are a policy, not a constant of the format. A deployment that knows
//! its own sizes should set them tighter.

/// Ceilings applied while decoding one message.
///
/// # Invariants
///
/// Every field is a hard upper bound: exceeding one is a rejection, never a
/// truncation. There is no "clamp and continue" path, because silently
/// processing less than the producer sent is how two sides diverge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeLimits {
    /// Total bytes of one message, envelope included.
    pub max_message_bytes: u64,
    /// Payload bytes, excluding the envelope.
    pub max_payload_bytes: u64,
    /// Entries in the section table. Bounds the only allocation the validator
    /// performs, and is checked before that allocation.
    pub max_section_count: u32,
    /// Bytes in any single section body.
    pub max_section_bytes: u64,
    /// Elements declared by any single section. Bounds what a consumer would
    /// allocate when it later materialises a section into a typed collection —
    /// the validator itself never does.
    pub max_elements_per_section: u64,
}

impl DecodeLimits {
    /// 256 MiB messages, 1024 sections, 128 MiB per section, 64 M elements.
    ///
    /// See the module documentation for how these relate to the measured
    /// workloads.
    pub const DEFAULT: Self = Self {
        max_message_bytes: 256 * 1024 * 1024,
        max_payload_bytes: 256 * 1024 * 1024,
        max_section_count: 1024,
        max_section_bytes: 128 * 1024 * 1024,
        max_elements_per_section: 64 * 1024 * 1024,
    };

    /// Deliberately tiny limits, for tests that must exercise the rejection
    /// paths without building large buffers.
    #[must_use]
    pub const fn strict() -> Self {
        Self {
            max_message_bytes: 64 * 1024,
            max_payload_bytes: 64 * 1024,
            max_section_count: 16,
            max_section_bytes: 32 * 1024,
            max_elements_per_section: 8192,
        }
    }
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_internally_consistent() {
        let d = DecodeLimits::DEFAULT;
        assert!(
            d.max_payload_bytes <= d.max_message_bytes,
            "a payload cannot be allowed to exceed the whole message"
        );
        assert!(
            d.max_section_bytes <= d.max_payload_bytes,
            "one section cannot be allowed to exceed the payload"
        );
        assert!(d.max_section_count > 0);
        assert!(d.max_elements_per_section > 0);
    }

    #[test]
    fn strict_limits_are_smaller_in_every_dimension() {
        let d = DecodeLimits::DEFAULT;
        let s = DecodeLimits::strict();
        assert!(s.max_message_bytes < d.max_message_bytes);
        assert!(s.max_payload_bytes < d.max_payload_bytes);
        assert!(s.max_section_count < d.max_section_count);
        assert!(s.max_section_bytes < d.max_section_bytes);
        assert!(s.max_elements_per_section < d.max_elements_per_section);
    }
}
