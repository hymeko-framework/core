// APPROVED-CORE-EDIT: ir-stream-wire-codec (2026-08-28, Phase 10A brief).

//! # QUARANTINED — this is not a wire protocol.
//!
//! **Do not use these types to exchange data between processes, hosts or
//! architectures.** Use [`crate::ir_stream`] instead.
//!
//! This module is Phase-1 shared-memory scaffolding. It was audited in Phase 9
//! (`reports/2026-08-28-phase9-kernel-infrastructure.md`,
//! `docs/hypergraph-synchronization.md` §1) and found unsuitable as a portable
//! wire format on six independent counts, each **measured** rather than
//! inferred:
//!
//! 1. **Fixed 4 MiB payload.** [`HypergraphWeights`] is 4 194 320 bytes and is
//!    carried in full regardless of content: a one-weight update ships 4 useful
//!    bytes inside 4 MiB. It is simultaneously *too small* — [`MAX_NNZ`] is
//!    1 048 576 while this repository's own `large` benchmark workload has
//!    1 642 260 incidences, and nothing here detects the overrun.
//! 2. **`usize` fields.** A `#[repr(C)]` struct containing `usize` has no
//!    stable layout across targets: 8 bytes here, 4 on a 32-bit consumer. The
//!    same bytes decode differently on each side.
//! 3. **Native endianness.** Nothing declares a byte order, so two
//!    little-endian hosts agree by accident and a big-endian endpoint does not.
//! 4. **`#[repr(u32)]` enum as received data.** [`ExpansionKind`] defines only
//!    1 and 2; materialising any other bit pattern into that type is undefined
//!    behaviour, and nothing here validates before doing so.
//! 5. **Unchecked size arithmetic.** The original `contiguous_offsets` computed
//!    `nnz * size_of::<i64>()` unchecked on a wire-supplied length. At
//!    `nnz = u64::MAX` it produced *descending* offsets (72, 64, 56, 48) and a
//!    total of **44 bytes — smaller than its own 72-byte header**, so a
//!    consumer guarding on `len >= total_bytes` passed and then indexed out of
//!    bounds. That behaviour is **removed**, not preserved; see below.
//! 6. **Unpopulated digest.** `etag` is the right size for BLAKE3 and was never
//!    written — a field that looked like integrity checking and provided none.
//!
//! # Why it still exists
//!
//! `hymeko_core` builds as `cdylib` and `rlib`. The module has no consumer
//! *inside* this repository, but downstream callers cannot be enumerated from
//! here, so deletion was not assumed safe. The path and the `ipc` feature gate
//! are kept so no import breaks; everything else is marked deprecated and the
//! dangerous arithmetic is gone.
//!
//! # The one deliberate breaking change
//!
//! `contiguous_offsets` and `calculate_required_bytes` now return `Option` and
//! compute with checked arithmetic. Preserving their previous behaviour was
//! explicitly not authorised, and it is precisely the defect described in
//! point 5. A caller that ignored the possibility of overflow was already
//! wrong; it now has to say so.

// Every item in this module is deprecated, and the `ZeroCopySend` derive
// expands to code that names the type it is applied to. Scoping the allow to
// the module is the smallest region that works: the alternative is one allow
// per item plus one inside a macro expansion this crate does not control.
// Nothing outside this quarantined module is covered.
#![allow(deprecated)]

use iceoryx2::prelude::*;
use std::mem::size_of;

/// Maximum incidences representable by [`HypergraphWeights`].
///
/// Smaller than this repository's own `large` workload (1 642 260 incidences).
/// That is one of the reasons this module is quarantined.
#[deprecated(
    since = "0.1.0",
    note = "not a wire protocol; use hymeko::ir_stream, which is length-delimited \
            and has no fixed capacity"
)]
pub const MAX_NNZ: usize = 1_048_576;

