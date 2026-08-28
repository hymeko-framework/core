// APPROVED-CORE-EDIT: ir-stream-transaction-assembler (2026-08-28, Phase 10B1).

//! The transaction state machine: validated messages in, committed
//! transactions out.
//!
//! # What this is
//!
//! A pure, synchronous consumer-side state machine for **one stream**. It
//! consumes [`ValidatedMessage`] values — nothing else — and decides whether
//! each one advances an open transaction, is a duplicate, aborts, commits, or
//! means the stream has diverged and must be resynchronised.
//!
//! It implements §6 of `docs/hypergraph-synchronization.md`: the
//! `Begin → Chunk* → Commit | Abort` lifecycle, sequence-gap and stale-base
//! detection, idempotence, and bounded staging.
//!
//! # What this deliberately is not
//!
//! No transport, no clock, no thread, no `HyperGraphView`, no GPU type, no
//! async. None of those appear in this module's API or its dependencies. The
//! assembler decides *what a message means*; it does not fetch messages, and it
//! does not apply a committed transaction to anything.
//!
//! Applying a [`CommittedTransaction`] — building a view, publishing a
//! snapshot, uploading buffers — is a later phase. That separation is the point:
//! this layer is fully testable without a graph, a device or a socket.
//!
//! # Staging, and why it is not optional
//!
//! Sections are **copied** into owned buffers as they arrive, and live state is
//! not touched until `Commit`. Two reasons, both load-bearing:
//!
//! * a [`ValidatedMessage`] borrows the byte buffer it was validated from, and
//!   that buffer belongs to the transport, which will reuse it;
//! * a transaction that is abandoned mid-way must leave no trace, so a consumer
//!   never observes a half-applied topology change. This is the property §6.1 of
//!   the design note requires and the reason `Abort` is cheap and total.
//!
//! # Idempotence
//!
//! Two independent mechanisms, because they catch different failures:
//!
//! * **Sequence** — a `seq` at or below the highest seen is a retransmission and
//!   is dropped. This is per-message.
//! * **Version domination** — a transaction whose `target_version` is already
//!   dominated by the consumer's current version is a no-op even if its `seq` is
//!   new. This catches a producer replaying a transaction after a resync, when
//!   sequence numbers have moved on.

use super::codec::ValidatedMessage;
use super::envelope::MessageType;
use super::section::{ElementKind, SectionKind};
use super::version::SnapshotVersion;

/// Ceilings on what one transaction may stage.
///
/// Separate from [`super::DecodeLimits`], which bounds a single *message*. A
/// stream can deliver many legal messages whose accumulated staging is not
/// legal, so the two limits are genuinely different and are checked at
/// different points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssemblerLimits {
    /// Total staged payload bytes across all chunks of one transaction.
    pub max_staged_bytes: usize,
    /// Total staged sections across all chunks of one transaction.
    pub max_staged_sections: usize,
    /// Chunk messages per transaction, so an endless chunk stream is bounded
    /// even when each chunk is small.
    pub max_chunks: u32,
}

impl AssemblerLimits {
    /// 512 MiB, 4096 sections, 65 536 chunks.
    ///
    /// Sized above `DecodeLimits::DEFAULT`'s per-message ceiling so a
    /// transaction may legitimately span several full-sized messages, and far
    /// below any plausible working set.
    pub const DEFAULT: Self = Self {
        max_staged_bytes: 512 * 1024 * 1024,
        max_staged_sections: 4096,
        max_chunks: 65_536,
    };

    /// Tiny limits for tests that must reach the rejection paths cheaply.
    #[must_use]
    pub const fn strict() -> Self {
        Self {
            max_staged_bytes: 64 * 1024,
            max_staged_sections: 16,
            max_chunks: 8,
        }
    }
}

impl Default for AssemblerLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Why the assembler cannot continue and the stream must be resynchronised.
///
/// Every variant carries what the producer needs to decide between replaying
/// from a log and sending a full snapshot, so a `ResyncRequest` can be formed
/// from this without further bookkeeping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResyncReason {
    /// A sequence number was skipped. The missing messages may have carried
    /// section data, so nothing after the gap can be interpreted.
    SequenceGap { expected: u64, got: u64 },
    /// The stream identity changed, which means the producer restarted.
    /// Sequence state from the previous producer is meaningless.
    StreamChanged { had: u64, got: u64 },
    /// A delta was computed against a version this consumer does not hold.
    StaleBase {
        have: SnapshotVersion,
        wanted: SnapshotVersion,
    },
    /// A message arrived that cannot occur in this state — a chunk with no open
    /// transaction, a commit for a different transaction, a `Begin` while one is
    /// already open, or a consumer-to-producer message on a consumer's input.
    UnexpectedMessage {
        msg_type: MessageType,
        had_open_transaction: bool,
    },
    /// A transaction tried to stage more than [`AssemblerLimits`] permits.
    /// Distinct from a decode limit: this is accumulated across chunks.
    StagingLimitExceeded { what: &'static str },
    /// A transaction bumped `topology` without also supplying new `weights` and
    /// `features`. Those are indexed by structure, so the combination is
    /// meaningless — see `docs/hypergraph-synchronization.md` §4.
    IncoherentVersionBump {
        base: SnapshotVersion,
        target: SnapshotVersion,
    },
}

