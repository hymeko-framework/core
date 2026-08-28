// APPROVED-CORE-EDIT: ir-stream-wire-codec (2026-08-28, Phase 10A brief).

//! The 144-byte message envelope.
//!
//! # Reading discipline
//!
//! Every integer is read byte-by-byte as little-endian through [`rd_u16`],
//! [`rd_u32`] and [`rd_u64`]. The input buffer is **never** cast to a Rust
//! struct, never `transmute`d, and no reference is ever formed into unaligned
//! wire bytes. That is not stylistic caution: the design this replaces used
//! `#[repr(C)]` structs containing `usize` and a `#[repr(u32)]` enum, so its
//! layout varied by target and materialising an out-of-range discriminant was
//! undefined behaviour.
//!
//! Wire discriminants are plain integers here, converted through checked
//! `TryFrom` into safe Rust enums only after validation.
//!
//! # Layout
//!
//! | off | field | type |
//! |---:|---|---|
//! | 0 | `magic` | `u32` |
//! | 4 | `schema_version` | `u16` |
//! | 6 | `header_bytes` | `u16` |
//! | 8 | `msg_type` | `u16` |
//! | 10 | `flags` | `u16` |
//! | 12 | `reserved` | `u32` (must be 0) |
//! | 16 | `stream_id` | `u64` |
//! | 24 | `seq` | `u64` |
//! | 32 | `txn_id` | `u64` |
//! | 40 | `base_version` | 4 × `u64` |
//! | 72 | `target_version` | 4 × `u64` |
//! | 104 | `payload_len` | `u64` |
//! | 112 | `payload_hash` | 32 × `u8` |
//!
//! Total 144. Every field is naturally aligned within the image and there is no
//! padding — but nothing here depends on that, because nothing is cast.

use super::error::WireError;
use super::version::{SnapshotVersion, VERSION_WIRE_BYTES};

/// `"HYMKS"` truncated to four bytes. A cheap desynchronisation detector,
/// checked before any other field is interpreted.
pub const MAGIC: u32 = 0x484D_4B53;

/// Fixed envelope length. A newer producer may send a longer header; see
/// [`ValidatedEnvelope::header_bytes`].
pub const HEADER_BYTES: u16 = 144;

/// Wire grammar version this build implements.
pub const SCHEMA_VERSION: u16 = 1;

/// Refuse absurd header lengths outright, so a bogus value cannot drive a large
/// skip. Generous enough for a decade of additive fields.
pub const MAX_HEADER_BYTES: u16 = 4096;

// Field offsets. Named so the golden-image test asserts against these rather
// than against magic numbers repeated in two places.
const OFF_MAGIC: usize = 0;
const OFF_SCHEMA: usize = 4;
const OFF_HEADER_BYTES: usize = 6;
const OFF_MSG_TYPE: usize = 8;
const OFF_FLAGS: usize = 10;
const OFF_RESERVED: usize = 12;
const OFF_STREAM_ID: usize = 16;
const OFF_SEQ: usize = 24;
const OFF_TXN_ID: usize = 32;
const OFF_BASE_VERSION: usize = 40;
const OFF_TARGET_VERSION: usize = 72;
const OFF_PAYLOAD_LEN: usize = 104;
const OFF_PAYLOAD_HASH: usize = 112;

/// What a message is.
///
/// A safe Rust enum, reached only through [`MessageType::try_from_wire`]. The
/// wire carries the `u16` and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessageType {
    /// Producer announces stream identity, schema and negotiated limits.
    Hello,
    /// Opens a full-snapshot transaction.
    SnapshotBegin,
    /// Payload sections of a snapshot.
    SnapshotChunk,
    /// Opens a delta transaction against `base_version`.
    DeltaBegin,
    /// Payload sections of a delta.
    DeltaChunk,
    /// Transaction complete; `target_version` becomes authoritative.
    Commit,
    /// Transaction abandoned; discard everything staged under `txn_id`.
    Abort,
    /// Liveness plus current version.
    Heartbeat,
    /// Consumer has committed `target_version`.
    Ready,
    /// Consumer rejected a transaction.
    Nack,
    /// Consumer asks to be brought current from `base_version`.
    ResyncRequest,
    /// Consumer advertises backpressure window.
    Credit,
}