/// Compile-time record of quarantine reason 1: the fixed capacity is smaller
/// than this repository's own `large` benchmark workload (1 642 260
/// incidences in `hymeko_hnn::workloads`). Asserted at compile time rather than
/// in a test, because it is a property of the constant, not of any execution.
const _: () = assert!(
    1_642_260usize > MAX_NNZ,
    "if this fails, MAX_NNZ grew and the quarantine note needs revisiting"
);

/// A fixed-capacity weight block for same-host shared memory.
///
/// 4 194 320 bytes regardless of how many weights are live. Retained only for
/// backward compatibility; see the module documentation.
#[deprecated(
    since = "0.1.0",
    note = "fixed 4 MiB payload with a `usize` field and no bounds relationship \
            between `nnz` and `values`; use hymeko::ir_stream"
)]
#[repr(C)]
#[derive(Debug, ZeroCopySend)]
pub struct HypergraphWeights {
    pub version: u64,
    /// **Not** validated against [`MAX_NNZ`] anywhere in this module. A reader
    /// that indexes `values[..nnz]` without checking will read out of bounds.
    pub nnz: usize,
    pub values: [f32; MAX_NNZ],
}

/// Which expansion an [`ExpansionHeader`] describes.
///
/// # Safety note
///
/// This is a `#[repr(u32)]` enum with two valid discriminants. It must **never**
/// be produced by reinterpreting received bytes: any other value is undefined
/// behaviour. [`crate::ir_stream`] carries discriminants as plain integers and
/// converts them through a checked `TryFrom` for exactly this reason.
#[deprecated(
    since = "0.1.0",
    note = "a Rust enum is unsound as a received wire field; use \
            hymeko::ir_stream::SectionKind, which validates before converting"
)]
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExpansionKind {
    Star3D = 1,
    Clique2D = 2,
}

/// Header for a contiguous expansion block in shared memory.
#[deprecated(
    since = "0.1.0",
    note = "native-endian repr(C) with an enum field and an unpopulated digest; \
            use hymeko::ir_stream::ValidatedEnvelope"
)]
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ExpansionHeader {
    pub version: u32,
    pub kind: ExpansionKind,
    pub nnz: u64,
    /// Never written by this module. Not an integrity check.
    pub etag: [u8; 32],
    pub dim_k: u64,
    pub dim_i: u64,
    pub dim_j: u64,
}

impl ExpansionHeader {
    pub const VERSION: u32 = 1;

    #[must_use]
    pub fn new(kind: ExpansionKind, nnz: usize, dim_k: usize, dim_i: usize, dim_j: usize) -> Self {
        Self {
            version: Self::VERSION,
            kind,
            nnz: nnz as u64,
            etag: [0u8; 32],
            dim_k: dim_k as u64,
            dim_i: dim_i as u64,
            dim_j: dim_j as u64,
        }
    }

    /// Byte offsets of the contiguous `(k, i, j, val)` buffers following the
    /// header, or `None` if the arithmetic would overflow.
    ///
    /// # Breaking change
    ///
    /// This previously returned `ExpansionOffsets` and computed with unchecked
    /// arithmetic. On a hostile or corrupt `nnz` it produced descending offsets
    /// and a total smaller than the header itself, which a length guard would
    /// accept. Returning `Option` is the minimum change that makes the failure
    /// expressible.
    ///
    /// # Postconditions
    ///
    /// When `Some`, the offsets are strictly increasing and `total_bytes` is at
    /// least `header_bytes`. Both are asserted by this module's tests.
    #[must_use]
    pub fn contiguous_offsets(&self) -> Option<ExpansionOffsets> {
        let header_bytes = size_of::<ExpansionHeader>();
        let nnz = usize::try_from(self.nnz).ok()?;

        let index_span = nnz.checked_mul(size_of::<i64>())?;
        let value_span = nnz.checked_mul(size_of::<f32>())?;

        let k_offset = header_bytes;
        let i_offset = k_offset.checked_add(index_span)?;
        let j_offset = i_offset.checked_add(index_span)?;
        let values_offset = j_offset.checked_add(index_span)?;
        let total_bytes = values_offset.checked_add(value_span)?;

        Some(ExpansionOffsets {
            header_bytes,
            k_offset,
            i_offset,
            j_offset,
            values_offset,
            total_bytes,
        })
    }
}