/// What applying a message did.
#[derive(Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Consumed and staged; nothing to publish yet.
    Accepted,
    /// Already seen, or superseded. Deliberately not an error: retransmission
    /// is a normal part of a lossy transport, and dropping silently is what
    /// makes it safe.
    Duplicate,
    /// An open transaction was discarded. Live state is untouched.
    Aborted,
    /// A transaction completed. The consumer's version has advanced.
    Committed(CommittedTransaction),
    /// The stream diverged; the caller should issue a resync request carrying
    /// [`TransactionAssembler::current_version`].
    ResyncRequired(ResyncReason),
}

/// One section, copied out of the borrowed message into owned storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedSection {
    kind: SectionKind,
    element_kind: ElementKind,
    element_count: u64,
    bytes: Vec<u8>,
}

impl StagedSection {
    #[must_use]
    pub const fn kind(&self) -> SectionKind {
        self.kind
    }

    #[must_use]
    pub const fn element_kind(&self) -> ElementKind {
        self.element_kind
    }

    #[must_use]
    pub const fn element_count(&self) -> u64 {
        self.element_count
    }

    /// The owned body. Length equals what validation proved.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// A transaction that reached `Commit` with every rule satisfied.
///
/// Fields are private and there is no public constructor: like
/// [`super::ValidatedEnvelope`], the name asserts a property, so it must not be
/// forgeable. The only source is [`TransactionAssembler::apply`].
#[derive(Debug, PartialEq, Eq)]
pub struct CommittedTransaction {
    stream_id: u64,
    txn_id: u64,
    kind: TransactionKind,
    base_version: SnapshotVersion,
    target_version: SnapshotVersion,
    sections: Vec<StagedSection>,
}

impl CommittedTransaction {
    #[must_use]
    pub const fn stream_id(&self) -> u64 {
        self.stream_id
    }

    #[must_use]
    pub const fn txn_id(&self) -> u64 {
        self.txn_id
    }

    #[must_use]
    pub const fn kind(&self) -> TransactionKind {
        self.kind
    }

    #[must_use]
    pub const fn base_version(&self) -> SnapshotVersion {
        self.base_version
    }

    /// The version the consumer holds after this transaction.
    #[must_use]
    pub const fn target_version(&self) -> SnapshotVersion {
        self.target_version
    }

    #[must_use]
    pub fn sections(&self) -> &[StagedSection] {
        &self.sections
    }

    /// The first staged section of a kind, if present.
    #[must_use]
    pub fn section(&self, kind: SectionKind) -> Option<&StagedSection> {
        self.sections.iter().find(|s| s.kind == kind)
    }

    /// Whether this transaction changes incidence structure.
    ///
    /// The distinction a consumer acts on: a topology change invalidates both
    /// orientations and every derived quantity, while the others permit partial
    /// updates. See `docs/hypergraph-synchronization.md` §8.
    #[must_use]
    pub const fn changes_topology(&self) -> bool {
        self.target_version.topology > self.base_version.topology
    }
}

/// Whether a transaction replaces state or patches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionKind {
    /// Full snapshot: the target version is established outright.
    Snapshot,
    /// Delta: only meaningful against the exact `base_version`.
    Delta,
}

/// Private staging for the one transaction that may be open.
#[derive(Debug)]
struct OpenTransaction {
    txn_id: u64,
    kind: TransactionKind,
    base_version: SnapshotVersion,
    target_version: SnapshotVersion,
    chunks: u32,
    staged_bytes: usize,
    sections: Vec<StagedSection>,
}

/// Consumer-side transaction state for one stream.
///
/// # Invariants
///
/// * At most one transaction is open at a time.
/// * `current_version` changes only at `Commit`.
/// * Staging is discarded completely on abort, on resync, and on any rejection;
///   there is no partial-apply path, so live state is never half-updated.
/// * The assembler never panics on any sequence of validated messages.
#[derive(Debug)]
pub struct TransactionAssembler {
    stream_id: Option<u64>,
    last_seq: Option<u64>,
    current: SnapshotVersion,
    open: Option<OpenTransaction>,
    limits: AssemblerLimits,
}