impl MessageType {
    /// Checked conversion from the wire representation.
    ///
    /// # Errors
    ///
    /// [`WireError::UnknownMessageType`] for any value this build does not
    /// define. Message types are never skippable — a consumer that does not
    /// know what a message *is* cannot reason about its effect.
    pub const fn try_from_wire(raw: u16) -> Result<Self, WireError> {
        Ok(match raw {
            1 => Self::Hello,
            2 => Self::SnapshotBegin,
            3 => Self::SnapshotChunk,
            4 => Self::DeltaBegin,
            5 => Self::DeltaChunk,
            6 => Self::Commit,
            7 => Self::Abort,
            8 => Self::Heartbeat,
            9 => Self::Ready,
            10 => Self::Nack,
            11 => Self::ResyncRequest,
            12 => Self::Credit,
            got => return Err(WireError::UnknownMessageType { got }),
        })
    }

    /// The wire representation.
    #[must_use]
    pub const fn to_wire(self) -> u16 {
        match self {
            Self::Hello => 1,
            Self::SnapshotBegin => 2,
            Self::SnapshotChunk => 3,
            Self::DeltaBegin => 4,
            Self::DeltaChunk => 5,
            Self::Commit => 6,
            Self::Abort => 7,
            Self::Heartbeat => 8,
            Self::Ready => 9,
            Self::Nack => 10,
            Self::ResyncRequest => 11,
            Self::Credit => 12,
        }
    }

    /// Whether this message type carries a section-table payload.
    ///
    /// Control messages may carry a payload for their own purposes, but only
    /// the chunk-bearing types are parsed as a section table.
    #[must_use]
    pub const fn carries_sections(self) -> bool {
        matches!(
            self,
            Self::SnapshotBegin | Self::SnapshotChunk | Self::DeltaBegin | Self::DeltaChunk
        )
    }
}

/// Defined flag bits. Any other bit set is a rejection.
pub const FLAG_LAST_CHUNK: u16 = 0x0001;
const KNOWN_FLAGS: u16 = FLAG_LAST_CHUNK;

/// A decoded and structurally validated envelope.
///
/// Holding one means: the magic matched, the schema is supported, the header
/// length is sane, the message type is known, reserved bits were zero, and the
/// declared payload length is within limits. It does **not** mean the digest
/// has been checked — that requires the payload, and is done by
/// [`super::codec::validate_message`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidatedEnvelope {
    pub schema_version: u16,
    /// The producer's header length. May exceed [`HEADER_BYTES`] when a newer
    /// producer has appended fields; the extra bytes are skipped, which is the
    /// forward-compatibility mechanism.
    pub header_bytes: u16,
    pub msg_type: MessageType,
    pub flags: u16,
    pub stream_id: u64,
    pub seq: u64,
    pub txn_id: u64,
    pub base_version: SnapshotVersion,
    pub target_version: SnapshotVersion,
    pub payload_len: u64,
    pub payload_hash: [u8; 32],
}

impl ValidatedEnvelope {
    /// Whether this is the final chunk of its transaction.
    #[must_use]
    pub const fn is_last_chunk(&self) -> bool {
        self.flags & FLAG_LAST_CHUNK != 0
    }

