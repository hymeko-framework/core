// APPROVED-CORE-EDIT: ir-stream-wire-codec (2026-08-28, Phase 10A brief).

//! The payload section table, and the bounds rules that make it safe.
//!
//! # What this replaces
//!
//! The audited `shared_state::ExpansionHeader::contiguous_offsets` computed
//! section offsets with unchecked `nnz * size_of::<i64>()` on a length taken
//! from the wire. At `nnz = u64::MAX` it produced **descending** offsets
//! (72, 64, 56, 48) and a total of **44 bytes — smaller than its own 72-byte
//! header**, so a consumer guarding on `len >= total_bytes` passed and then
//! indexed out of bounds.
//!
//! Three independent rules here each reject that input on their own:
//! checked arithmetic on every offset and length; strictly increasing,
//! non-overlapping offsets; and `element_count × element_size == byte_len`.
//! `hostile_lengths_from_the_phase9_audit_are_rejected` replays the measured
//! values.
//!
//! # Layout
//!
//! The payload of a section-bearing message is:
//!
//! ```text
//! [ u32 section_count ][ u32 reserved ][ entry × count ][ bodies ... ]
//! ```
//!
//! Each entry is 32 bytes:
//!
//! | off | field | type |
//! |---:|---|---|
//! | 0 | `kind` | `u16` |
//! | 2 | `element_kind` | `u16` |
//! | 4 | `reserved` | `u32` (must be 0) |
//! | 8 | `element_count` | `u64` |
//! | 16 | `byte_offset` | `u64` (payload-relative) |
//! | 24 | `byte_len` | `u64` |
//!
//! # Alignment policy
//!
//! Section bodies must begin on a **4-byte** boundary, matching the widest
//! element type carried (`u32` / `f32`). This lets a consumer copy a body into
//! an aligned buffer without a shifting copy, and is cheap to check. Gaps of up
//! to three bytes between sections are therefore permitted purely as alignment
//! padding; they are covered by the payload digest like any other byte.
//!
//! # Iteration order
//!
//! Sections iterate in table order, which validation has proved equals
//! increasing byte order. The order is therefore deterministic and does not
//! depend on the kinds present.

use super::error::WireError;
use super::limits::DecodeLimits;

/// Bytes per section-table entry.
pub const ENTRY_BYTES: usize = 32;
/// Bytes of the table header preceding the entries: count plus reserved.
pub const TABLE_PREFIX_BYTES: usize = 8;
/// Required alignment of a section body within the payload.
pub const SECTION_ALIGN: u64 = 4;

/// Section kinds below this boundary must be understood; at or above it they
/// may be skipped.
///
/// This is the schema-evolution seam: a producer adding an *optional* section
/// numbers it at or above the boundary and old consumers ignore it. A section
/// whose absence would change the meaning of the message is numbered below, and
/// an old consumer refuses rather than silently computing on partial data.
pub const CRITICAL_SECTION_MAX: u16 = 1000;

/// What a section carries.
///
/// A safe Rust enum reached only through [`SectionKind::try_from_wire`]; the
/// wire carries `u16`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SectionKind {
    // --- critical: a consumer that does not understand these must refuse ---
    /// `u32 × (num_edges + 1)`, edge-major CSR offsets.
    EdgeOffsets,
    /// `u32 × nnz`, edge-major incidence column indices.
    EdgeNodes,
    /// `f32 × nnz`, incidence weights in edge-major order.
    IncidenceWeights,
    /// `f32 × (num_nodes × feature_dim)`.
    NodeFeatures,
    /// `f32`, layer-defined model parameters.
    ModelParams,
    /// `i8 × nnz`, optional sign pattern.
    SignPattern,
    /// `u32` indices then `f32` values — sparse weight patch, edge-major.
    WeightPatchSparse,
    /// Contiguous weight range.
    WeightPatchRange,
    /// Row ids then rows of features.
    FeatureRowPatch,
    /// Contiguous parameter range.
    ParamPatchRange,
    /// A section this build does not know, at or above
    /// [`CRITICAL_SECTION_MAX`], preserved so a consumer can forward it.
    Unknown(u16),
}