impl TransactionAssembler {
    /// A consumer that holds nothing and has seen no stream.
    #[must_use]
    pub const fn new(limits: AssemblerLimits) -> Self {
        Self {
            stream_id: None,
            last_seq: None,
            current: SnapshotVersion::ZERO,
            open: None,
            limits,
        }
    }

    /// A consumer resuming at a known version, as after a restart.
    ///
    /// The version is the consumer's durable state; everything else —
    /// stream identity, sequence position, staging — is deliberately not
    /// restored, because a restart cannot know whether the producer also
    /// restarted. The first message re-establishes them.
    #[must_use]
    pub const fn resuming_at(version: SnapshotVersion, limits: AssemblerLimits) -> Self {
        Self {
            stream_id: None,
            last_seq: None,
            current: version,
            open: None,
            limits,
        }
    }

    /// The version this consumer currently holds.
    #[must_use]
    pub const fn current_version(&self) -> SnapshotVersion {
        self.current
    }

    #[must_use]
    pub const fn has_open_transaction(&self) -> bool {
        self.open.is_some()
    }

    /// Bytes currently staged by the open transaction.
    #[must_use]
    pub fn staged_bytes(&self) -> usize {
        self.open.as_ref().map_or(0, |o| o.staged_bytes)
    }

    /// The highest sequence number accepted, if any.
    #[must_use]
    pub const fn last_seq(&self) -> Option<u64> {
        self.last_seq
    }

    /// Discard any open transaction and forget stream position.
    ///
    /// Called after issuing a resync request. `current_version` is retained:
    /// it is what the consumer still legitimately holds and what the producer
    /// needs in order to bring it current.
    pub fn reset_for_resync(&mut self) {
        self.open = None;
        self.stream_id = None;
        self.last_seq = None;
    }

    /// Apply one validated message.
    ///
    /// # Panics
    ///
    /// Never, for any sequence of validated messages.
    #[must_use]
    pub fn apply(&mut self, msg: &ValidatedMessage<'_>) -> ApplyOutcome {
        let env = msg.envelope();

        // --- stream identity ------------------------------------------------
        match self.stream_id {
            None => self.stream_id = Some(env.stream_id()),
            Some(known) if known != env.stream_id() => {
                let reason = ResyncReason::StreamChanged {
                    had: known,
                    got: env.stream_id(),
                };
                self.open = None;
                return ApplyOutcome::ResyncRequired(reason);
            }
            Some(_) => {}
        }

        // --- sequence -------------------------------------------------------
        // Checked before the message is interpreted: a gap means messages that
        // may have carried section data were lost, so nothing after it can be
        // trusted to describe a complete transaction.
        if let Some(last) = self.last_seq {
            if env.seq() <= last {
                return ApplyOutcome::Duplicate;
            }
            let expected = last.saturating_add(1);
            if env.seq() != expected {
                self.open = None;
                return ApplyOutcome::ResyncRequired(ResyncReason::SequenceGap {
                    expected,
                    got: env.seq(),
                });
            }
        }

        let outcome = self.dispatch(msg);

        // A message that advanced the machine advances the sequence. One that
        // demanded a resync does not, because the caller will reset.
        if !matches!(outcome, ApplyOutcome::ResyncRequired(_)) {
            self.last_seq = Some(env.seq());
        }
        outcome
    }

    fn dispatch(&mut self, msg: &ValidatedMessage<'_>) -> ApplyOutcome {
        let env = msg.envelope();
        match env.msg_type() {
            MessageType::Hello | MessageType::Heartbeat => ApplyOutcome::Accepted,

            MessageType::SnapshotBegin => self.begin(msg, TransactionKind::Snapshot),
            MessageType::DeltaBegin => self.begin(msg, TransactionKind::Delta),

            MessageType::SnapshotChunk | MessageType::DeltaChunk => self.chunk(msg),

            MessageType::Commit => self.commit(env.txn_id()),
            MessageType::Abort => {
                if self.open.take().is_some() {
                    ApplyOutcome::Aborted
                } else {
                    // Aborting nothing is harmless and idempotent.
                    ApplyOutcome::Duplicate
                }
            }

            // Consumer-to-producer messages must not arrive on a consumer's
            // input. Treating them as benign would hide a mis-wired stream.
            MessageType::Ready
            | MessageType::Nack
            | MessageType::ResyncRequest
            | MessageType::Credit => {
                ApplyOutcome::ResyncRequired(ResyncReason::UnexpectedMessage {
                    msg_type: env.msg_type(),
                    had_open_transaction: self.open.is_some(),
                })
            }
        }
    }

