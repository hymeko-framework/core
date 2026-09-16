// APPROVED-CORE-EDIT: ir-stream-cbor-snapshot (2026-09-15, user chat).

//! Canonical CBOR snapshots carried by an `ir_stream` snapshot transaction.
//!
//! # Wire shape
//!
//! One document, as produced by [`super::encode`] or [`super::encode_compiled`], travels as
//!
//! ```text
//! SnapshotBegin   empty section table
//! SnapshotChunk   one CanonicalIrCbor section of Raw8 bytes   (x n; FLAG_LAST_CHUNK on the last)
//! Commit
//! ```
//!
//! The document is the concatenation of the staged `CanonicalIrCbor` bodies in staging order:
//! message order, then section-table order. The kind lies below the critical boundary, so a consumer
//! built before it existed refuses the message instead of silently skipping part of a model.
//!
//! # Trust
//!
//! The producer does not decode the document, and neither the payload digest nor this module
//! provides authenticity. [`decode_snapshot`] therefore ends in the complete [`super::decode`]
//! boundary: preflight, schema, identifiers, index, coherence and the recomputed canonical hash.
//! Nothing is installed here; the caller receives a validated [`DecodedCborPayload`].

use super::{CborError, DecodedCborPayload, decode};
use crate::ir_stream::section::{ENTRY_BYTES, SECTION_ALIGN, SectionEntry, TABLE_PREFIX_BYTES};
use crate::ir_stream::{
    CommittedTransaction, DecodeLimits, ElementKind, EnvelopeDraft, FLAG_LAST_CHUNK, MessageType,
    SectionKind, SnapshotVersion, TransactionKind, encode_message,
};
use std::fmt;

/// Payload offset of the single section body in a chunk: the table prefix and one entry.
const SECTION_BODY_OFFSET: usize = TABLE_PREFIX_BYTES + ENTRY_BYTES;

// Section bodies must start on the wire alignment; the fixed one-entry layout guarantees it.
const _: () = assert!((SECTION_BODY_OFFSET as u64).is_multiple_of(SECTION_ALIGN));

/// A section table with no entries: the payload of the `SnapshotBegin`.
const EMPTY_SECTION_TABLE: [u8; TABLE_PREFIX_BYTES] = [0; TABLE_PREFIX_BYTES];

/// Identity and fragmentation of one snapshot transaction.
///
/// `target_version` must satisfy the assembler's version rules: it dominates what the consumer
/// holds, and a topology bump also bumps `weights` and `features`. A draft that breaks them produces
/// well-formed messages that the consumer's assembler refuses at `SnapshotBegin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotMessageDraft {
    pub stream_id: u64,
    /// Sequence number of the `SnapshotBegin`; the chunks and the `Commit` take the next ones.
    pub first_seq: u64,
    pub txn_id: u64,
    pub base_version: SnapshotVersion,
    pub target_version: SnapshotVersion,
    /// Largest CBOR fragment one `SnapshotChunk` carries. The consumer's `DecodeLimits` must admit a
    /// section of this many bytes and a payload of 40 more.
    pub max_fragment_bytes: usize,
}

/// Why a snapshot could not be produced or accepted.
#[derive(Debug)]
pub enum CborStreamError {
    /// The document to send has no bytes.
    EmptyDocument,
    /// `max_fragment_bytes` is zero.
    ZeroFragmentSize,
    /// The transaction's sequence numbers would pass `u64::MAX`.
    SequenceOverflow,
    /// A CBOR snapshot replaces state and is carried only by a snapshot transaction.
    NotASnapshot { kind: TransactionKind },
    /// The transaction carries no section at all.
    NoFragments,
    /// A section other than `CanonicalIrCbor`; the snapshot is the complete state, so a numeric
    /// section beside it would be a second authority.
    UnexpectedSection { index: usize, kind: SectionKind },
    /// A `CanonicalIrCbor` section whose elements are not opaque bytes.
    WrongElementKind { index: usize, got: ElementKind },
    /// The reassembled document exceeds `DecodeLimits::max_message_bytes`.
    TooLarge { got: u64, max: u64 },
    /// The reassembled bytes failed the canonical CBOR boundary.
    Cbor(CborError),
}

