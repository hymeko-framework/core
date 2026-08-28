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

use super::envelope::ValidatedEnvelope;
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
    if envelope.payload_len > limits.max_payload_bytes {
        return Err(WireError::PayloadTooLarge {
            got: envelope.payload_len,
            max: limits.max_payload_bytes,
        });
    }

    let total = u64::from(envelope.header_bytes)
        .checked_add(envelope.payload_len)
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

    let header_us = usize::from(envelope.header_bytes);
    let payload = &bytes[header_us..total_us];

    // 4. Digest, over exactly the payload, before any structure is parsed.
    let computed = blake3::hash(payload);
    if computed.as_bytes() != &envelope.payload_hash {
        return Err(WireError::HashMismatch);
    }

    // 5. Section table, only for the types that carry one.
    let sections = if envelope.msg_type.carries_sections() {
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
pub fn encode_message(envelope: &ValidatedEnvelope, payload: &[u8]) -> Vec<u8> {
    let mut e = *envelope;
    e.payload_len = payload.len() as u64;
    e.payload_hash = *blake3::hash(payload).as_bytes();

    let header = e.to_bytes();
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

    fn envelope(msg_type: MessageType) -> ValidatedEnvelope {
        ValidatedEnvelope {
            schema_version: SCHEMA_VERSION,
            header_bytes: HEADER_BYTES,
            msg_type,
            flags: 0,
            stream_id: 42,
            seq: 1,
            txn_id: 1,
            base_version: SnapshotVersion::ZERO,
            target_version: SnapshotVersion::new(1, 1, 1, 0),
            payload_len: 0,
            payload_hash: [0; 32],
        }
    }

    #[test]
    fn a_control_message_with_no_payload_validates() {
        let msg = encode_message(&envelope(MessageType::Heartbeat), &[]);
        let v = validate_message(&msg, &DecodeLimits::strict()).expect("valid");
        assert_eq!(v.envelope().msg_type, MessageType::Heartbeat);
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
            assert_eq!(v.envelope().msg_type, t);
            assert_eq!(v.envelope().stream_id, 42);
            assert_eq!(
                v.envelope().target_version,
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
        let mut e = envelope(MessageType::Heartbeat);
        e.payload_len = u64::MAX;
        let mut msg = e.to_bytes().to_vec();
        msg[104..112].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(
            validate_message(&msg, &DecodeLimits::strict()),
            Err(WireError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn header_plus_payload_overflow_is_checked() {
        let mut e = envelope(MessageType::Heartbeat);
        e.payload_len = u64::MAX;
        let mut msg = e.to_bytes().to_vec();
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