    fn begin(&mut self, msg: &ValidatedMessage<'_>, kind: TransactionKind) -> ApplyOutcome {
        let env = msg.envelope();

        if self.open.is_some() {
            let reason = ResyncReason::UnexpectedMessage {
                msg_type: env.msg_type(),
                had_open_transaction: true,
            };
            self.open = None;
            return ApplyOutcome::ResyncRequired(reason);
        }

        // Idempotence by version: a transaction that cannot advance us is a
        // replay, whatever its sequence number says.
        if self.current.dominates(&env.target_version()) {
            return ApplyOutcome::Duplicate;
        }

        // A delta is meaningful only against the exact state it was computed
        // from. A snapshot replaces state outright, so its base is advisory.
        if kind == TransactionKind::Delta && env.base_version() != self.current {
            return ApplyOutcome::ResyncRequired(ResyncReason::StaleBase {
                have: self.current,
                wanted: env.base_version(),
            });
        }

        // A topology bump invalidates weights and features, which are indexed by
        // structure, so a transaction that raises topology alone is incoherent.
        let base = if kind == TransactionKind::Snapshot {
            env.base_version()
        } else {
            self.current
        };
        let t = env.target_version();
        if t.topology > base.topology && (t.weights <= base.weights || t.features <= base.features)
        {
            return ApplyOutcome::ResyncRequired(ResyncReason::IncoherentVersionBump {
                base,
                target: t,
            });
        }

        self.open = Some(OpenTransaction {
            txn_id: env.txn_id(),
            kind,
            base_version: env.base_version(),
            target_version: t,
            chunks: 0,
            staged_bytes: 0,
            sections: Vec::new(),
        });

        // A Begin may carry sections of its own.
        self.stage(msg)
    }

    fn chunk(&mut self, msg: &ValidatedMessage<'_>) -> ApplyOutcome {
        let env = msg.envelope();
        match self.open.as_ref() {
            Some(open) if open.txn_id == env.txn_id() => self.stage(msg),
            other => {
                let reason = ResyncReason::UnexpectedMessage {
                    msg_type: env.msg_type(),
                    had_open_transaction: other.is_some(),
                };
                self.open = None;
                ApplyOutcome::ResyncRequired(reason)
            }
        }
    }

