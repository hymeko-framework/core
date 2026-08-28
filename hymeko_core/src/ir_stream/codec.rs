// APPROVED-CORE-EDIT: ir-stream-wire-codec (2026-08-28, Phase 10A brief).

//! The validation entry point: bytes in, `ValidatedMessage` or an error out.
//!
//! # The pipeline, in order
//!
//! ```text
//! &[u8]
//!   -> envelope decoding          magic, schema, header length, type, flags
//!   -> checked structural bounds  limits, checked arithmetic, exact length
//!   -> digest validation          BLAKE3 over the payload
//!   -> section-table validation   bounds, alignment, ordering, uniqueness
//!   -> ValidatedMessage
//! ```
//!
//! The order is not arbitrary. The digest is checked **before** the section
//! table is parsed, so no attacker-chosen structure is interpreted until the
//! bytes are known to be the ones the producer sent. The limits are checked
//! before the digest, because hashing an enormous buffer is itself a denial of
//! service.
//!
//! # Digest coverage
//!
//! `payload_hash` is BLAKE3 over **exactly the payload bytes**, `payload_len`
//! of them, and nothing else. The envelope is not covered.
//!
//! That is deliberate and is the coverage specified in
//! `docs/hypergraph-synchronization.md` §5.1. The envelope is protected by
//! structural validation — magic, schema, header length, known type, zero
//! reserved, exact total length — every field of which is checked before use.
//! What the digest does *not* provide is authenticity: an attacker who can
//! rewrite bytes can rewrite both payload and hash. Authenticity is a transport
//! concern (signed or authenticated channels) and is deliberately not solved
//! here, because solving it badly at this layer would look like security
//! without being it.
//!
//! # Trailing bytes
//!
//! **Rejected.** `bytes.len()` must equal `header_bytes + payload_len` exactly.
//! A framing that tolerated trailing bytes would let two consumers disagree
//! about where a message ends, which is the ambiguity that makes stream
//! resynchronisation unreliable.

use super::envelope::{EnvelopeDraft, ValidatedEnvelope};
use super::error::WireError;
use super::limits::DecodeLimits;
use super::section::{ValidatedSection, validate_sections};

/// A message whose envelope, digest and section table have all been validated.
///
/// Borrows the input buffer: the only allocation performed during validation is
/// the section-entry vector, bounded by [`DecodeLimits::max_section_count`].
#[derive(Debug)]
pub struct ValidatedMessage<'a> {
    envelope: ValidatedEnvelope,
    payload: &'a [u8],
    sections: Vec<ValidatedSection<'a>>,
}

impl<'a> ValidatedMessage<'a> {
    #[must_use]
    pub const fn envelope(&self) -> &ValidatedEnvelope {
        &self.envelope
    }

    /// The raw payload. Already digest-verified.
    #[must_use]
    pub const fn payload(&self) -> &'a [u8] {
        self.payload
    }

    /// Validated sections, in increasing byte order.
    ///
    /// Empty for message types that do not carry a section table.
    #[must_use]
    pub fn sections(&self) -> &[ValidatedSection<'a>] {
        &self.sections
    }

    /// The first section of a given kind, if present.
    #[must_use]
    pub fn section(&self, kind: super::section::SectionKind) -> Option<&ValidatedSection<'a>> {
        self.sections.iter().find(|s| s.kind() == kind)
    }
}

/// Validate a complete message.
///
/// # Errors
///
/// Any [`WireError`]. Every rejection names the rule that fired, so a caller
/// can distinguish "this producer is too new" from "these bytes are corrupt"
/// from "this is not our protocol".
///
/// # Panics
///
/// Never, for any input. Every index is bounds-checked or preceded by an
/// explicit length check, and every arithmetic step is checked.
/// `no_input_can_panic_the_validator` drives a hostile corpus through this
/// function to assert it.
pub fn validate_message<'a>(
    bytes: &'a [u8],
    limits: &DecodeLimits,
) -> Result<ValidatedMessage<'a>, WireError> {
    // 1. Envelope: magic first, then schema, then anything interpreted.
    let envelope = ValidatedEnvelope::decode(bytes)?;

    // 2. Limits, before hashing anything. Hashing an attacker-declared
    //    multi-gigabyte buffer would be a denial of service in its own right.
    if envelope.payload_len() > limits.max_payload_bytes {
        return Err(WireError::PayloadTooLarge {
            got: envelope.payload_len(),
            max: limits.max_payload_bytes,
        });
    }

    let total = u64::from(envelope.header_bytes())
        .checked_add(envelope.payload_len())
        .ok_or(WireError::LengthOverflow {
            what: "header_bytes + payload_len",
        })?;
    if total > limits.max_message_bytes {
        return Err(WireError::MessageTooLarge {
            got: total,
            max: limits.max_message_bytes,
        });
    }

    // 3. Exact length. Trailing bytes are rejected; see the module docs.
    let total_us = usize::try_from(total).map_err(|_| WireError::NotAddressable {
        what: "header_bytes + payload_len",
        got: total,
    })?;
    if bytes.len() != total_us {
        return Err(WireError::LengthMismatch {
            expected: total,
            got: bytes.len(),
        });
    }

    let header_us = usize::from(envelope.header_bytes());
    let payload = &bytes[header_us..total_us];

    // 4. Digest, over exactly the payload, before any structure is parsed.
    let computed = blake3::hash(payload);
    if computed.as_bytes() != envelope.payload_hash() {
        return Err(WireError::HashMismatch);
    }

    // 5. Section table, only for the types that carry one.
    let sections = if envelope.msg_type().carries_sections() {
        validate_sections(payload, limits)?
    } else {
        Vec::new()
    };

    Ok(ValidatedMessage {
        envelope,
        payload,
        sections,
    })
}