impl SectionKind {
    /// Checked conversion from the wire representation.
    ///
    /// # Errors
    ///
    /// [`WireError::UnknownMandatorySection`] when `raw` is below
    /// [`CRITICAL_SECTION_MAX`] and unrecognised. Values at or above the
    /// boundary become [`SectionKind::Unknown`] and are skippable.
    pub const fn try_from_wire(raw: u16, index: usize) -> Result<Self, WireError> {
        Ok(match raw {
            1 => Self::EdgeOffsets,
            2 => Self::EdgeNodes,
            3 => Self::IncidenceWeights,
            4 => Self::NodeFeatures,
            5 => Self::ModelParams,
            6 => Self::SignPattern,
            7 => Self::WeightPatchSparse,
            8 => Self::WeightPatchRange,
            9 => Self::FeatureRowPatch,
            10 => Self::ParamPatchRange,
            other if other >= CRITICAL_SECTION_MAX => Self::Unknown(other),
            other => {
                return Err(WireError::UnknownMandatorySection { index, kind: other });
            }
        })
    }

    /// The wire representation.
    #[must_use]
    pub const fn to_wire(self) -> u16 {
        match self {
            Self::EdgeOffsets => 1,
            Self::EdgeNodes => 2,
            Self::IncidenceWeights => 3,
            Self::NodeFeatures => 4,
            Self::ModelParams => 5,
            Self::SignPattern => 6,
            Self::WeightPatchSparse => 7,
            Self::WeightPatchRange => 8,
            Self::FeatureRowPatch => 9,
            Self::ParamPatchRange => 10,
            Self::Unknown(raw) => raw,
        }
    }

    /// Whether at most one of these may appear in a message.
    ///
    /// The structural sections describe one thing each, so a second occurrence
    /// is ambiguous rather than additive. Patch sections are explicitly
    /// repeatable: several disjoint patches in one transaction are normal.
    #[must_use]
    pub const fn is_singleton(self) -> bool {
        matches!(
            self,
            Self::EdgeOffsets
                | Self::EdgeNodes
                | Self::IncidenceWeights
                | Self::NodeFeatures
                | Self::ModelParams
                | Self::SignPattern
        )
    }

    /// Whether a consumer may ignore this section.
    #[must_use]
    pub const fn is_skippable(self) -> bool {
        matches!(self, Self::Unknown(_))
    }
}

/// Element type of a section body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ElementKind {
    U32,
    F32,
    I8,
    /// Opaque bytes, for sections whose interpretation is not this layer's.
    Raw8,
}

impl ElementKind {
    /// Checked conversion from the wire representation.
    ///
    /// # Errors
    ///
    /// [`WireError::UnknownElementKind`] for an undefined value. Element kinds
    /// are never skippable: without one, `byte_len` cannot be checked against
    /// `element_count`, which is one of the three overflow guards.
    pub const fn try_from_wire(raw: u16, index: usize) -> Result<Self, WireError> {
        Ok(match raw {
            1 => Self::U32,
            2 => Self::F32,
            3 => Self::I8,
            4 => Self::Raw8,
            kind => return Err(WireError::UnknownElementKind { index, kind }),
        })
    }

    /// The wire representation.
    #[must_use]
    pub const fn to_wire(self) -> u16 {
        match self {
            Self::U32 => 1,
            Self::F32 => 2,
            Self::I8 => 3,
            Self::Raw8 => 4,
        }
    }

    /// Bytes per element.
    #[must_use]
    pub const fn size(self) -> u64 {
        match self {
            Self::U32 | Self::F32 => 4,
            Self::I8 | Self::Raw8 => 1,
        }
    }
}

/// One validated section-table entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionEntry {
    pub kind: SectionKind,
    pub element_kind: ElementKind,
    pub element_count: u64,
    pub byte_offset: u64,
    pub byte_len: u64,
}

