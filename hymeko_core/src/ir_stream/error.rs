// APPROVED-CORE-EDIT: ir-stream-wire-codec (2026-08-28, Phase 10A brief).

//! Every way a wire message can be rejected.
//!
//! Hand-written rather than derived, matching `ComputeError`: `hymeko_core`
//! gains no dependency from this module.
//!
//! # Why the variants are this specific
//!
//! A validator that returns one opaque "invalid" tells a producer nothing and
//! makes a test unable to prove *which* check fired. Every variant below
//! corresponds to exactly one check in the pipeline, so a test asserting
//! `WireError::SectionNotIncreasing` is asserting that the ordering rule caught
//! it — not merely that something did.

use core::fmt;

/// Rejection reasons, one per validation rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    /// Fewer bytes than the fixed envelope requires.
    TooShort { need: usize, got: usize },
    /// Leading magic did not match. Checked before anything else is read.
    BadMagic { got: u32 },
    /// `schema_version` outside the range this build understands.
    UnsupportedSchema { got: u16, supported: u16 },
    /// `header_bytes` smaller than the fixed envelope, or beyond the limit.
    BadHeaderLength { got: u16 },
    /// A reserved field was not zero — refuse rather than ignore, so a future
    /// field cannot be mistaken for old noise.
    ReservedNotZero { field: &'static str },
    /// `msg_type` is not a discriminant this build knows.
    UnknownMessageType { got: u16 },
    /// A flag bit outside the defined set was set.
    UnknownFlags { got: u16 },
    /// Declared payload exceeds the configured limit.
    PayloadTooLarge { got: u64, max: u64 },
    /// Whole message exceeds the configured limit.
    MessageTooLarge { got: u64, max: u64 },
    /// Buffer length does not equal `header_bytes + payload_len` exactly.
    /// Trailing bytes are rejected; see the module documentation.
    LengthMismatch { expected: u64, got: usize },
    /// Checked arithmetic would have wrapped. Names the quantity so a test can
    /// assert which computation was protected.
    LengthOverflow { what: &'static str },
    /// A `u64` from the wire does not fit this platform's `usize`.
    NotAddressable { what: &'static str, got: u64 },
    /// Computed digest disagrees with the declared one.
    HashMismatch,
    /// More sections declared than the configured limit allows. Checked before
    /// any allocation sized by that count.
    TooManySections { got: u32, max: u32 },
    /// The section table does not fit inside the payload.
    SectionTableOutOfBounds { need: u64, payload_len: u64 },
    /// A section kind below the critical boundary that this build does not
    /// know. Skipping it would silently drop meaning.
    UnknownMandatorySection { index: usize, kind: u16 },
    /// An element kind this build does not know, on a known section.
    UnknownElementKind { index: usize, kind: u16 },
    /// A singleton section appeared more than once.
    DuplicateSection { index: usize, kind: u16 },
    /// Section body offset is not on the required alignment.
    SectionMisaligned {
        index: usize,
        offset: u64,
        align: u64,
    },
    /// Offsets must strictly increase in table order.
    SectionNotIncreasing {
        index: usize,
        offset: u64,
        previous_end: u64,
    },
    /// Section body extends past the declared payload.
    SectionOutOfBounds {
        index: usize,
        end: u64,
        payload_len: u64,
    },
    /// A single section exceeds the configured byte limit.
    SectionTooLarge { index: usize, got: u64, max: u64 },
    /// Element count exceeds the configured limit.
    TooManyElements { index: usize, got: u64, max: u64 },
    /// `byte_len` disagrees with `element_count * element_size`.
    SectionLengthMismatch {
        index: usize,
        expected: u64,
        got: u64,
    },
    /// The section body starts before the end of the section table.
    SectionOverlapsTable {
        index: usize,
        offset: u64,
        table_end: u64,
    },
}

impl fmt::Display for WireError {
    #[allow(clippy::too_many_lines)] // one arm per variant; a table, not logic
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { need, got } => {
                write!(f, "message too short: need {need} bytes, got {got}")
            }
            Self::BadMagic { got } => write!(f, "bad magic: got {got:#010x}"),
            Self::UnsupportedSchema { got, supported } => write!(
                f,
                "unsupported schema version {got}; this build supports {supported}"
            ),
            Self::BadHeaderLength { got } => write!(f, "invalid header_bytes: {got}"),
            Self::ReservedNotZero { field } => {
                write!(f, "reserved field `{field}` was not zero")
            }
            Self::UnknownMessageType { got } => write!(f, "unknown message type {got}"),
            Self::UnknownFlags { got } => write!(f, "unknown flag bits set: {got:#06x}"),
            Self::PayloadTooLarge { got, max } => {
                write!(f, "payload of {got} bytes exceeds the limit of {max}")
            }
            Self::MessageTooLarge { got, max } => {
                write!(f, "message of {got} bytes exceeds the limit of {max}")
            }
            Self::LengthMismatch { expected, got } => write!(
                f,
                "buffer length {got} does not equal header + payload ({expected}); \
                 trailing bytes are not accepted"
            ),
            Self::LengthOverflow { what } => {
                write!(f, "length arithmetic for `{what}` would overflow")
            }
            Self::NotAddressable { what, got } => {
                write!(f, "`{what}` = {got} does not fit this platform's usize")
            }
            Self::HashMismatch => write!(f, "payload digest does not match the declared hash"),
            Self::TooManySections { got, max } => {
                write!(f, "{got} sections declared, limit is {max}")
            }
            Self::SectionTableOutOfBounds { need, payload_len } => write!(
                f,
                "section table needs {need} bytes but the payload is {payload_len}"
            ),
            Self::UnknownMandatorySection { index, kind } => write!(
                f,
                "section {index} has unknown mandatory kind {kind}; it cannot be skipped"
            ),
            Self::UnknownElementKind { index, kind } => {
                write!(f, "section {index} has unknown element kind {kind}")
            }
            Self::DuplicateSection { index, kind } => {
                write!(f, "section {index} repeats singleton kind {kind}")
            }
            Self::SectionMisaligned {
                index,
                offset,
                align,
            } => write!(
                f,
                "section {index} offset {offset} is not a multiple of {align}"
            ),
            Self::SectionNotIncreasing {
                index,
                offset,
                previous_end,
            } => write!(
                f,
                "section {index} offset {offset} does not follow the previous section, \
                 which ended at {previous_end}"
            ),
            Self::SectionOutOfBounds {
                index,
                end,
                payload_len,
            } => write!(
                f,
                "section {index} ends at {end}, past the payload length {payload_len}"
            ),
            Self::SectionTooLarge { index, got, max } => {
                write!(f, "section {index} is {got} bytes, limit is {max}")
            }
            Self::TooManyElements { index, got, max } => {
                write!(f, "section {index} declares {got} elements, limit is {max}")
            }
            Self::SectionLengthMismatch {
                index,
                expected,
                got,
            } => write!(
                f,
                "section {index} declares {got} bytes but its element count implies {expected}"
            ),
            Self::SectionOverlapsTable {
                index,
                offset,
                table_end,
            } => write!(
                f,
                "section {index} body starts at {offset}, inside the section table \
                 which ends at {table_end}"
            ),
        }
    }
}

impl std::error::Error for WireError {}