/// Assemble a message from an envelope and a payload, filling in the digest.
///
/// Present so producers and tests build messages through the same rules the
/// validator enforces, rather than by hand. It is not a general serialization
/// framework and deliberately does not build section tables — a producer that
/// needs one composes the payload itself, which keeps this function to the one
/// job the format actually constrains.
#[must_use]
pub fn encode_message(draft: &EnvelopeDraft, payload: &[u8]) -> Vec<u8> {
    let hash = blake3::hash(payload);
    let header = draft.to_bytes(payload.len() as u64, hash.as_bytes());
    let mut out = Vec::with_capacity(header.len() + payload.len());
    out.extend_from_slice(&header);
    out.extend_from_slice(payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir_stream::envelope::{HEADER_BYTES, MessageType, SCHEMA_VERSION};
    use crate::ir_stream::version::SnapshotVersion;

    fn envelope(msg_type: MessageType) -> EnvelopeDraft {
        EnvelopeDraft {
            stream_id: 42,
            seq: 1,
            txn_id: 1,
            target_version: SnapshotVersion::new(1, 1, 1, 0),
            ..EnvelopeDraft::new(msg_type)
        }
    }

    #[test]
    fn a_control_message_with_no_payload_validates() {
        let msg = encode_message(&envelope(MessageType::Heartbeat), &[]);
        let v = validate_message(&msg, &DecodeLimits::strict()).expect("valid");
        assert_eq!(v.envelope().msg_type(), MessageType::Heartbeat);
        assert!(v.payload().is_empty());
        assert!(v.sections().is_empty());
    }

    #[test]
    fn round_trip_preserves_every_envelope_field() {
        for t in [
            MessageType::Hello,
            MessageType::Commit,
            MessageType::Abort,
            MessageType::Ready,
            MessageType::Nack,
            MessageType::ResyncRequest,
            MessageType::Credit,
        ] {
            let payload = vec![7u8; 33];
            let msg = encode_message(&envelope(t), &payload);
            let v = validate_message(&msg, &DecodeLimits::strict()).expect("valid");
            assert_eq!(v.envelope().msg_type(), t);
            assert_eq!(v.envelope().stream_id(), 42);
            assert_eq!(
                v.envelope().target_version(),
                SnapshotVersion::new(1, 1, 1, 0)
            );
            assert_eq!(v.payload(), payload.as_slice());
        }
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut msg = encode_message(&envelope(MessageType::Heartbeat), &[1, 2, 3]);
        msg.push(0);
        assert!(matches!(
            validate_message(&msg, &DecodeLimits::strict()),
            Err(WireError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn a_short_buffer_is_rejected() {
        let msg = encode_message(&envelope(MessageType::Heartbeat), &[1, 2, 3]);
        assert!(matches!(
            validate_message(&msg[..msg.len() - 1], &DecodeLimits::strict()),
            Err(WireError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn any_payload_bit_flip_is_caught_by_the_digest() {
        let payload: Vec<u8> = (0..64u8).collect();
        let msg = encode_message(&envelope(MessageType::Heartbeat), &payload);
        for byte in HEADER_BYTES as usize..msg.len() {
            for bit in 0..8 {
                let mut bad = msg.clone();
                bad[byte] ^= 1 << bit;
                assert!(
                    matches!(
                        validate_message(&bad, &DecodeLimits::strict()),
                        Err(WireError::HashMismatch)
                    ),
                    "flip at byte {byte} bit {bit} was not caught"
                );
            }
        }
    }

    #[test]
    fn a_flipped_hash_field_is_caught() {
        let msg = encode_message(&envelope(MessageType::Heartbeat), &[9, 9, 9]);
        let mut bad = msg.clone();
        bad[112] ^= 0x01;
        assert!(matches!(
            validate_message(&bad, &DecodeLimits::strict()),
            Err(WireError::HashMismatch)
        ));
    }

    #[test]
    fn limits_are_enforced_before_hashing() {
        // Declare a payload far past the limit without providing the bytes: the
        // limit check must fire, not a length or hash error, which proves the
        // ordering.
        let e = envelope(MessageType::Heartbeat);
        let mut msg = e.to_bytes(u64::MAX, &[0; 32]).to_vec();
        msg[104..112].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(
            validate_message(&msg, &DecodeLimits::strict()),
            Err(WireError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn header_plus_payload_overflow_is_checked() {
        let e = envelope(MessageType::Heartbeat);
        let mut msg = e.to_bytes(u64::MAX, &[0; 32]).to_vec();
        msg[104..112].copy_from_slice(&u64::MAX.to_le_bytes());
        // With an enormous limit the addition itself must be the guard.
        let permissive = DecodeLimits {
            max_payload_bytes: u64::MAX,
            max_message_bytes: u64::MAX,
            ..DecodeLimits::DEFAULT
        };
        assert!(matches!(
            validate_message(&msg, &permissive),
            Err(WireError::LengthOverflow { .. })
        ));
    }

    #[test]
    fn no_input_can_panic_the_validator() {
        // A hostile corpus: every truncation of a valid message, every single
        // byte flipped in the header, and a set of adversarial length fields.
        let payload: Vec<u8> = (0..96u8).collect();
        let good = encode_message(&envelope(MessageType::SnapshotChunk), &payload);

        for n in 0..=good.len() {
            let _ = validate_message(&good[..n], &DecodeLimits::strict());
        }
        for byte in 0..HEADER_BYTES as usize {
            let mut bad = good.clone();
            bad[byte] ^= 0xFF;
            let _ = validate_message(&bad, &DecodeLimits::strict());
        }
        for hostile in [0u64, 1, 1 << 61, (1 << 61) - 1, u64::MAX] {
            let mut bad = good.clone();
            bad[104..112].copy_from_slice(&hostile.to_le_bytes());
            let _ = validate_message(&bad, &DecodeLimits::strict());
            let _ = validate_message(&bad, &DecodeLimits::DEFAULT);
        }
        // Empty and sub-header inputs.
        let _ = validate_message(&[], &DecodeLimits::strict());
        let _ = validate_message(&[0u8; 7], &DecodeLimits::strict());
    }
}

#[cfg(test)]
mod frozen {
    //! Known-answer tests, independent of the encoder/decoder round trip.
    //!
    //! # Why round-trip tests are not enough
    //!
    //! `encode -> decode -> compare` passes even if the encoder and the decoder
    //! are wrong in the same way — a swapped pair of offsets, or a consistent
    //! byte-order mistake, cancels out. The vector below is a fixed byte string
    //! that was decoded **by hand** against the specification table in
    //! `docs/hypergraph-synchronization.md` §5.1, field by field, before being
    //! frozen here. A change to either side that alters the wire image fails
    //! this test even though the round trip still passes.
    //!
    //! # Provenance of the digest
    //!
    //! No BLAKE3 command-line tool exists on the build host (`b3sum`, `blake3`
    //! and `rhash` are absent; `openssl` offers only BLAKE2). Independence is
    //! therefore established a different way: [`blake3_matches_the_published_empty_vector`]
    //! asserts the **official BLAKE3 test vector** for the empty input, which is
    //! published by the algorithm's authors and is not derived from this crate.
    //! If that matches, the implementation is genuine BLAKE3 and the frozen
    //! digests below are trustworthy as fixtures.

    use super::*;
    use crate::ir_stream::envelope::{FLAG_LAST_CHUNK, HEADER_BYTES, MessageType, SCHEMA_VERSION};
    use crate::ir_stream::version::SnapshotVersion;

    /// The frozen message: a `Commit` with the payload `b"HyMeKo"`.
    const FROZEN_HEX: &str = "534b4d48010090000600010000000000efcdab89674523018877665544332211\
                               ff00ff00ff00ff00010000000000000002000000000000000300000000000000\
                               0400000000000000050000000000000006000000000000000700000000000000\
                               08000000000000000600000000000000d73afba8b1049226255d0779fdb38039\
                               49fd3621e35b1d799feb1f3e56e8107b48794d654b6f";

    /// BLAKE3 of the empty input, from the algorithm's published test vectors.
    const BLAKE3_EMPTY: &str = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";

    fn frozen_bytes() -> Vec<u8> {
        let hex: String = FROZEN_HEX.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(hex.len().is_multiple_of(2), "fixture must be whole bytes");
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("fixture hex"))
            .collect()
    }

    #[test]
    fn blake3_matches_the_published_empty_vector() {
        // Anchors the fixtures below to the algorithm rather than to this crate.
        assert_eq!(
            blake3::hash(b"").to_hex().as_str(),
            BLAKE3_EMPTY,
            "the blake3 dependency does not match the published BLAKE3 test vector; \
             every frozen digest in this module is suspect"
        );
    }

    #[test]
    fn the_frozen_vector_decodes_to_the_hand_checked_values() {
        let bytes = frozen_bytes();
        assert_eq!(bytes.len(), 150, "144-byte header + 6-byte payload");

        let m =
            validate_message(&bytes, &DecodeLimits::DEFAULT).expect("frozen vector must decode");
        let e = m.envelope();

        assert_eq!(e.schema_version(), SCHEMA_VERSION);
        assert_eq!(e.header_bytes(), HEADER_BYTES);
        assert_eq!(e.msg_type(), MessageType::Commit);
        assert_eq!(e.flags(), FLAG_LAST_CHUNK);
        assert_eq!(e.stream_id(), 0x0123_4567_89AB_CDEF);
        assert_eq!(e.seq(), 0x1122_3344_5566_7788);
        assert_eq!(e.txn_id(), 0x00FF_00FF_00FF_00FF);
        assert_eq!(e.base_version(), SnapshotVersion::new(1, 2, 3, 4));
        assert_eq!(e.target_version(), SnapshotVersion::new(5, 6, 7, 8));
        assert_eq!(e.payload_len(), 6);
        assert_eq!(m.payload(), b"HyMeKo");
    }

    #[test]
    fn the_frozen_vector_has_the_expected_bytes_at_every_documented_offset() {
        // Read straight out of the fixture, without using the decoder at all.
        let b = frozen_bytes();
        assert_eq!(&b[0..4], &[0x53, 0x4b, 0x4d, 0x48], "magic, little-endian");
        assert_eq!(&b[4..6], &[0x01, 0x00], "schema 1");
        assert_eq!(&b[6..8], &[0x90, 0x00], "header_bytes 144");
        assert_eq!(&b[8..10], &[0x06, 0x00], "msg_type 6 = Commit");
        assert_eq!(&b[10..12], &[0x01, 0x00], "flags = LAST_CHUNK");
        assert_eq!(&b[12..16], &[0, 0, 0, 0], "reserved zero");
        assert_eq!(
            &b[16..24],
            &[0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01],
            "stream_id least-significant byte first"
        );
        assert_eq!(&b[104..112], &[0x06, 0, 0, 0, 0, 0, 0, 0], "payload_len 6");
        assert_eq!(&b[144..150], b"HyMeKo", "payload");
    }

    #[test]
    fn the_frozen_digest_is_blake3_of_the_payload_and_nothing_else() {
        let b = frozen_bytes();
        let declared = &b[112..144];
        // Coverage is exactly the payload: not the header, not the whole buffer.
        assert_eq!(declared, blake3::hash(b"HyMeKo").as_bytes(), "payload only");
        assert_ne!(
            declared,
            blake3::hash(&b).as_bytes(),
            "not the whole message"
        );
        assert_ne!(
            declared,
            blake3::hash(&b[..144]).as_bytes(),
            "not the header"
        );
    }

    #[test]
    fn the_encoder_reproduces_the_frozen_bytes_exactly() {
        // The other direction: encoder output must equal the hand-checked image.
        let e = EnvelopeDraft {
            flags: FLAG_LAST_CHUNK,
            stream_id: 0x0123_4567_89AB_CDEF,
            seq: 0x1122_3344_5566_7788,
            txn_id: 0x00FF_00FF_00FF_00FF,
            base_version: SnapshotVersion::new(1, 2, 3, 4),
            target_version: SnapshotVersion::new(5, 6, 7, 8),
            ..EnvelopeDraft::new(MessageType::Commit)
        };
        assert_eq!(encode_message(&e, b"HyMeKo"), frozen_bytes());
    }

    #[test]
    fn no_single_byte_mutation_can_be_silently_absorbed() {
        // The property that actually matters is not "every flip is rejected" --
        // flipping msg_type 6 (Commit) to 7 (Abort) is a different, legitimate
        // message. It is that no flip produces something that both validates AND
        // is indistinguishable from the original. Silent absorption is the
        // failure mode; a different-but-valid message is not.
        let good = frozen_bytes();
        let original = validate_message(&good, &DecodeLimits::DEFAULT).expect("frozen is valid");
        let orig_env = *original.envelope();
        let orig_payload = original.payload().to_vec();

        for i in 0..good.len() {
            for bit in 0..8 {
                let mut bad = good.clone();
                bad[i] ^= 1 << bit;
                if let Ok(m) = validate_message(&bad, &DecodeLimits::DEFAULT) {
                    assert!(
                        *m.envelope() != orig_env || m.payload() != orig_payload.as_slice(),
                        "byte {i} bit {bit} flipped and the message still decoded identically"
                    );
                }
            }
        }
    }
}