impl SectionEntry {
    /// Encode to the 32-byte wire image. Present for producers and tests.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; ENTRY_BYTES] {
        let mut b = [0u8; ENTRY_BYTES];
        b[0..2].copy_from_slice(&self.kind.to_wire().to_le_bytes());
        b[2..4].copy_from_slice(&self.element_kind.to_wire().to_le_bytes());
        // reserved stays zero
        b[8..16].copy_from_slice(&self.element_count.to_le_bytes());
        b[16..24].copy_from_slice(&self.byte_offset.to_le_bytes());
        b[24..32].copy_from_slice(&self.byte_len.to_le_bytes());
        b
    }
}

/// A section body whose bounds have been proved to lie inside the payload.
///
/// # Why the body is only `&[u8]`
///
/// Returning `&[u32]` or `&[f32]` would require forming a reference into wire
/// bytes at an arbitrary address, which is unsound for any type with alignment
/// greater than one. The typed accessors below therefore **copy**. A consumer
/// that wants zero-copy must arrange its own aligned staging buffer; this layer
/// will not hand out an unaligned reference to make that look free.
#[derive(Clone, Copy, Debug)]
pub struct ValidatedSection<'a> {
    entry: SectionEntry,
    body: &'a [u8],
}

impl<'a> ValidatedSection<'a> {
    pub(crate) const fn new(entry: SectionEntry, body: &'a [u8]) -> Self {
        Self { entry, body }
    }

    #[must_use]
    pub const fn kind(&self) -> SectionKind {
        self.entry.kind
    }

    #[must_use]
    pub const fn element_kind(&self) -> ElementKind {
        self.entry.element_kind
    }

    #[must_use]
    pub const fn element_count(&self) -> u64 {
        self.entry.element_count
    }

    #[must_use]
    pub const fn entry(&self) -> SectionEntry {
        self.entry
    }

    /// The raw body. Length is guaranteed to equal `byte_len`.
    #[must_use]
    pub const fn bytes(&self) -> &'a [u8] {
        self.body
    }

    /// Copy the body as `u32`s.
    ///
    /// Copies rather than reinterprets; see the type documentation.
    ///
    /// # Errors
    ///
    /// [`WireError::SectionLengthMismatch`] if the declared element kind is not
    /// [`ElementKind::U32`].
    pub fn to_u32_vec(&self) -> Result<Vec<u32>, WireError> {
        self.typed_copy(ElementKind::U32, |c| {
            u32::from_le_bytes([c[0], c[1], c[2], c[3]])
        })
    }

    /// Copy the body as `f32`s. Copies rather than reinterprets.
    ///
    /// # Errors
    ///
    /// As [`Self::to_u32_vec`].
    pub fn to_f32_vec(&self) -> Result<Vec<f32>, WireError> {
        self.typed_copy(ElementKind::F32, |c| {
            f32::from_le_bytes([c[0], c[1], c[2], c[3]])
        })
    }

    fn typed_copy<T>(
        &self,
        want: ElementKind,
        convert: impl Fn(&[u8]) -> T,
    ) -> Result<Vec<T>, WireError> {
        if self.entry.element_kind != want {
            return Err(WireError::SectionLengthMismatch {
                index: 0,
                expected: want.size(),
                got: self.entry.element_kind.size(),
            });
        }
        let step = want.size() as usize;
        // `byte_len == element_count * size` was proved during validation, so
        // this division is exact and the collect cannot be short.
        Ok(self.body.chunks_exact(step).map(convert).collect())
    }
}