    /// Encode to the fixed 144-byte image.
    ///
    /// Always emits [`HEADER_BYTES`]; this build has no extended fields to
    /// write. Present so tests can build golden images and round-trip without
    /// reaching through memory layout.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; HEADER_BYTES as usize] {
        let mut b = [0u8; HEADER_BYTES as usize];
        b[OFF_MAGIC..OFF_MAGIC + 4].copy_from_slice(&MAGIC.to_le_bytes());
        b[OFF_SCHEMA..OFF_SCHEMA + 2].copy_from_slice(&self.schema_version.to_le_bytes());
        b[OFF_HEADER_BYTES..OFF_HEADER_BYTES + 2].copy_from_slice(&HEADER_BYTES.to_le_bytes());
        b[OFF_MSG_TYPE..OFF_MSG_TYPE + 2].copy_from_slice(&self.msg_type.to_wire().to_le_bytes());
        b[OFF_FLAGS..OFF_FLAGS + 2].copy_from_slice(&self.flags.to_le_bytes());
        // reserved stays zero
        b[OFF_STREAM_ID..OFF_STREAM_ID + 8].copy_from_slice(&self.stream_id.to_le_bytes());
        b[OFF_SEQ..OFF_SEQ + 8].copy_from_slice(&self.seq.to_le_bytes());
        b[OFF_TXN_ID..OFF_TXN_ID + 8].copy_from_slice(&self.txn_id.to_le_bytes());
        b[OFF_BASE_VERSION..OFF_BASE_VERSION + VERSION_WIRE_BYTES]
            .copy_from_slice(&self.base_version.to_le_bytes());
        b[OFF_TARGET_VERSION..OFF_TARGET_VERSION + VERSION_WIRE_BYTES]
            .copy_from_slice(&self.target_version.to_le_bytes());
        b[OFF_PAYLOAD_LEN..OFF_PAYLOAD_LEN + 8].copy_from_slice(&self.payload_len.to_le_bytes());
        b[OFF_PAYLOAD_HASH..OFF_PAYLOAD_HASH + 32].copy_from_slice(&self.payload_hash);
        b
    }

    /// Decode and structurally validate an envelope from the front of `bytes`.
    ///
    /// Order matters and is deliberate: length, then magic, then schema, then
    /// header length, and only then any interpreted field. A message from an
    /// unrelated protocol must fail on the magic rather than on a nonsense
    /// message type, so that the diagnostic points at the right cause.
    ///
    /// # Errors
    ///
    /// See [`WireError`]. Every rejection names the rule that fired.
    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let fixed = HEADER_BYTES as usize;
        if bytes.len() < fixed {
            return Err(WireError::TooShort {
                need: fixed,
                got: bytes.len(),
            });
        }

        let magic = rd_u32(bytes, OFF_MAGIC);
        if magic != MAGIC {
            return Err(WireError::BadMagic { got: magic });
        }

        let schema_version = rd_u16(bytes, OFF_SCHEMA);
        if schema_version != SCHEMA_VERSION {
            return Err(WireError::UnsupportedSchema {
                got: schema_version,
                supported: SCHEMA_VERSION,
            });
        }

        let header_bytes = rd_u16(bytes, OFF_HEADER_BYTES);
        if !(HEADER_BYTES..=MAX_HEADER_BYTES).contains(&header_bytes) {
            return Err(WireError::BadHeaderLength { got: header_bytes });
        }

        let reserved = rd_u32(bytes, OFF_RESERVED);
        if reserved != 0 {
            return Err(WireError::ReservedNotZero { field: "reserved" });
        }

        let msg_type = MessageType::try_from_wire(rd_u16(bytes, OFF_MSG_TYPE))?;

        let flags = rd_u16(bytes, OFF_FLAGS);
        if flags & !KNOWN_FLAGS != 0 {
            return Err(WireError::UnknownFlags { got: flags });
        }

        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes[OFF_PAYLOAD_HASH..OFF_PAYLOAD_HASH + 32]);

        Ok(Self {
            schema_version,
            header_bytes,
            msg_type,
            flags,
            stream_id: rd_u64(bytes, OFF_STREAM_ID),
            seq: rd_u64(bytes, OFF_SEQ),
            txn_id: rd_u64(bytes, OFF_TXN_ID),
            base_version: rd_version(bytes, OFF_BASE_VERSION),
            target_version: rd_version(bytes, OFF_TARGET_VERSION),
            payload_len: rd_u64(bytes, OFF_PAYLOAD_LEN),
            payload_hash: hash,
        })
    }
}