    /// Copy this message's sections into the open transaction, enforcing the
    /// accumulated limits **before** each allocation.
    fn stage(&mut self, msg: &ValidatedMessage<'_>) -> ApplyOutcome {
        let Some(open) = self.open.as_mut() else {
            return ApplyOutcome::ResyncRequired(ResyncReason::UnexpectedMessage {
                msg_type: msg.envelope().msg_type(),
                had_open_transaction: false,
            });
        };

        let Some(chunks) = open.chunks.checked_add(1) else {
            self.open = None;
            return ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded {
                what: "chunk count overflowed",
            });
        };
        if chunks > self.limits.max_chunks {
            self.open = None;
            return ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded {
                what: "chunks per transaction",
            });
        }
        open.chunks = chunks;

        for section in msg.sections() {
            if open.sections.len() >= self.limits.max_staged_sections {
                self.open = None;
                return ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded {
                    what: "staged sections",
                });
            }
            let body = section.bytes();
            // Checked, and checked BEFORE the copy that would allocate.
            let Some(total) = open.staged_bytes.checked_add(body.len()) else {
                self.open = None;
                return ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded {
                    what: "staged byte count overflowed",
                });
            };
            if total > self.limits.max_staged_bytes {
                self.open = None;
                return ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded {
                    what: "staged bytes",
                });
            }
            open.staged_bytes = total;
            open.sections.push(StagedSection {
                kind: section.kind(),
                element_kind: section.element_kind(),
                element_count: section.element_count(),
                bytes: body.to_vec(),
            });
        }

        ApplyOutcome::Accepted
    }

    fn commit(&mut self, txn_id: u64) -> ApplyOutcome {
        // `take` unconditionally: a commit naming the wrong transaction still
        // discards the open one, because the producer and consumer no longer
        // agree about what is in flight and staged data cannot be trusted.
        match self.open.take() {
            Some(open) if open.txn_id == txn_id => {
                self.current = open.target_version;
                ApplyOutcome::Committed(CommittedTransaction {
                    stream_id: self.stream_id.unwrap_or_default(),
                    txn_id: open.txn_id,
                    kind: open.kind,
                    base_version: open.base_version,
                    target_version: open.target_version,
                    sections: open.sections,
                })
            }
            other => ApplyOutcome::ResyncRequired(ResyncReason::UnexpectedMessage {
                msg_type: MessageType::Commit,
                had_open_transaction: other.is_some(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir_stream::codec::{encode_message, validate_message};
    use crate::ir_stream::envelope::{EnvelopeDraft, FLAG_LAST_CHUNK};
    use crate::ir_stream::limits::DecodeLimits;
    use crate::ir_stream::section::{ENTRY_BYTES, SectionEntry, TABLE_PREFIX_BYTES};

    const STREAM: u64 = 0xABCD;

    /// An empty section table: zero sections, still eight bytes.
    ///
    /// A section-bearing message always carries a table, even when it has no
    /// sections -- the payload IS the table. A zero-byte payload is therefore
    /// not "no sections", it is a malformed message, and the validator says so.
    fn empty_table() -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&0u32.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes());
        p
    }

    /// A payload carrying one `f32` section of `count` elements.
    fn payload(kind: SectionKind, count: u64) -> Vec<u8> {
        let table = TABLE_PREFIX_BYTES + ENTRY_BYTES;
        let entry = SectionEntry {
            kind,
            element_kind: ElementKind::F32,
            element_count: count,
            byte_offset: table as u64,
            byte_len: count * 4,
        };
        let mut p = Vec::new();
        p.extend_from_slice(&1u32.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes());
        p.extend_from_slice(&entry.to_bytes());
        p.resize(table + (count as usize) * 4, 0);
        p
    }

    struct Producer {
        seq: u64,
        stream_id: u64,
    }

    impl Producer {
        fn new() -> Self {
            Self {
                seq: 0,
                stream_id: STREAM,
            }
        }

        fn msg(
            &mut self,
            msg_type: MessageType,
            txn_id: u64,
            base: SnapshotVersion,
            target: SnapshotVersion,
            body: &[u8],
        ) -> Vec<u8> {
            self.seq += 1;
            // A real producer always emits a table on a section-bearing type.
            let owned;
            let body = if msg_type.carries_sections() && body.is_empty() {
                owned = empty_table();
                owned.as_slice()
            } else {
                body
            };
            let d = EnvelopeDraft {
                flags: FLAG_LAST_CHUNK,
                stream_id: self.stream_id,
                seq: self.seq,
                txn_id,
                base_version: base,
                target_version: target,
                ..EnvelopeDraft::new(msg_type)
            };
            encode_message(&d, body)
        }
    }

    /// Drive bytes through the real validator, as production would.
    fn apply(a: &mut TransactionAssembler, bytes: &[u8]) -> ApplyOutcome {
        let m =
            validate_message(bytes, &DecodeLimits::DEFAULT).expect("test message must be valid");
        a.apply(&m)
    }

    fn v(t: u64, w: u64, f: u64, p: u64) -> SnapshotVersion {
        SnapshotVersion::new(t, w, f, p)
    }

    const ZERO: SnapshotVersion = SnapshotVersion::ZERO;

    // --- the happy path -----------------------------------------------------

    #[test]
    fn a_snapshot_transaction_begins_chunks_and_commits() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let target = v(1, 1, 1, 0);

        let begin = p.msg(
            MessageType::SnapshotBegin,
            7,
            ZERO,
            target,
            &payload(SectionKind::EdgeOffsets, 4),
        );
        assert_eq!(apply(&mut a, &begin), ApplyOutcome::Accepted);
        assert!(a.has_open_transaction());
        assert_eq!(a.current_version(), ZERO, "not yet visible");

        let chunk = p.msg(
            MessageType::SnapshotChunk,
            7,
            ZERO,
            target,
            &payload(SectionKind::IncidenceWeights, 8),
        );
        assert_eq!(apply(&mut a, &chunk), ApplyOutcome::Accepted);

        let commit = p.msg(MessageType::Commit, 7, ZERO, target, &[]);
        match apply(&mut a, &commit) {
            ApplyOutcome::Committed(c) => {
                assert_eq!(c.txn_id(), 7);
                assert_eq!(c.kind(), TransactionKind::Snapshot);
                assert_eq!(c.target_version(), target);
                assert_eq!(c.sections().len(), 2, "both chunks staged");
                assert_eq!(
                    c.section(SectionKind::EdgeOffsets).map(|s| s.bytes().len()),
                    Some(16)
                );
                assert_eq!(
                    c.section(SectionKind::IncidenceWeights)
                        .map(|s| s.bytes().len()),
                    Some(32)
                );
                assert!(c.changes_topology());
            }
            other => panic!("expected Committed, got {other:?}"),
        }
        assert_eq!(a.current_version(), target, "visible only after commit");
        assert!(!a.has_open_transaction());
    }

    #[test]
    fn a_weight_only_delta_commits_and_does_not_change_topology() {
        let mut a = TransactionAssembler::resuming_at(v(3, 3, 3, 1), AssemblerLimits::strict());
        let mut p = Producer::new();
        let base = v(3, 3, 3, 1);
        let target = v(3, 4, 3, 1);

        let begin = p.msg(
            MessageType::DeltaBegin,
            1,
            base,
            target,
            &payload(SectionKind::WeightPatchRange, 4),
        );
        assert_eq!(apply(&mut a, &begin), ApplyOutcome::Accepted);
        let commit = p.msg(MessageType::Commit, 1, base, target, &[]);
        match apply(&mut a, &commit) {
            ApplyOutcome::Committed(c) => {
                assert_eq!(c.kind(), TransactionKind::Delta);
                assert!(
                    !c.changes_topology(),
                    "a weight delta must not claim topology"
                );
            }
            other => panic!("expected Committed, got {other:?}"),
        }
    }

    // --- staging isolation --------------------------------------------------

    #[test]
    fn an_abort_leaves_no_trace() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let target = v(1, 1, 1, 0);

        let begin = p.msg(
            MessageType::SnapshotBegin,
            5,
            ZERO,
            target,
            &payload(SectionKind::EdgeOffsets, 4),
        );
        assert_eq!(apply(&mut a, &begin), ApplyOutcome::Accepted);
        assert!(a.staged_bytes() > 0);

        let abort = p.msg(MessageType::Abort, 5, ZERO, target, &[]);
        assert_eq!(apply(&mut a, &abort), ApplyOutcome::Aborted);
        assert!(!a.has_open_transaction());
        assert_eq!(a.staged_bytes(), 0);
        assert_eq!(
            a.current_version(),
            ZERO,
            "an aborted transaction must not advance the version"
        );
    }

    #[test]
    fn state_is_never_visible_before_commit() {
        // The section 6.1 property: no message other than Commit may advance
        // the version, whatever it claims.
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let target = v(9, 9, 9, 9);
        for t in [MessageType::SnapshotBegin, MessageType::SnapshotChunk] {
            let m = p.msg(t, 1, ZERO, target, &payload(SectionKind::EdgeNodes, 2));
            let _ = apply(&mut a, &m);
            assert_eq!(a.current_version(), ZERO, "{t:?} advanced the version");
        }
    }

    // --- idempotence and duplicates ----------------------------------------

    #[test]
    fn a_replayed_sequence_number_is_a_duplicate() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let m = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert_eq!(apply(&mut a, &m), ApplyOutcome::Accepted);
        // The very same bytes again.
        assert_eq!(apply(&mut a, &m), ApplyOutcome::Duplicate);
        assert_eq!(apply(&mut a, &m), ApplyOutcome::Duplicate);
    }

    #[test]
    fn a_transaction_that_cannot_advance_us_is_a_duplicate_whatever_its_sequence() {
        // Version-level idempotence: the replay arrives with a NEW seq, so the
        // sequence check cannot catch it.
        let mut a = TransactionAssembler::resuming_at(v(5, 5, 5, 5), AssemblerLimits::strict());
        let mut p = Producer::new();
        let begin = p.msg(MessageType::SnapshotBegin, 1, ZERO, v(4, 4, 4, 4), &[]);
        assert_eq!(apply(&mut a, &begin), ApplyOutcome::Duplicate);
        assert!(
            !a.has_open_transaction(),
            "a superseded begin must not open anything"
        );
        assert_eq!(a.current_version(), v(5, 5, 5, 5));
    }

    // --- divergence ---------------------------------------------------------

    #[test]
    fn a_sequence_gap_demands_resync_and_discards_staging() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let target = v(1, 1, 1, 0);
        let begin = p.msg(
            MessageType::SnapshotBegin,
            1,
            ZERO,
            target,
            &payload(SectionKind::EdgeNodes, 2),
        );
        assert_eq!(apply(&mut a, &begin), ApplyOutcome::Accepted);

        p.seq += 3; // three messages lost
        let chunk = p.msg(MessageType::SnapshotChunk, 1, ZERO, target, &[]);
        assert_eq!(
            apply(&mut a, &chunk),
            ApplyOutcome::ResyncRequired(ResyncReason::SequenceGap {
                expected: 2,
                got: 5
            })
        );
        assert!(
            !a.has_open_transaction(),
            "staging must be discarded on a gap"
        );
        assert_eq!(a.current_version(), ZERO);
    }

    #[test]
    fn a_delta_against_the_wrong_base_is_refused() {
        let mut a = TransactionAssembler::resuming_at(v(2, 2, 2, 0), AssemblerLimits::strict());
        let mut p = Producer::new();
        let begin = p.msg(
            MessageType::DeltaBegin,
            1,
            v(1, 1, 1, 0),
            v(2, 3, 2, 0),
            &[],
        );
        assert_eq!(
            apply(&mut a, &begin),
            ApplyOutcome::ResyncRequired(ResyncReason::StaleBase {
                have: v(2, 2, 2, 0),
                wanted: v(1, 1, 1, 0)
            })
        );
        assert!(!a.has_open_transaction());
    }

    #[test]
    fn a_changed_stream_identity_demands_resync() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let m = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert_eq!(apply(&mut a, &m), ApplyOutcome::Accepted);

        p.stream_id = 0x9999;
        p.seq = 0;
        let m2 = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert_eq!(
            apply(&mut a, &m2),
            ApplyOutcome::ResyncRequired(ResyncReason::StreamChanged {
                had: STREAM,
                got: 0x9999
            })
        );
    }

    #[test]
    fn a_chunk_without_an_open_transaction_is_refused() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let chunk = p.msg(MessageType::SnapshotChunk, 3, ZERO, v(1, 1, 1, 0), &[]);
        assert!(matches!(
            apply(&mut a, &chunk),
            ApplyOutcome::ResyncRequired(ResyncReason::UnexpectedMessage { .. })
        ));
    }

    #[test]
    fn a_commit_for_a_different_transaction_is_refused_and_discards_staging() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let target = v(1, 1, 1, 0);
        let begin = p.msg(
            MessageType::SnapshotBegin,
            1,
            ZERO,
            target,
            &payload(SectionKind::EdgeNodes, 2),
        );
        assert_eq!(apply(&mut a, &begin), ApplyOutcome::Accepted);

        let commit = p.msg(MessageType::Commit, 999, ZERO, target, &[]);
        assert!(matches!(
            apply(&mut a, &commit),
            ApplyOutcome::ResyncRequired(ResyncReason::UnexpectedMessage { .. })
        ));
        assert!(!a.has_open_transaction());
        assert_eq!(a.current_version(), ZERO);
    }

    #[test]
    fn a_second_begin_while_one_is_open_is_refused() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let target = v(1, 1, 1, 0);
        let b1 = p.msg(MessageType::SnapshotBegin, 1, ZERO, target, &[]);
        assert_eq!(apply(&mut a, &b1), ApplyOutcome::Accepted);
        let b2 = p.msg(MessageType::SnapshotBegin, 2, ZERO, v(2, 2, 2, 0), &[]);
        assert!(matches!(
            apply(&mut a, &b2),
            ApplyOutcome::ResyncRequired(ResyncReason::UnexpectedMessage {
                had_open_transaction: true,
                ..
            })
        ));
    }

    #[test]
    fn consumer_to_producer_messages_are_refused_on_a_consumer_input() {
        for t in [
            MessageType::Ready,
            MessageType::Nack,
            MessageType::ResyncRequest,
            MessageType::Credit,
        ] {
            let mut a = TransactionAssembler::new(AssemblerLimits::strict());
            let mut p = Producer::new();
            let m = p.msg(t, 0, ZERO, ZERO, &[]);
            assert!(
                matches!(
                    apply(&mut a, &m),
                    ApplyOutcome::ResyncRequired(ResyncReason::UnexpectedMessage { .. })
                ),
                "{t:?} must not be accepted by a consumer"
            );
        }
    }

    #[test]
    fn a_topology_bump_without_weights_and_features_is_incoherent() {
        // Section 4: topology is dominant; weights and features are indexed by
        // it, so raising topology alone describes nothing coherent.
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let begin = p.msg(MessageType::SnapshotBegin, 1, ZERO, v(1, 0, 0, 0), &[]);
        assert!(matches!(
            apply(&mut a, &begin),
            ApplyOutcome::ResyncRequired(ResyncReason::IncoherentVersionBump { .. })
        ));
    }

    // --- bounded staging ----------------------------------------------------

    #[test]
    fn staging_is_bounded_by_bytes() {
        let mut a = TransactionAssembler::new(AssemblerLimits {
            max_staged_bytes: 32,
            ..AssemblerLimits::strict()
        });
        let mut p = Producer::new();
        let target = v(1, 1, 1, 0);
        let begin = p.msg(
            MessageType::SnapshotBegin,
            1,
            ZERO,
            target,
            &payload(SectionKind::EdgeNodes, 64),
        );
        assert!(matches!(
            apply(&mut a, &begin),
            ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded {
                what: "staged bytes"
            })
        ));
        assert!(
            !a.has_open_transaction(),
            "over-limit staging must be discarded"
        );
    }

    #[test]
    fn staging_is_bounded_by_chunk_count() {
        let mut a = TransactionAssembler::new(AssemblerLimits {
            max_chunks: 2,
            ..AssemblerLimits::strict()
        });
        let mut p = Producer::new();
        let target = v(1, 1, 1, 0);
        let begin = p.msg(MessageType::SnapshotBegin, 1, ZERO, target, &[]);
        assert_eq!(apply(&mut a, &begin), ApplyOutcome::Accepted);
        let c1 = p.msg(MessageType::SnapshotChunk, 1, ZERO, target, &[]);
        assert_eq!(apply(&mut a, &c1), ApplyOutcome::Accepted);
        let c2 = p.msg(MessageType::SnapshotChunk, 1, ZERO, target, &[]);
        assert!(matches!(
            apply(&mut a, &c2),
            ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded {
                what: "chunks per transaction"
            })
        ));
    }

    // --- recovery -----------------------------------------------------------

    #[test]
    fn resync_keeps_the_version_and_forgets_the_position() {
        let mut a = TransactionAssembler::resuming_at(v(4, 4, 4, 4), AssemblerLimits::strict());
        let mut p = Producer::new();
        let m = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert_eq!(apply(&mut a, &m), ApplyOutcome::Accepted);

        a.reset_for_resync();
        assert_eq!(
            a.current_version(),
            v(4, 4, 4, 4),
            "version is durable state"
        );
        assert_eq!(a.last_seq(), None, "position is not");
        assert!(!a.has_open_transaction());

        // A different producer, sequence restarting from 1, is now acceptable.
        let mut p2 = Producer::new();
        p2.stream_id = 0x1234;
        let m2 = p2.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert_eq!(apply(&mut a, &m2), ApplyOutcome::Accepted);
    }

    #[test]
    fn a_full_snapshot_and_a_delta_chain_reach_the_same_version_and_content() {
        // Claim C3 of the design note, at the version level: two routes from
        // ZERO to one target must leave the consumer in the same state.
        let target = v(1, 2, 1, 0);

        let mut direct = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let b = p.msg(
            MessageType::SnapshotBegin,
            1,
            ZERO,
            target,
            &payload(SectionKind::IncidenceWeights, 4),
        );
        assert_eq!(apply(&mut direct, &b), ApplyOutcome::Accepted);
        let c = p.msg(MessageType::Commit, 1, ZERO, target, &[]);
        let direct_sections = match apply(&mut direct, &c) {
            ApplyOutcome::Committed(t) => t.sections().to_vec(),
            other => panic!("expected Committed, got {other:?}"),
        };

        let mut chained = TransactionAssembler::new(AssemblerLimits::strict());
        let mut q = Producer::new();
        let mid = v(1, 1, 1, 0);
        let b1 = q.msg(MessageType::SnapshotBegin, 1, ZERO, mid, &[]);
        assert_eq!(apply(&mut chained, &b1), ApplyOutcome::Accepted);
        let c1 = q.msg(MessageType::Commit, 1, ZERO, mid, &[]);
        assert!(matches!(
            apply(&mut chained, &c1),
            ApplyOutcome::Committed(_)
        ));
        let b2 = q.msg(
            MessageType::DeltaBegin,
            2,
            mid,
            target,
            &payload(SectionKind::IncidenceWeights, 4),
        );
        assert_eq!(apply(&mut chained, &b2), ApplyOutcome::Accepted);
        let c2 = q.msg(MessageType::Commit, 2, mid, target, &[]);
        let chained_sections = match apply(&mut chained, &c2) {
            ApplyOutcome::Committed(t) => t.sections().to_vec(),
            other => panic!("expected Committed, got {other:?}"),
        };

        assert_eq!(direct.current_version(), chained.current_version());
        assert_eq!(direct_sections, chained_sections);
    }

    #[test]
    fn no_message_sequence_can_panic_the_assembler() {
        // Every message type, in every state, over all ordered triples.
        let types = [
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
        for first in types {
            for second in types {
                for third in types {
                    let mut a = TransactionAssembler::new(AssemblerLimits::strict());
                    let mut p = Producer::new();
                    for t in [first, second, third] {
                        let m = p.msg(t, 1, ZERO, v(1, 1, 1, 0), &[]);
                        let out = apply(&mut a, &m);
                        if matches!(out, ApplyOutcome::ResyncRequired(_)) {
                            a.reset_for_resync();
                            p.seq = 0;
                        }
                    }
                }
            }
        }
    }
}