impl fmt::Display for CborStreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyDocument => write!(f, "a CBOR snapshot needs at least one byte"),
            Self::ZeroFragmentSize => write!(f, "snapshot fragments must hold at least one byte"),
            Self::SequenceOverflow => {
                write!(f, "the snapshot's sequence numbers do not fit in u64")
            }
            Self::NotASnapshot { kind } => write!(
                f,
                "a CBOR snapshot needs a snapshot transaction, got {kind:?}"
            ),
            Self::NoFragments => {
                write!(
                    f,
                    "the snapshot transaction carries no CanonicalIrCbor section"
                )
            }
            Self::UnexpectedSection { index, kind } => write!(
                f,
                "section {index} is {kind:?}; a CBOR snapshot carries only CanonicalIrCbor sections"
            ),
            Self::WrongElementKind { index, got } => write!(
                f,
                "section {index} declares {got:?} elements; CanonicalIrCbor requires Raw8"
            ),
            Self::TooLarge { got, max } => {
                write!(
                    f,
                    "reassembled CBOR snapshot has {got} bytes; limit is {max}"
                )
            }
            Self::Cbor(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for CborStreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Cbor(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CborError> for CborStreamError {
    fn from(error: CborError) -> Self {
        Self::Cbor(error)
    }
}

/// Split one encoded CBOR document into the messages of a snapshot transaction.
///
/// # Preconditions
///
/// `cbor` came from [`super::encode`] or [`super::encode_compiled`]; it is not decoded here, and a
/// consumer rejects anything else in [`decode_snapshot`]. `draft.first_seq` is the stream's next
/// sequence number.
///
/// # Postconditions
///
/// The result is a `SnapshotBegin` with an empty section table, one `SnapshotChunk` per fragment of
/// at most `max_fragment_bytes` bytes (in document order, `FLAG_LAST_CHUNK` on the last) and a
/// `Commit`, with contiguous sequence numbers starting at `first_seq`. Every message passes
/// [`crate::ir_stream::validate_message`] under limits that admit one fragment.
///
/// # Errors
///
/// [`CborStreamError::EmptyDocument`], [`CborStreamError::ZeroFragmentSize`] and
/// [`CborStreamError::SequenceOverflow`], all checked before any message is built.
pub fn encode_snapshot_messages(
    cbor: &[u8],
    draft: &SnapshotMessageDraft,
) -> Result<Vec<Vec<u8>>, CborStreamError> {
    if cbor.is_empty() {
        return Err(CborStreamError::EmptyDocument);
    }
    if draft.max_fragment_bytes == 0 {
        return Err(CborStreamError::ZeroFragmentSize);
    }
    let fragments = cbor.chunks(draft.max_fragment_bytes);
    let chunk_count = fragments.len();
    // Begin, the chunks and Commit use first_seq ..= first_seq + chunk_count + 1.
    let commit_offset = u64::try_from(chunk_count)
        .ok()
        .and_then(|count| count.checked_add(1))
        .filter(|offset| draft.first_seq.checked_add(*offset).is_some())
        .ok_or(CborStreamError::SequenceOverflow)?;

    let envelope = |msg_type: MessageType, offset: u64, flags: u16| EnvelopeDraft {
        msg_type,
        flags,
        stream_id: draft.stream_id,
        // No overflow: first_seq + commit_offset was checked and offset <= commit_offset.
        seq: draft.first_seq + offset,
        txn_id: draft.txn_id,
        base_version: draft.base_version,
        target_version: draft.target_version,
    };

    let mut messages = Vec::with_capacity(chunk_count + 2);
    messages.push(encode_message(
        &envelope(MessageType::SnapshotBegin, 0, 0),
        &EMPTY_SECTION_TABLE,
    ));
    for (offset, fragment) in (1..commit_offset).zip(fragments) {
        let flags = if offset + 1 == commit_offset {
            FLAG_LAST_CHUNK
        } else {
            0
        };
        messages.push(encode_message(
            &envelope(MessageType::SnapshotChunk, offset, flags),
            &single_section_payload(SectionKind::CanonicalIrCbor, ElementKind::Raw8, fragment),
        ));
    }
    messages.push(encode_message(
        &envelope(MessageType::Commit, commit_offset, 0),
        &[],
    ));
    Ok(messages)
}

/// Reassemble and validate the CBOR document of a committed snapshot transaction.
///
/// # Preconditions
///
/// `transaction` came from [`crate::ir_stream::TransactionAssembler::apply`], whose
/// `AssemblerLimits` admitted the document: `max_staged_bytes` at least the document length,
/// `max_staged_sections` at least the fragment count, and `max_chunks` at least the fragment count
/// plus one, because the `SnapshotBegin` also counts as a staging message.
///
/// # Postconditions
///
/// On success the payload passed every check of [`super::decode`] with the same `limits`. No state
/// is kept or published on either outcome.
///
/// # Errors
///
/// In order: [`CborStreamError::NotASnapshot`], [`CborStreamError::NoFragments`],
/// [`CborStreamError::UnexpectedSection`], [`CborStreamError::WrongElementKind`],
/// [`CborStreamError::TooLarge`] (checked before the reassembly buffer is allocated) and
/// [`CborStreamError::Cbor`].
pub fn decode_snapshot(
    transaction: &CommittedTransaction,
    limits: &DecodeLimits,
) -> Result<DecodedCborPayload, CborStreamError> {
    if transaction.kind() != TransactionKind::Snapshot {
        return Err(CborStreamError::NotASnapshot {
            kind: transaction.kind(),
        });
    }
    let sections = transaction.sections();
    if sections.is_empty() {
        return Err(CborStreamError::NoFragments);
    }

    let too_large = |got| CborStreamError::TooLarge {
        got,
        max: limits.max_message_bytes,
    };
    let mut total: u64 = 0;
    for (index, section) in sections.iter().enumerate() {
        if section.kind() != SectionKind::CanonicalIrCbor {
            return Err(CborStreamError::UnexpectedSection {
                index,
                kind: section.kind(),
            });
        }
        if section.element_kind() != ElementKind::Raw8 {
            return Err(CborStreamError::WrongElementKind {
                index,
                got: section.element_kind(),
            });
        }
        total = total
            .checked_add(section.bytes().len() as u64)
            .ok_or_else(|| too_large(u64::MAX))?;
    }
    if total > limits.max_message_bytes {
        return Err(too_large(total));
    }

    let capacity = usize::try_from(total).map_err(|_| too_large(total))?;
    let mut document = Vec::with_capacity(capacity);
    for section in sections {
        document.extend_from_slice(section.bytes());
    }
    decode(&document, limits).map_err(CborStreamError::Cbor)
}

/// A section-bearing payload with exactly one section of `kind`, whose body is `body`.
///
/// `body.len()` must be a multiple of the element size. The producer passes `Raw8` fragments; the
/// unit tests also build foreign and mistyped sections with it.
pub(super) fn single_section_payload(
    kind: SectionKind,
    element_kind: ElementKind,
    body: &[u8],
) -> Vec<u8> {
    let byte_len = body.len() as u64;
    let entry = SectionEntry {
        kind,
        element_kind,
        element_count: byte_len / element_kind.size(),
        byte_offset: SECTION_BODY_OFFSET as u64,
        byte_len,
    };
    let mut payload = Vec::with_capacity(SECTION_BODY_OFFSET + body.len());
    payload.extend_from_slice(&1u32.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.extend_from_slice(&entry.to_bytes());
    payload.extend_from_slice(body);
    payload
}