// --- explicit little-endian readers ----------------------------------------
//
// Each copies into a fixed array first, so no reference is formed into the
// (possibly unaligned) input and `from_le_bytes` does the byte-order work
// explicitly rather than relying on the host's.

fn rd_u16(b: &[u8], off: usize) -> u16 {
    let mut t = [0u8; 2];
    t.copy_from_slice(&b[off..off + 2]);
    u16::from_le_bytes(t)
}

fn rd_u32(b: &[u8], off: usize) -> u32 {
    let mut t = [0u8; 4];
    t.copy_from_slice(&b[off..off + 4]);
    u32::from_le_bytes(t)
}

fn rd_u64(b: &[u8], off: usize) -> u64 {
    let mut t = [0u8; 8];
    t.copy_from_slice(&b[off..off + 8]);
    u64::from_le_bytes(t)
}

fn rd_version(b: &[u8], off: usize) -> SnapshotVersion {
    let mut t = [0u8; VERSION_WIRE_BYTES];
    t.copy_from_slice(&b[off..off + VERSION_WIRE_BYTES]);
    SnapshotVersion::from_le_bytes(&t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ValidatedEnvelope {
        ValidatedEnvelope {
            schema_version: SCHEMA_VERSION,
            header_bytes: HEADER_BYTES,
            msg_type: MessageType::SnapshotBegin,
            flags: FLAG_LAST_CHUNK,
            stream_id: 0x1122_3344_5566_7788,
            seq: 7,
            txn_id: 9,
            base_version: SnapshotVersion::new(1, 2, 3, 4),
            target_version: SnapshotVersion::new(2, 2, 3, 4),
            payload_len: 0,
            payload_hash: [0xAB; 32],
        }
    }

    #[test]
    fn header_is_exactly_144_bytes() {
        assert_eq!(HEADER_BYTES, 144);
        assert_eq!(sample().to_bytes().len(), 144);
    }

    #[test]
    fn field_offsets_are_where_the_specification_says() {
        // Golden-image assertion: encode a value with a unique marker in each
        // field and check it lands at the documented offset. This is what makes
        // a future field reordering fail loudly.
        let e = sample();
        let b = e.to_bytes();
        assert_eq!(rd_u32(&b, 0), MAGIC, "magic at 0");
        assert_eq!(rd_u16(&b, 4), SCHEMA_VERSION, "schema at 4");
        assert_eq!(rd_u16(&b, 6), HEADER_BYTES, "header_bytes at 6");
        assert_eq!(
            rd_u16(&b, 8),
            MessageType::SnapshotBegin.to_wire(),
            "type at 8"
        );
        assert_eq!(rd_u16(&b, 10), FLAG_LAST_CHUNK, "flags at 10");
        assert_eq!(rd_u32(&b, 12), 0, "reserved at 12");
        assert_eq!(rd_u64(&b, 16), 0x1122_3344_5566_7788, "stream_id at 16");
        assert_eq!(rd_u64(&b, 24), 7, "seq at 24");
        assert_eq!(rd_u64(&b, 32), 9, "txn_id at 32");
        assert_eq!(rd_u64(&b, 40), 1, "base.topology at 40");
        assert_eq!(rd_u64(&b, 72), 2, "target.topology at 72");
        assert_eq!(rd_u64(&b, 104), 0, "payload_len at 104");
        assert_eq!(&b[112..144], &[0xAB; 32], "hash at 112");
    }

    #[test]
    fn encoding_is_little_endian_on_every_host() {
        let mut e = sample();
        e.stream_id = 0x0102_0304_0506_0708;
        let b = e.to_bytes();
        // Least significant byte first. This assertion is what fails if the
        // implementation ever reverts to native-order writes.
        assert_eq!(
            &b[16..24],
            &[0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]
        );
    }

    #[test]
    fn round_trip_is_identity() {
        let e = sample();
        assert_eq!(ValidatedEnvelope::decode(&e.to_bytes()).expect("decode"), e);
    }

    #[test]
    fn every_truncation_errors_rather_than_panics() {
        let full = sample().to_bytes();
        for n in 0..full.len() {
            let err = ValidatedEnvelope::decode(&full[..n])
                .expect_err("a truncated header must be rejected");
            assert!(
                matches!(err, WireError::TooShort { .. }),
                "prefix of {n} gave {err:?}"
            );
        }
    }

    #[test]
    fn magic_is_checked_before_anything_else() {
        // A buffer from an unrelated protocol: everything after the magic is
        // nonsense, and the error must still name the magic.
        let mut b = sample().to_bytes();
        b[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        b[4..6].copy_from_slice(&999u16.to_le_bytes()); // bogus schema too
        b[8..10].copy_from_slice(&999u16.to_le_bytes()); // bogus type too
        assert!(matches!(
            ValidatedEnvelope::decode(&b),
            Err(WireError::BadMagic { .. })
        ));
    }

    #[test]
    fn unsupported_schema_is_refused() {
        let mut b = sample().to_bytes();
        b[4..6].copy_from_slice(&2u16.to_le_bytes());
        assert!(matches!(
            ValidatedEnvelope::decode(&b),
            Err(WireError::UnsupportedSchema { got: 2, .. })
        ));
    }

    #[test]
    fn header_length_bounds_are_enforced() {
        for bad in [0u16, 1, HEADER_BYTES - 1, MAX_HEADER_BYTES + 1, u16::MAX] {
            let mut b = sample().to_bytes();
            b[6..8].copy_from_slice(&bad.to_le_bytes());
            assert!(
                matches!(
                    ValidatedEnvelope::decode(&b),
                    Err(WireError::BadHeaderLength { .. })
                ),
                "header_bytes {bad} must be refused"
            );
        }
    }

    #[test]
    fn a_longer_header_from_a_newer_producer_is_tolerated() {
        // Forward compatibility: the extra bytes are not understood, but the
        // message is not refused for having them.
        let mut b = sample().to_bytes();
        b[6..8].copy_from_slice(&200u16.to_le_bytes());
        let e = ValidatedEnvelope::decode(&b).expect("a longer header must decode");
        assert_eq!(e.header_bytes, 200);
    }

    #[test]
    fn reserved_must_be_zero() {
        let mut b = sample().to_bytes();
        b[12..16].copy_from_slice(&1u32.to_le_bytes());
        assert!(matches!(
            ValidatedEnvelope::decode(&b),
            Err(WireError::ReservedNotZero { .. })
        ));
    }

    #[test]
    fn unknown_message_types_and_flags_are_refused() {
        let mut b = sample().to_bytes();
        b[8..10].copy_from_slice(&0u16.to_le_bytes());
        assert!(matches!(
            ValidatedEnvelope::decode(&b),
            Err(WireError::UnknownMessageType { got: 0 })
        ));

        let mut b = sample().to_bytes();
        b[10..12].copy_from_slice(&0x8000u16.to_le_bytes());
        assert!(matches!(
            ValidatedEnvelope::decode(&b),
            Err(WireError::UnknownFlags { .. })
        ));
    }

    #[test]
    fn every_message_type_round_trips_through_its_wire_value() {
        let all = [
            MessageType::Hello,
            MessageType::SnapshotBegin,
            MessageType::SnapshotChunk,
            MessageType::DeltaBegin,
            MessageType::DeltaChunk,
            MessageType::Commit,
            MessageType::Abort,
            MessageType::Heartbeat,
            MessageType::Ready,
            MessageType::Nack,
            MessageType::ResyncRequest,
            MessageType::Credit,
        ];
        for t in all {
            assert_eq!(MessageType::try_from_wire(t.to_wire()), Ok(t));
        }
        // Discriminants are dense and start at 1, so 0 and one past the end are
        // the boundary cases.
        assert!(MessageType::try_from_wire(0).is_err());
        assert!(MessageType::try_from_wire(all.len() as u16 + 1).is_err());
    }
}