/// Byte offsets within a contiguous expansion block.
#[deprecated(
    since = "0.1.0",
    note = "`usize` fields have no stable layout across targets; use \
            hymeko::ir_stream::SectionEntry"
)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ExpansionOffsets {
    pub header_bytes: usize,
    pub k_offset: usize,
    pub i_offset: usize,
    pub j_offset: usize,
    pub values_offset: usize,
    pub total_bytes: usize,
}

/// Bytes required for a contiguous expansion block, or `None` on overflow.
///
/// Layout: `[ExpansionHeader][k: i64 × nnz][i: i64 × nnz][j: i64 × nnz][val: f32 × nnz]`.
///
/// # Breaking change
///
/// Previously returned `usize` and wrapped on overflow. See
/// [`ExpansionHeader::contiguous_offsets`].
#[must_use]
pub fn calculate_required_bytes(nnz: usize) -> Option<usize> {
    let index_span = nnz.checked_mul(size_of::<i64>())?;
    let value_span = nnz.checked_mul(size_of::<f32>())?;
    size_of::<ExpansionHeader>()
        .checked_add(index_span)?
        .checked_add(index_span)?
        .checked_add(index_span)?
        .checked_add(value_span)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_sizes_still_compute() {
        let h = ExpansionHeader::new(ExpansionKind::Star3D, 10, 1, 2, 3);
        let o = h.contiguous_offsets().expect("10 nnz is representable");
        assert_eq!(o.header_bytes, size_of::<ExpansionHeader>());
        assert_eq!(o.k_offset, o.header_bytes);
        assert_eq!(o.i_offset, o.k_offset + 80);
        assert_eq!(o.j_offset, o.i_offset + 80);
        assert_eq!(o.values_offset, o.j_offset + 80);
        assert_eq!(o.total_bytes, o.values_offset + 40);
        assert_eq!(calculate_required_bytes(10), Some(o.total_bytes));
    }

    #[test]
    fn offsets_are_monotone_whenever_they_are_produced() {
        // The property the old implementation violated. Anything that returns
        // Some must be internally consistent.
        for nnz in [0usize, 1, 2, 1024, 1_642_260] {
            let h = ExpansionHeader::new(ExpansionKind::Clique2D, nnz, 0, 0, 0);
            let o = h.contiguous_offsets().expect("representable");
            assert!(o.k_offset <= o.i_offset, "nnz={nnz}");
            assert!(o.i_offset <= o.j_offset, "nnz={nnz}");
            assert!(o.j_offset <= o.values_offset, "nnz={nnz}");
            assert!(o.values_offset <= o.total_bytes, "nnz={nnz}");
            assert!(o.total_bytes >= o.header_bytes, "nnz={nnz}");
        }
    }

    #[test]
    fn the_phase9_hostile_values_now_report_instead_of_wrapping() {
        // Measured in the Phase 9 audit: these produced descending offsets and
        // a 44-byte total for a 72-byte header.
        for nnz in [1u64 << 61, (1u64 << 61) - 1, u64::MAX] {
            let mut h = ExpansionHeader::new(ExpansionKind::Star3D, 0, 0, 0, 0);
            h.nnz = nnz;
            assert_eq!(
                h.contiguous_offsets(),
                None,
                "nnz={nnz} must report overflow rather than wrap"
            );
        }
        assert_eq!(calculate_required_bytes(usize::MAX), None);
        assert_eq!(calculate_required_bytes(usize::MAX / 4), None);
    }
}