/// Decode and fully validate a section table plus its bodies.
///
/// `payload` is the message payload, already length-checked and digest-checked.
///
/// # Errors
///
/// Any [`WireError`] in the section family. Every rule is checked for every
/// entry; the first violation is returned.
pub(crate) fn validate_sections<'a>(
    payload: &'a [u8],
    limits: &DecodeLimits,
) -> Result<Vec<ValidatedSection<'a>>, WireError> {
    let payload_len = payload.len() as u64;

    if payload.len() < TABLE_PREFIX_BYTES {
        return Err(WireError::SectionTableOutOfBounds {
            need: TABLE_PREFIX_BYTES as u64,
            payload_len,
        });
    }

    let count = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let reserved = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    if reserved != 0 {
        return Err(WireError::ReservedNotZero {
            field: "section table reserved",
        });
    }

    // CHECKED BEFORE ANY ALLOCATION. This is the guard that stops a 200-byte
    // message from asking for a 4-billion-entry Vec.
    if count > limits.max_section_count {
        return Err(WireError::TooManySections {
            got: count,
            max: limits.max_section_count,
        });
    }

    let table_bytes = u64::from(count)
        .checked_mul(ENTRY_BYTES as u64)
        .and_then(|b| b.checked_add(TABLE_PREFIX_BYTES as u64))
        .ok_or(WireError::LengthOverflow {
            what: "section table size",
        })?;
    if table_bytes > payload_len {
        return Err(WireError::SectionTableOutOfBounds {
            need: table_bytes,
            payload_len,
        });
    }

    // Bounded by the limit checked above.
    let mut out: Vec<ValidatedSection<'a>> = Vec::with_capacity(count as usize);
    let mut singletons_seen: Vec<u16> = Vec::new();
    let mut previous_end: u64 = table_bytes;

    for index in 0..count as usize {
        let base = TABLE_PREFIX_BYTES + index * ENTRY_BYTES;
        let e = &payload[base..base + ENTRY_BYTES];

        let entry_reserved = u32::from_le_bytes([e[4], e[5], e[6], e[7]]);
        if entry_reserved != 0 {
            return Err(WireError::ReservedNotZero {
                field: "section entry reserved",
            });
        }

        let kind = SectionKind::try_from_wire(u16::from_le_bytes([e[0], e[1]]), index)?;
        let element_kind = ElementKind::try_from_wire(u16::from_le_bytes([e[2], e[3]]), index)?;
        let element_count = rd_u64(e, 8);
        let byte_offset = rd_u64(e, 16);
        let byte_len = rd_u64(e, 24);

        if element_count > limits.max_elements_per_section {
            return Err(WireError::TooManyElements {
                index,
                got: element_count,
                max: limits.max_elements_per_section,
            });
        }
        if byte_len > limits.max_section_bytes {
            return Err(WireError::SectionTooLarge {
                index,
                got: byte_len,
                max: limits.max_section_bytes,
            });
        }

        // Guard 1: declared length must equal the element arithmetic, computed
        // with a checked multiply.
        let expected =
            element_count
                .checked_mul(element_kind.size())
                .ok_or(WireError::LengthOverflow {
                    what: "element_count * element_size",
                })?;
        if expected != byte_len {
            return Err(WireError::SectionLengthMismatch {
                index,
                expected,
                got: byte_len,
            });
        }

        // Guard 2: the body may not start inside the table.
        if byte_offset < table_bytes {
            return Err(WireError::SectionOverlapsTable {
                index,
                offset: byte_offset,
                table_end: table_bytes,
            });
        }

        // Guard 3: strictly increasing and non-overlapping. This alone rejects
        // the descending offsets the audited code produced.
        if byte_offset < previous_end {
            return Err(WireError::SectionNotIncreasing {
                index,
                offset: byte_offset,
                previous_end,
            });
        }

        if !byte_offset.is_multiple_of(SECTION_ALIGN) {
            return Err(WireError::SectionMisaligned {
                index,
                offset: byte_offset,
                align: SECTION_ALIGN,
            });
        }

        // Guard 4: checked end, then bounds against the real payload length.
        let end = byte_offset
            .checked_add(byte_len)
            .ok_or(WireError::LengthOverflow {
                what: "byte_offset + byte_len",
            })?;
        if end > payload_len {
            return Err(WireError::SectionOutOfBounds {
                index,
                end,
                payload_len,
            });
        }

        if kind.is_singleton() {
            let raw = kind.to_wire();
            if singletons_seen.contains(&raw) {
                return Err(WireError::DuplicateSection { index, kind: raw });
            }
            singletons_seen.push(raw);
        }

        // Checked narrowing; on a 32-bit consumer a large-but-legal u64 offset
        // is reported rather than truncated.
        let start_us = usize::try_from(byte_offset).map_err(|_| WireError::NotAddressable {
            what: "byte_offset",
            got: byte_offset,
        })?;
        let end_us = usize::try_from(end).map_err(|_| WireError::NotAddressable {
            what: "byte_offset + byte_len",
            got: end,
        })?;

        out.push(ValidatedSection::new(
            SectionEntry {
                kind,
                element_kind,
                element_count,
                byte_offset,
                byte_len,
            },
            &payload[start_us..end_us],
        ));
        previous_end = end;
    }

    Ok(out)
}

