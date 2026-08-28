// APPROVED-CORE-EDIT: ir-stream-wire-codec (2026-08-28, Phase 10A brief).

//! HyMeKo IR-stream wire codec: a hostile-input validator for the IPC boundary.
//!
//! # What this is
//!
//! The pure, transport-independent boundary between untrusted bytes and
//! anything this process is willing to interpret:
//!
//! ```text
//! &[u8]
//!   -> envelope decoding
//!   -> checked structural validation
//!   -> digest validation
//!   -> section-table validation
//!   -> ValidatedMessage
//! ```
//!
//! Holding a [`ValidatedMessage`] means every field was read explicitly as
//! little-endian, every length was checked, the payload digest matched, and
//! every section body lies inside the payload at a legal offset. It does *not*
//! mean the message is semantically applicable — sequence ordering, base-version
//! matching, transaction staging and snapshot publication all belong to layers
//! this module deliberately does not implement.
//!
//! # What this is not
//!
//! Not a transport: no iceoryx2, no sockets, no async. Not a serialization
//! framework: there is no derive, no schema compiler, and no attempt to encode
//! arbitrary Rust types. Not a transaction engine. The full protocol —
//! `Begin`/`Chunk`/`Commit`/`Abort`, resynchronisation, backpressure, snapshot
//! lifetime against in-flight GPU submissions — is specified in
//! `docs/hypergraph-synchronization.md` and implemented by later phases.
//!
//! # Design rules, each individually tested
//!
//! * The input buffer is never cast to a Rust struct and never `transmute`d.
//! * No reference is ever formed into unaligned wire bytes; typed section
//!   access **copies**.
//! * No `usize`, `isize`, `bool`, `char` or Rust `enum` appears in the wire
//!   representation. Discriminants travel as integers and are converted through
//!   checked `TryFrom` only after validation.
//! * Every integer is read byte-by-byte as little-endian, so the same bytes
//!   decode identically on any host.
//! * Every `u64` → `usize` conversion is checked, so a 32-bit consumer reports
//!   rather than truncates.
//! * Every offset and length arithmetic step is checked. The audited
//!   predecessor computed `nnz * size_of::<i64>()` unchecked and, at
//!   `nnz = u64::MAX`, produced descending offsets and a total smaller than its
//!   own header — a buffer-overrun primitive reachable from any corrupt
//!   producer.
//! * Resource limits are validated **before** the allocation they bound, so a
//!   200-byte message cannot request a four-billion-entry `Vec`.
//! * Trailing bytes are rejected; the buffer length must equal
//!   `header_bytes + payload_len` exactly.
//!
//! # Relationship to `tensor::shared_state`
//!
//! That module is the quarantined predecessor. It is deprecated, its arithmetic
//! has been made checked, and it is documented as **not a wire protocol**. It is
//! retained only so that a downstream caller outside this repository does not
//! break on an import; nothing here is derived from it.
//!
//! # Example
//!
//! ```
//! use hymeko::ir_stream::{
//!     DecodeLimits, MessageType, SnapshotVersion, ValidatedEnvelope,
//!     encode_message, validate_message,
//! };
//! use hymeko::ir_stream::envelope::{HEADER_BYTES, SCHEMA_VERSION};
//!
//! let envelope = ValidatedEnvelope {
//!     schema_version: SCHEMA_VERSION,
//!     header_bytes: HEADER_BYTES,
//!     msg_type: MessageType::Heartbeat,
//!     flags: 0,
//!     stream_id: 1,
//!     seq: 1,
//!     txn_id: 0,
//!     base_version: SnapshotVersion::ZERO,
//!     target_version: SnapshotVersion::new(3, 1, 1, 0),
//!     payload_len: 0,
//!     payload_hash: [0; 32],
//! };
//!
//! let bytes = encode_message(&envelope, b"payload");
//! let message = validate_message(&bytes, &DecodeLimits::DEFAULT)?;
//! assert_eq!(message.envelope().target_version.topology, 3);
//! assert_eq!(message.payload(), b"payload");
//!
//! // A single flipped bit is rejected, not tolerated.
//! let mut corrupt = bytes.clone();
//! corrupt[HEADER_BYTES as usize] ^= 1;
//! assert!(validate_message(&corrupt, &DecodeLimits::DEFAULT).is_err());
//! # Ok::<(), hymeko::ir_stream::WireError>(())
//! ```

pub mod codec;
pub mod envelope;
pub mod error;
pub mod limits;
pub mod section;
pub mod version;

pub use codec::{ValidatedMessage, encode_message, validate_message};
pub use envelope::{FLAG_LAST_CHUNK, MessageType, ValidatedEnvelope};
pub use error::WireError;
pub use limits::DecodeLimits;
pub use section::{ElementKind, SectionEntry, SectionKind, ValidatedSection};
pub use version::SnapshotVersion;