fn rd_u64(b: &[u8], off: usize) -> u64 {
    let mut t = [0u8; 8];
    t.copy_from_slice(&b[off..off + 8]);
    u64::from_le_bytes(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a payload with the given entries and a body region of `body_len`.
    fn payload_with(entries: &[SectionEntry], body_len: usize) -> Vec<u8> {
        let count = u32::try_from(entries.len()).expect("test entry count");
        let mut p = Vec::new();
        p.extend_from_slice(&count.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes());
        for e in entries {
            p.extend_from_slice(&e.to_bytes());
        }
        p.resize(
            TABLE_PREFIX_BYTES + entries.len() * ENTRY_BYTES + body_len,
            0,
        );
        p
    }

    fn entry(kind: SectionKind, ek: ElementKind, count: u64, offset: u64) -> SectionEntry {
        SectionEntry {
            kind,
            element_kind: ek,
            element_count: count,
            byte_offset: offset,
            byte_len: count * ek.size(),
        }
    }

    #[test]
    fn a_well_formed_table_validates_and_iterates_in_offset_order() {
        let table_end = (TABLE_PREFIX_BYTES + 2 * ENTRY_BYTES) as u64;
        let a = entry(SectionKind::EdgeOffsets, ElementKind::U32, 4, table_end);
        let b = entry(SectionKind::EdgeNodes, ElementKind::U32, 8, table_end + 16);
        let p = payload_with(&[a, b], 16 + 32);
        let secs = validate_sections(&p, &DecodeLimits::strict()).expect("valid table");
        assert_eq!(secs.len(), 2);
        assert_eq!(secs[0].kind(), SectionKind::EdgeOffsets);
        assert_eq!(secs[1].kind(), SectionKind::EdgeNodes);
        assert!(
            secs[0].entry().byte_offset < secs[1].entry().byte_offset,
            "iteration order must equal increasing byte order"
        );
        assert_eq!(secs[0].bytes().len(), 16);
        assert_eq!(secs[1].bytes().len(), 32);
    }

    #[test]
    fn hostile_lengths_from_the_phase9_audit_are_rejected() {
        // The measured values that made the old code produce descending offsets
        // and a 44-byte total for a 72-byte header.
        let table_end = (TABLE_PREFIX_BYTES + ENTRY_BYTES) as u64;
        for hostile in [1u64 << 61, (1u64 << 61) - 1, u64::MAX] {
            for e in [
                // as element_count
                SectionEntry {
                    kind: SectionKind::EdgeNodes,
                    element_kind: ElementKind::U32,
                    element_count: hostile,
                    byte_offset: table_end,
                    byte_len: 4,
                },
                // as byte_len
                SectionEntry {
                    kind: SectionKind::EdgeNodes,
                    element_kind: ElementKind::U32,
                    element_count: 1,
                    byte_offset: table_end,
                    byte_len: hostile,
                },
                // as byte_offset
                SectionEntry {
                    kind: SectionKind::EdgeNodes,
                    element_kind: ElementKind::U32,
                    element_count: 1,
                    byte_offset: hostile,
                    byte_len: 4,
                },
            ] {
                let p = payload_with(&[e], 64);
                let err = validate_sections(&p, &DecodeLimits::DEFAULT)
                    .expect_err("hostile length must be refused, never wrapped");
                // Any of the guards may fire first; what matters is that one did
                // and that it is a length-family rejection.
                assert!(
                    matches!(
                        err,
                        WireError::LengthOverflow { .. }
                            | WireError::SectionLengthMismatch { .. }
                            | WireError::SectionOutOfBounds { .. }
                            | WireError::SectionTooLarge { .. }
                            | WireError::TooManyElements { .. }
                            | WireError::SectionMisaligned { .. }
                    ),
                    "hostile {hostile} gave {err:?}"
                );
            }
        }
    }

    #[test]
    fn descending_offsets_are_rejected() {
        let table_end = (TABLE_PREFIX_BYTES + 2 * ENTRY_BYTES) as u64;
        let a = entry(
            SectionKind::EdgeOffsets,
            ElementKind::U32,
            4,
            table_end + 16,
        );
        let b = entry(SectionKind::EdgeNodes, ElementKind::U32, 4, table_end);
        let p = payload_with(&[a, b], 64);
        assert!(matches!(
            validate_sections(&p, &DecodeLimits::strict()),
            Err(WireError::SectionNotIncreasing { index: 1, .. })
        ));
    }

    #[test]
    fn overlapping_sections_are_rejected() {
        let table_end = (TABLE_PREFIX_BYTES + 2 * ENTRY_BYTES) as u64;
        let a = entry(SectionKind::EdgeOffsets, ElementKind::U32, 4, table_end);
        // starts 4 bytes into a's 16-byte body
        let b = entry(SectionKind::EdgeNodes, ElementKind::U32, 2, table_end + 4);
        let p = payload_with(&[a, b], 64);
        assert!(matches!(
            validate_sections(&p, &DecodeLimits::strict()),
            Err(WireError::SectionNotIncreasing { .. })
        ));
    }

    #[test]
    fn a_body_inside_the_table_is_rejected() {
        let a = entry(SectionKind::EdgeOffsets, ElementKind::U32, 1, 8);
        let p = payload_with(&[a], 64);
        assert!(matches!(
            validate_sections(&p, &DecodeLimits::strict()),
            Err(WireError::SectionOverlapsTable { .. })
        ));
    }

    #[test]
    fn misaligned_bodies_are_rejected() {
        let table_end = (TABLE_PREFIX_BYTES + ENTRY_BYTES) as u64;
        let a = entry(SectionKind::EdgeOffsets, ElementKind::U32, 1, table_end + 1);
        let p = payload_with(&[a], 64);
        assert!(matches!(
            validate_sections(&p, &DecodeLimits::strict()),
            Err(WireError::SectionMisaligned { align: 4, .. })
        ));
    }

    #[test]
    fn a_length_that_disagrees_with_the_element_count_is_rejected() {
        let table_end = (TABLE_PREFIX_BYTES + ENTRY_BYTES) as u64;
        let a = SectionEntry {
            kind: SectionKind::EdgeOffsets,
            element_kind: ElementKind::U32,
            element_count: 4,
            byte_offset: table_end,
            byte_len: 12, // should be 16
        };
        let p = payload_with(&[a], 64);
        assert!(matches!(
            validate_sections(&p, &DecodeLimits::strict()),
            Err(WireError::SectionLengthMismatch {
                expected: 16,
                got: 12,
                ..
            })
        ));
    }

    #[test]
    fn duplicate_singletons_are_rejected_but_patches_may_repeat() {
        let table_end = (TABLE_PREFIX_BYTES + 2 * ENTRY_BYTES) as u64;

        let a = entry(SectionKind::EdgeOffsets, ElementKind::U32, 2, table_end);
        let b = entry(SectionKind::EdgeOffsets, ElementKind::U32, 2, table_end + 8);
        let p = payload_with(&[a, b], 32);
        assert!(matches!(
            validate_sections(&p, &DecodeLimits::strict()),
            Err(WireError::DuplicateSection { index: 1, .. })
        ));

        let a = entry(
            SectionKind::WeightPatchRange,
            ElementKind::F32,
            2,
            table_end,
        );
        let b = entry(
            SectionKind::WeightPatchRange,
            ElementKind::F32,
            2,
            table_end + 8,
        );
        let p = payload_with(&[a, b], 32);
        assert!(
            validate_sections(&p, &DecodeLimits::strict()).is_ok(),
            "repeatable patch sections must be allowed to appear twice"
        );
    }

    #[test]
    fn unknown_sections_skip_or_reject_by_criticality() {
        let table_end = (TABLE_PREFIX_BYTES + ENTRY_BYTES) as u64;

        // Below the boundary and unknown: must refuse.
        let mut raw = entry(SectionKind::EdgeOffsets, ElementKind::U32, 1, table_end);
        raw.kind = SectionKind::Unknown(999);
        let p = payload_with(&[raw], 32);
        assert!(matches!(
            validate_sections(&p, &DecodeLimits::strict()),
            Err(WireError::UnknownMandatorySection { kind: 999, .. })
        ));

        // At or above the boundary: skippable, and preserved for forwarding.
        let mut fut = entry(SectionKind::EdgeOffsets, ElementKind::Raw8, 4, table_end);
        fut.kind = SectionKind::Unknown(CRITICAL_SECTION_MAX);
        let p = payload_with(&[fut], 32);
        let secs = validate_sections(&p, &DecodeLimits::strict()).expect("skippable");
        assert_eq!(secs[0].kind(), SectionKind::Unknown(CRITICAL_SECTION_MAX));
        assert!(secs[0].kind().is_skippable());
    }

    #[test]
    fn a_tiny_message_cannot_request_an_enormous_allocation() {
        // 8 bytes of table prefix claiming u32::MAX entries. The count check
        // must fire before Vec::with_capacity is reached.
        let mut p = Vec::new();
        p.extend_from_slice(&u32::MAX.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            validate_sections(&p, &DecodeLimits::DEFAULT),
            Err(WireError::TooManySections { got: u32::MAX, .. })
        ));
    }

    #[test]
    fn typed_access_copies_and_respects_the_declared_element_kind() {
        let table_end = (TABLE_PREFIX_BYTES + ENTRY_BYTES) as u64;
        let a = entry(
            SectionKind::IncidenceWeights,
            ElementKind::F32,
            2,
            table_end,
        );
        let mut p = payload_with(&[a], 8);
        let body = TABLE_PREFIX_BYTES + ENTRY_BYTES;
        p[body..body + 4].copy_from_slice(&1.5f32.to_le_bytes());
        p[body + 4..body + 8].copy_from_slice(&(-2.5f32).to_le_bytes());

        let secs = validate_sections(&p, &DecodeLimits::strict()).expect("valid");
        assert_eq!(secs[0].to_f32_vec().expect("f32"), vec![1.5, -2.5]);
        assert!(
            secs[0].to_u32_vec().is_err(),
            "reading an f32 section as u32 must be refused"
        );
    }

    #[test]
    fn every_truncation_of_a_valid_payload_errors_rather_than_panics() {
        let table_end = (TABLE_PREFIX_BYTES + ENTRY_BYTES) as u64;
        let a = entry(SectionKind::EdgeOffsets, ElementKind::U32, 4, table_end);
        let full = payload_with(&[a], 16);
        for n in 0..full.len() {
            // Must not panic; either an error or (for a prefix that happens to
            // be self-consistent) a valid parse.
            let _ = validate_sections(&full[..n], &DecodeLimits::strict());
        }
    }
}
