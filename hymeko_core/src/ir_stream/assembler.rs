// APPROVED-CORE-EDIT: ir-stream-content-aware-replay (2026-08-28, Phase 10B1 corrective).
// Supersedes: APPROVED-CORE-EDIT: ir-stream-transaction-assembler (Phase 10B1).

//! The transaction state machine: validated messages in, committed
//! transactions out.
//!
//! # What this is
//!
//! A pure, synchronous consumer-side state machine for **one stream**. It
//! consumes [`ValidatedMessage`] values — nothing else — and decides whether
//! each one advances an open transaction, is a replay, belongs to a superseded
//! transaction, or means the stream has diverged.
//!
//! It implements §6 of `docs/hypergraph-synchronization.md`: the
//! `Begin → Chunk* → Commit | Abort` lifecycle, sequence-gap and stale-base
//! detection, idempotence, and bounded staging.
//!
//! # Replay is content-aware
//!
//! Phase 10B1 returned a single `Duplicate` for every message whose sequence
//! number had been seen. That was measured to collapse five distinct situations
//! into one answer: an identical retransmission, a different message type at the
//! same sequence, a different transaction id, a different version tuple and a
//! different payload were all reported the same way. Two *different*
//! transactions claiming the same target version were also silently equated.
//!
//! A conflict is not a duplicate. It means the producer and consumer disagree
//! about what was sent, which is a resynchronisation condition, and reporting it
//! as a harmless replay hides a real divergence. The outcomes are therefore
//! distinguished — see [`ApplyOutcome`] — and every distinction is backed by a
//! **canonical identity**, never by a sequence number alone.
//!
//! # Cost
//!
//! Replay detection is O(1) in time and O(configured capacity) in memory,
//! constant with stream age: a fixed-capacity ring indexed by `seq % capacity`,
//! each slot recording the sequence it holds so a wrapped collision can never be
//! mistaken for a match. Payload bytes are **never** retained for replay
//! detection and are **never** hashed a second time — a message's payload digest
//! was already computed and verified by the validator, and transaction identity
//! composes those digests rather than re-reading the bytes.
//!
//! # What this deliberately is not
//!
//! No transport, no clock, no thread, no `HyperGraphView`, no GPU type, no
//! async. The assembler decides *what a message means*; it does not fetch
//! messages, and it does not apply a committed transaction to anything.
//! Applying one — building a view, publishing a snapshot, uploading buffers —
//! is a later phase.
//!
//! # Staging, and why it is not optional
//!
//! Sections are **copied** into owned buffers as they arrive, and live state is
//! not touched until `Commit`. Two reasons, both load-bearing:
//!
//! * a [`ValidatedMessage`] borrows the byte buffer it was validated from, and
//!   that buffer belongs to the transport, which will reuse it;
//! * a transaction that is abandoned mid-way must leave no trace, so a consumer
//!   never observes a half-applied topology change.

use super::codec::ValidatedMessage;
use super::envelope::{MessageType, ValidatedEnvelope};
use super::section::{ElementKind, SectionKind};
use super::version::SnapshotVersion;

/// Domain separation for the transaction digest.
///
/// Prefixed to every canonical transaction encoding so a digest computed here
/// can never collide with one computed over a payload, an envelope, or any
/// other BLAKE3 use in this crate — the digests are over different domains even
/// when the bytes would otherwise coincide.
const TXN_DOMAIN: &[u8] = b"hymeko.ir_stream.transaction.v1\x00";

/// Ceilings on what one transaction may stage, and how much replay history is
/// retained.
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
    /// Messages per transaction, so an endless chunk stream is bounded even
    /// when each chunk is small. Applies to a draining superseded transaction
    /// too, which otherwise could be driven forever for free.
    pub max_chunks: u32,
    /// Sequence numbers retained for replay authentication.
    ///
    /// Memory is `capacity × size_of::<Option<MessageIdentity>>()`, constant
    /// with stream age. **Zero has explicit semantics**: no history is retained,
    /// so every replay reports [`ApplyOutcome::ReplayOutsideWindow`] and none
    /// can be authenticated as exact. That is a legitimate configuration for a
    /// consumer that would rather resynchronise than remember.
    pub replay_capacity: usize,
}

impl AssemblerLimits {
    /// 512 MiB staged, 4096 sections, 65 536 messages, 1024 retained sequences.
    ///
    /// The replay capacity is ~136 KiB of identities. It is sized to cover a
    /// retransmission burst, not a session: authenticating a replay from long
    /// ago is what resynchronisation is for.
    pub const DEFAULT: Self = Self {
        max_staged_bytes: 512 * 1024 * 1024,
        max_staged_sections: 4096,
        max_chunks: 65_536,
        replay_capacity: 1024,
    };

    /// Tiny limits for tests that must reach the rejection paths cheaply.
    #[must_use]
    pub const fn strict() -> Self {
        Self {
            max_staged_bytes: 64 * 1024,
            max_staged_sections: 16,
            max_chunks: 8,
            replay_capacity: 16,
        }
    }
}

impl Default for AssemblerLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Why the assembler cannot continue and the stream must be resynchronised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResyncReason {
    /// A sequence number was skipped. The missing messages may have carried
    /// section data, so nothing after the gap can be interpreted.
    SequenceGap { expected: u64, got: u64 },
    /// The stream identity changed, which means the producer restarted.
    StreamChanged { had: u64, got: u64 },
    /// A delta was computed against a version this consumer does not hold.
    StaleBase {
        have: SnapshotVersion,
        wanted: SnapshotVersion,
    },
    /// A message arrived that cannot occur in this state.
    UnexpectedMessage {
        msg_type: MessageType,
        state: AssemblerState,
    },
    /// A transaction tried to stage or drain more than [`AssemblerLimits`]
    /// permits. Accumulated across messages, unlike a decode limit.
    StagingLimitExceeded { what: &'static str },
    /// A target regressed a version component, or bumped `topology` without
    /// also supplying new `weights` and `features` indexed by that structure.
    IncoherentVersionBump {
        base: SnapshotVersion,
        target: SnapshotVersion,
    },
    /// A transaction re-established the version the consumer already holds, but
    /// with **different content**. The two sides disagree about what that
    /// version means, which no amount of replaying can reconcile.
    ConflictingCommit {
        version: SnapshotVersion,
        expected_digest: [u8; 32],
        got_digest: [u8; 32],
    },
}

/// Which part of the lifecycle the assembler is in.
///
/// Carried in [`ResyncReason::UnexpectedMessage`] so a diagnostic says *why* a
/// message was impossible, not merely that it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssemblerState {
    /// No transaction in progress.
    Idle,
    /// Staging a transaction that will advance the version.
    Staging,
    /// Staging a transaction that claims the version already held, whose
    /// identity will be compared at commit.
    RevalidatingReplay,
    /// Draining a transaction already superseded by the current version.
    DiscardingSuperseded,
}

/// What applying a message did.
///
/// The taxonomy exists because a single "duplicate" answer cannot distinguish a
/// harmless retransmission from a genuine disagreement. Phase 10B1 returned one
/// value for both; the corrective measured that and split them.
#[derive(Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Consumed and staged; nothing to publish yet.
    Accepted,
    /// A byte-for-byte identical message already seen at this sequence, or a
    /// transaction whose canonical identity equals the one already committed at
    /// this version. Safe to ignore.
    ExactDuplicate,
    /// A message reusing a sequence number, or re-establishing the current
    /// version, with **different content**. Never safe to ignore.
    ConflictingReplay { seq: u64 },
    /// A message belonging to a transaction whose target the consumer has
    /// already moved past. Consumed and discarded without staging.
    Superseded,
    /// A replay whose sequence is older than the retained window, or whose
    /// established version was restored without its commit identity. It cannot
    /// be authenticated as either exact or conflicting; the caller must recover
    /// rather than publish it as a new commit.
    ReplayOutsideWindow { seq: u64 },
    /// The sequence space is exhausted: no successor to `u64::MAX` exists, so
    /// this stream cannot continue and a new stream identity is required.
    SequenceExhausted,
    /// An open transaction was discarded. Live state is untouched.
    Aborted,
    /// A transaction completed. The consumer's version has advanced.
    Committed(CommittedTransaction),
    /// The stream diverged; issue a resync request carrying
    /// [`TransactionAssembler::current_version`].
    ResyncRequired(ResyncReason),
}

/// The complete identity of one accepted message.
///
/// Fixed size, derived only from already-validated envelope fields. Constructed
/// in O(1): the payload digest is **copied** from the envelope, where the
/// validator already computed and verified it. The payload itself is never
/// retained and never rehashed.
///
/// Equality means every property that could distinguish two messages is equal,
/// which is what makes "exact duplicate" a claim rather than a guess.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageIdentity {
    stream_id: u64,
    seq: u64,
    txn_id: u64,
    msg_type: MessageType,
    base_version: SnapshotVersion,
    target_version: SnapshotVersion,
    payload_digest: [u8; 32],
}

impl MessageIdentity {
    /// O(1); copies 32 bytes of already-verified digest and nothing else.
    #[must_use]
    pub fn of(env: &ValidatedEnvelope) -> Self {
        Self {
            stream_id: env.stream_id(),
            seq: env.seq(),
            txn_id: env.txn_id(),
            msg_type: env.msg_type(),
            base_version: env.base_version(),
            target_version: env.target_version(),
            payload_digest: *env.payload_hash(),
        }
    }

    #[must_use]
    pub const fn seq(&self) -> u64 {
        self.seq
    }

    #[must_use]
    pub const fn payload_digest(&self) -> &[u8; 32] {
        &self.payload_digest
    }
}

/// A fixed-capacity, O(1) replay window indexed by sequence number.
///
/// # Why a ring and not a map
///
/// Sequence numbers are monotone and dense, so `seq % capacity` is a perfect
/// index for the most recent `capacity` of them. A `HashMap` would need
/// eviction bookkeeping and would allocate per insert; this allocates once.
///
/// # Why each slot stores its own sequence
///
/// Two sequences `capacity` apart share a slot. Storing the sequence alongside
/// the identity makes a wrapped collision detectable, so an old entry can never
/// be mistaken for a match — the slot answers "is *this* sequence retained",
/// not merely "is something here".
#[derive(Debug)]
struct ReplayWindow {
    slots: Box<[Option<MessageIdentity>]>,
}

/// What the window can say about a sequence it is asked to authenticate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplayVerdict {
    Exact,
    Conflict,
    Outside,
}

impl ReplayWindow {
    fn new(capacity: usize) -> Self {
        Self {
            slots: vec![None; capacity].into_boxed_slice(),
        }
    }

    /// Bytes of retained identity. Constant with stream age.
    fn memory_bytes(&self) -> usize {
        self.slots.len() * core::mem::size_of::<Option<MessageIdentity>>()
    }

    /// The slot a sequence maps to.
    ///
    /// The cast is exact rather than merely plausible: the modulo result is
    /// strictly less than `slots.len()`, which is itself a `usize`, so the
    /// value provably fits. Asserted in debug builds so the reasoning is
    /// checked rather than trusted.
    fn slot_of(&self, seq: u64) -> usize {
        let len = self.slots.len();
        let slot = (seq % len as u64) as usize;
        debug_assert!(slot < len, "slot index escaped the ring");
        slot
    }

    /// O(1). No scan, no allocation.
    fn verdict(&self, id: &MessageIdentity) -> ReplayVerdict {
        if self.slots.is_empty() {
            // Capacity zero: nothing is retained, so nothing can be
            // authenticated. Explicit, not accidental.
            return ReplayVerdict::Outside;
        }
        let slot = self.slot_of(id.seq);
        match self.slots.get(slot).and_then(|s| s.as_ref()) {
            Some(seen) if seen.seq == id.seq => {
                if seen == id {
                    ReplayVerdict::Exact
                } else {
                    ReplayVerdict::Conflict
                }
            }
            // Empty, or occupied by a different sequence that wrapped onto this
            // slot. Either way this sequence is not retained.
            _ => ReplayVerdict::Outside,
        }
    }

    /// O(1). Called only for messages that were accepted as advancing the
    /// machine, so the window never records something the assembler rejected.
    fn record(&mut self, id: MessageIdentity) {
        if self.slots.is_empty() {
            return;
        }
        let slot = self.slot_of(id.seq);
        if let Some(entry) = self.slots.get_mut(slot) {
            *entry = Some(id);
        }
    }

    fn clear(&mut self) {
        for slot in &mut self.slots {
            *slot = None;
        }
    }
}

/// Incremental canonical digest of a transaction.
///
/// # What it commits to, and why not the bytes
///
/// The staged payload bytes are already committed to by each message's
/// validated payload digest. Hashing them again would be a second O(payload)
/// pass for no additional guarantee, so this composes the digests instead: the
/// result still transitively commits to every payload byte.
///
/// # Canonical encoding
///
/// A domain-separation constant, then fixed-width little-endian fields in a
/// fixed order, then the message count. Every field is fixed width, so no
/// concatenation ambiguity is possible — two different transactions cannot
/// produce the same byte sequence by shifting a boundary. The message count is
/// committed last so a truncated message list cannot masquerade as a shorter
/// transaction.
#[derive(Debug, Clone)]
struct TransactionDigestBuilder {
    hasher: blake3::Hasher,
    messages: u64,
}

impl TransactionDigestBuilder {
    fn new(
        kind: TransactionKind,
        stream_id: u64,
        txn_id: u64,
        base: SnapshotVersion,
        target: SnapshotVersion,
    ) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(TXN_DOMAIN);
        hasher.update(&[kind.to_wire()]);
        hasher.update(&stream_id.to_le_bytes());
        hasher.update(&txn_id.to_le_bytes());
        hasher.update(&base.to_le_bytes());
        hasher.update(&target.to_le_bytes());
        Self {
            hasher,
            messages: 0,
        }
    }

    /// Commit one message to the transaction identity, in arrival order.
    ///
    /// The **index** is included, so reordering two chunks changes the digest
    /// even when the same messages are present.
    ///
    /// The transport **sequence number is deliberately excluded**. It positions
    /// a message in a stream; it does not describe content. Including it was
    /// measured to break the case the digest exists for: a producer replaying a
    /// transaction after a resynchronisation necessarily uses new sequence
    /// numbers, and that legitimate retransmission was reported as a conflicting
    /// commit. Identity here means "the same transaction", not "delivered at the
    /// same stream position".
    fn push(&mut self, id: &MessageIdentity) {
        self.hasher.update(&self.messages.to_le_bytes());
        self.hasher.update(&id.msg_type.to_wire().to_le_bytes());
        self.hasher.update(&id.payload_digest);
        self.messages = self.messages.saturating_add(1);
    }

    /// Finalise, committing the message count.
    fn finish(&self) -> [u8; 32] {
        let mut h = self.hasher.clone();
        h.update(b"count");
        h.update(&self.messages.to_le_bytes());
        *h.finalize().as_bytes()
    }
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

/// Whether a transaction replaces state or patches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionKind {
    /// Full snapshot: the target version is established outright.
    Snapshot,
    /// Delta: only meaningful against the exact `base_version`.
    Delta,
}

impl TransactionKind {
    const fn to_wire(self) -> u8 {
        match self {
            Self::Snapshot => 1,
            Self::Delta => 2,
        }
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
    digest: [u8; 32],
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

    /// The canonical transaction digest.
    ///
    /// Two transactions with equal digests carried the same kind, identity,
    /// versions, and the same ordered messages with the same payloads. This is
    /// what makes "the same version was re-established with the same content" a
    /// checkable claim rather than an assumption about transaction ids.
    #[must_use]
    pub const fn digest(&self) -> &[u8; 32] {
        &self.digest
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
    /// A topology change invalidates both orientations and every derived
    /// quantity; the others permit partial updates. See §8 of the design note.
    #[must_use]
    pub const fn changes_topology(&self) -> bool {
        self.target_version.topology > self.base_version.topology
    }
}

/// Private staging for a transaction that will advance, or re-establish, the
/// version.
#[derive(Debug)]
struct OpenTransaction {
    txn_id: u64,
    kind: TransactionKind,
    base_version: SnapshotVersion,
    target_version: SnapshotVersion,
    /// True when the target equals the version already held: the transaction
    /// may be an exact replay, but that cannot be known until its identity is
    /// complete, so it is staged and compared at commit.
    revalidating: bool,
    messages: u32,
    staged_bytes: usize,
    sections: Vec<StagedSection>,
    digest: TransactionDigestBuilder,
}

/// A transaction the consumer has already moved past, being consumed without
/// staging so its remaining messages are not mistaken for orphans.
#[derive(Debug)]
struct DiscardingTransaction {
    txn_id: u64,
    target_version: SnapshotVersion,
    messages: u32,
}

/// Consumer-side transaction state for one stream.
///
/// # Invariants
///
/// Checked by `debug_invariants` after every `apply` under `cfg(test)`:
///
/// * at most one transaction is open **or** draining, never both;
/// * `current_version` changes only on a new valid `Commit`;
/// * `current_commit_digest` is present exactly when a commit has occurred;
/// * staged bytes equal the sum of staged section lengths;
/// * the replay window holds only accepted message identities;
/// * replay memory is fixed at construction and never grows.
#[derive(Debug)]
pub struct TransactionAssembler {
    stream_id: Option<u64>,
    last_seq: Option<u64>,
    current: SnapshotVersion,
    /// Distinguishes an empty bootstrap from a restored version, including ZERO.
    has_current_version: bool,
    /// Digest of the transaction that established `current`, when one did.
    /// Bounded: 32 bytes, replaced on each commit.
    current_commit_digest: Option<[u8; 32]>,
    open: Option<OpenTransaction>,
    discarding: Option<DiscardingTransaction>,
    replay: ReplayWindow,
    limits: AssemblerLimits,
}

impl TransactionAssembler {
    /// A consumer that holds nothing and has seen no stream.
    #[must_use]
    pub fn new(limits: AssemblerLimits) -> Self {
        Self {
            stream_id: None,
            last_seq: None,
            current: SnapshotVersion::ZERO,
            has_current_version: false,
            current_commit_digest: None,
            open: None,
            discarding: None,
            replay: ReplayWindow::new(limits.replay_capacity),
            limits,
        }
    }

    /// A consumer resuming at a known version, as after a restart.
    ///
    /// The caller must already hold the state corresponding to `version`; this
    /// restores only its version metadata. Stream identity, sequence position,
    /// staging and replay history are deliberately not restored: a
    /// restart cannot know whether the producer also restarted. The commit
    /// digest is likewise unknown, so a transaction re-establishing this exact
    /// version cannot be authenticated as an exact replay and reports
    /// [`ApplyOutcome::ReplayOutsideWindow`] instead of guessing.
    #[must_use]
    pub fn resuming_at(version: SnapshotVersion, limits: AssemblerLimits) -> Self {
        let mut a = Self::new(limits);
        a.current = version;
        a.has_current_version = true;
        a
    }

    /// The version this consumer currently holds.
    #[must_use]
    pub const fn current_version(&self) -> SnapshotVersion {
        self.current
    }

    /// The digest of the transaction that established the current version, if
    /// this assembler observed it.
    #[must_use]
    pub const fn current_commit_digest(&self) -> Option<&[u8; 32]> {
        self.current_commit_digest.as_ref()
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

    /// Fixed bytes of replay history. Does not grow with stream age.
    #[must_use]
    pub fn replay_memory_bytes(&self) -> usize {
        self.replay.memory_bytes()
    }

    /// The highest sequence number accepted, if any.
    #[must_use]
    pub const fn last_seq(&self) -> Option<u64> {
        self.last_seq
    }

    /// Which part of the lifecycle the assembler is in.
    #[must_use]
    pub const fn state(&self) -> AssemblerState {
        if self.discarding.is_some() {
            AssemblerState::DiscardingSuperseded
        } else if let Some(open) = self.open.as_ref() {
            if open.revalidating {
                AssemblerState::RevalidatingReplay
            } else {
                AssemblerState::Staging
            }
        } else {
            AssemblerState::Idle
        }
    }

    /// Discard transaction state and forget stream position and replay history.
    ///
    /// Called after issuing a resync request. `current_version` and its digest
    /// are retained: they are what the consumer still legitimately holds and
    /// what the producer needs in order to bring it current.
    pub fn reset_for_resync(&mut self) {
        self.open = None;
        self.discarding = None;
        self.stream_id = None;
        self.last_seq = None;
        self.replay.clear();
    }

    /// Apply one validated message.
    ///
    /// # Panics
    ///
    /// Never, for any sequence of validated messages.
    #[must_use]
    pub fn apply(&mut self, msg: &ValidatedMessage<'_>) -> ApplyOutcome {
        let outcome = self.apply_inner(msg);
        #[cfg(test)]
        self.debug_invariants();
        outcome
    }

    fn apply_inner(&mut self, msg: &ValidatedMessage<'_>) -> ApplyOutcome {
        let env = msg.envelope();
        let id = MessageIdentity::of(env);

        // --- stream identity ------------------------------------------------
        match self.stream_id {
            None => self.stream_id = Some(id.stream_id),
            Some(known) if known != id.stream_id => {
                let reason = ResyncReason::StreamChanged {
                    had: known,
                    got: id.stream_id,
                };
                self.abandon();
                return ApplyOutcome::ResyncRequired(reason);
            }
            Some(_) => {}
        }

        // --- sequence -------------------------------------------------------
        if let Some(last) = self.last_seq {
            if id.seq <= last {
                // A replay. Which kind is a question about CONTENT, answered by
                // the window, never by the sequence number alone.
                return match self.replay.verdict(&id) {
                    ReplayVerdict::Exact => ApplyOutcome::ExactDuplicate,
                    ReplayVerdict::Conflict => ApplyOutcome::ConflictingReplay { seq: id.seq },
                    // At u64::MAX no successor exists, so a message that cannot
                    // be authenticated is not merely un-rememberd -- the stream
                    // can go no further and needs a new identity. Reported as
                    // exhaustion rather than as an ordinary window miss.
                    ReplayVerdict::Outside if last == u64::MAX => ApplyOutcome::SequenceExhausted,
                    ReplayVerdict::Outside => ApplyOutcome::ReplayOutsideWindow { seq: id.seq },
                };
            }
            // Not a replay, so `seq > last`, which means `last < u64::MAX` and a
            // successor exists. The checked add is kept rather than assumed:
            // an unchecked `+ 1` here would be correct only for as long as the
            // branch above stays exactly as it is.
            let Some(expected) = last.checked_add(1) else {
                return ApplyOutcome::SequenceExhausted;
            };
            if id.seq != expected {
                self.abandon();
                return ApplyOutcome::ResyncRequired(ResyncReason::SequenceGap {
                    expected,
                    got: id.seq,
                });
            }
        }

        let outcome = self.dispatch(msg, &id);

        // Replay bookkeeping is updated only for messages that advanced the
        // machine. A resync-demanding message must not poison the history with
        // an identity that was never accepted.
        if !matches!(
            outcome,
            ApplyOutcome::ResyncRequired(_) | ApplyOutcome::SequenceExhausted
        ) {
            self.last_seq = Some(id.seq);
            self.replay.record(id);
        }
        outcome
    }

    /// Drop all transaction state without touching the committed version.
    fn abandon(&mut self) {
        self.open = None;
        self.discarding = None;
    }

    fn dispatch(&mut self, msg: &ValidatedMessage<'_>, id: &MessageIdentity) -> ApplyOutcome {
        match id.msg_type {
            MessageType::Hello | MessageType::Heartbeat => ApplyOutcome::Accepted,

            MessageType::SnapshotBegin => self.begin(msg, id, TransactionKind::Snapshot),
            MessageType::DeltaBegin => self.begin(msg, id, TransactionKind::Delta),

            MessageType::SnapshotChunk | MessageType::DeltaChunk => self.chunk(msg, id),

            MessageType::Commit => self.commit(id),
            MessageType::Abort => {
                if self.open.take().is_some() || self.discarding.take().is_some() {
                    ApplyOutcome::Aborted
                } else {
                    // Aborting nothing is harmless and idempotent.
                    ApplyOutcome::Accepted
                }
            }

            // Consumer-to-producer messages must not arrive on a consumer's
            // input. Treating them as benign would hide a mis-wired stream.
            MessageType::Ready
            | MessageType::Nack
            | MessageType::ResyncRequest
            | MessageType::Credit => self.unexpected(id.msg_type),
        }
    }

    fn unexpected(&mut self, msg_type: MessageType) -> ApplyOutcome {
        let state = self.state();
        self.abandon();
        ApplyOutcome::ResyncRequired(ResyncReason::UnexpectedMessage { msg_type, state })
    }

    fn begin(
        &mut self,
        msg: &ValidatedMessage<'_>,
        id: &MessageIdentity,
        kind: TransactionKind,
    ) -> ApplyOutcome {
        if self.open.is_some() || self.discarding.is_some() {
            return self.unexpected(id.msg_type);
        }

        let target = id.target_version;

        // Superseded: the consumer has already moved past this target. The
        // transaction's remaining messages will still arrive, so they are
        // drained rather than left to look like orphans.
        if self.current.dominates(&target) && target != self.current {
            self.discarding = Some(DiscardingTransaction {
                txn_id: id.txn_id,
                target_version: target,
                messages: 1,
            });
            return ApplyOutcome::Superseded;
        }

        // A single-writer stream cannot move one version axis backwards while
        // advancing another. Fully superseded targets were handled above.
        if !target.dominates(&self.current) {
            return ApplyOutcome::ResyncRequired(ResyncReason::IncoherentVersionBump {
                base: self.current,
                target,
            });
        }

        // A delta is meaningful only against the exact state it was computed
        // from. A snapshot replaces state outright, so its base is advisory.
        if kind == TransactionKind::Delta && id.base_version != self.current {
            return ApplyOutcome::ResyncRequired(ResyncReason::StaleBase {
                have: self.current,
                wanted: id.base_version,
            });
        }

        // A topology bump invalidates weights and features, which are indexed
        // by structure, so raising topology alone describes nothing coherent.
        let base = if kind == TransactionKind::Snapshot {
            id.base_version
        } else {
            self.current
        };
        if target.topology > base.topology
            && (target.weights <= base.weights || target.features <= base.features)
        {
            return ApplyOutcome::ResyncRequired(ResyncReason::IncoherentVersionBump {
                base,
                target,
            });
        }

        // Equal target: this may be an exact replay of the transaction that
        // established the current version, but its identity is not complete
        // until Commit. Stage it under ordinary limits and compare then --
        // never publish or replace anything before equality is established.
        let revalidating = target == self.current && self.has_current_version;

        let mut digest =
            TransactionDigestBuilder::new(kind, id.stream_id, id.txn_id, id.base_version, target);
        digest.push(id);

        self.open = Some(OpenTransaction {
            txn_id: id.txn_id,
            kind,
            base_version: id.base_version,
            target_version: target,
            revalidating,
            messages: 1,
            staged_bytes: 0,
            sections: Vec::new(),
            digest,
        });

        self.stage(msg)
    }

    fn chunk(&mut self, msg: &ValidatedMessage<'_>, id: &MessageIdentity) -> ApplyOutcome {
        // A chunk of the transaction being drained: consume it, do not stage.
        if let Some(d) = self.discarding.as_mut() {
            if d.txn_id == id.txn_id {
                let Some(n) = d.messages.checked_add(1) else {
                    self.abandon();
                    return ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded {
                        what: "superseded drain message count overflowed",
                    });
                };
                if n > self.limits.max_chunks {
                    self.abandon();
                    return ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded {
                        what: "messages in a superseded transaction",
                    });
                }
                d.messages = n;
                return ApplyOutcome::Superseded;
            }
            return self.unexpected(id.msg_type);
        }

        match self.open.as_ref() {
            Some(open) if open.txn_id == id.txn_id => {
                if let Some(o) = self.open.as_mut() {
                    o.digest.push(id);
                }
                self.stage(msg)
            }
            _ => self.unexpected(id.msg_type),
        }
    }

    /// Copy this message's sections into the open transaction, enforcing the
    /// accumulated limits **before** each allocation.
    fn stage(&mut self, msg: &ValidatedMessage<'_>) -> ApplyOutcome {
        let Some(open) = self.open.as_mut() else {
            return ApplyOutcome::ResyncRequired(ResyncReason::UnexpectedMessage {
                msg_type: msg.envelope().msg_type(),
                state: AssemblerState::Idle,
            });
        };

        let Some(messages) = open.messages.checked_add(1) else {
            self.abandon();
            return ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded {
                what: "message count overflowed",
            });
        };
        // `messages` starts at 1 for the Begin itself, so compare after the
        // increment that this staging call represents.
        if messages > self.limits.max_chunks.saturating_add(1) {
            self.abandon();
            return ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded {
                what: "chunks per transaction",
            });
        }
        open.messages = messages;

        for section in msg.sections() {
            if open.sections.len() >= self.limits.max_staged_sections {
                self.abandon();
                return ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded {
                    what: "staged sections",
                });
            }
            let body = section.bytes();
            // Checked, and checked BEFORE the copy that would allocate.
            let Some(total) = open.staged_bytes.checked_add(body.len()) else {
                self.abandon();
                return ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded {
                    what: "staged byte count overflowed",
                });
            };
            if total > self.limits.max_staged_bytes {
                self.abandon();
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

    fn commit(&mut self, id: &MessageIdentity) -> ApplyOutcome {
        // Completing a superseded transaction: nothing to publish.
        if let Some(d) = self.discarding.take() {
            if d.txn_id == id.txn_id {
                return ApplyOutcome::Superseded;
            }
            return self.unexpected(id.msg_type);
        }

        // `take` unconditionally: a commit naming the wrong transaction still
        // discards the open one, because the two sides no longer agree about
        // what is in flight and staged data cannot be trusted.
        let Some(mut open) = self.open.take() else {
            return ApplyOutcome::ResyncRequired(ResyncReason::UnexpectedMessage {
                msg_type: MessageType::Commit,
                state: AssemblerState::Idle,
            });
        };
        if open.txn_id != id.txn_id {
            return ApplyOutcome::ResyncRequired(ResyncReason::UnexpectedMessage {
                msg_type: MessageType::Commit,
                state: AssemblerState::Staging,
            });
        }

        open.digest.push(id);
        let digest = open.digest.finish();

        // Equal-target case: publish nothing until identity is established.
        if open.revalidating {
            return match self.current_commit_digest {
                Some(known) if known == digest => ApplyOutcome::ExactDuplicate,
                Some(known) => ApplyOutcome::ResyncRequired(ResyncReason::ConflictingCommit {
                    version: open.target_version,
                    expected_digest: known,
                    got_digest: digest,
                }),
                // A restored version without its commit identity cannot
                // authenticate this replay. Keep the restored state intact.
                None => ApplyOutcome::ReplayOutsideWindow { seq: id.seq },
            };
        }

        self.current = open.target_version;
        self.has_current_version = true;
        self.current_commit_digest = Some(digest);
        ApplyOutcome::Committed(CommittedTransaction {
            stream_id: self.stream_id.unwrap_or_default(),
            txn_id: open.txn_id,
            kind: open.kind,
            base_version: open.base_version,
            target_version: open.target_version,
            digest,
            sections: open.sections,
        })
    }

    /// Structural invariants, checked after every `apply` in test builds.
    #[cfg(test)]
    fn debug_invariants(&self) {
        assert!(
            !(self.open.is_some() && self.discarding.is_some()),
            "a transaction cannot be both staged and draining"
        );
        if let Some(open) = self.open.as_ref() {
            let summed: usize = open.sections.iter().map(|s| s.bytes.len()).sum();
            assert_eq!(
                summed, open.staged_bytes,
                "staged byte accounting drifted from the staged sections"
            );
            assert!(
                open.staged_bytes <= self.limits.max_staged_bytes,
                "staged bytes exceeded the configured limit"
            );
            assert!(
                open.sections.len() <= self.limits.max_staged_sections,
                "staged sections exceeded the configured limit"
            );
        }
        assert_eq!(
            self.replay.slots.len(),
            self.limits.replay_capacity,
            "replay window changed size"
        );
        for slot in self.replay.slots.iter().flatten() {
            assert!(
                self.last_seq.is_some_and(|l| slot.seq <= l),
                "replay window holds a sequence that was never accepted"
            );
        }
    }
}

#[cfg(test)]
mod tests_support {
    //! Shared fixtures for the assembler test modules. One copy, so a
    //! probe and the ordinary tests cannot drift apart.
    use super::*;
    use crate::ir_stream::codec::{encode_message, validate_message};
    use crate::ir_stream::envelope::{EnvelopeDraft, FLAG_LAST_CHUNK};
    use crate::ir_stream::limits::DecodeLimits;
    use crate::ir_stream::section::{ENTRY_BYTES, SectionEntry, TABLE_PREFIX_BYTES};

    pub const STREAM: u64 = 0xABCD;

    /// An empty section table: zero sections, still eight bytes.
    ///
    /// A section-bearing message always carries a table, even when it has no
    /// sections -- the payload IS the table. A zero-byte payload is therefore
    /// not "no sections", it is a malformed message, and the validator says so.
    pub fn empty_table() -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&0u32.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes());
        p
    }

    /// A payload carrying one `f32` section of `count` elements.
    pub fn payload(kind: SectionKind, count: u64) -> Vec<u8> {
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

    pub struct Producer {
        pub seq: u64,
        pub stream_id: u64,
    }

    impl Producer {
        pub fn new() -> Self {
            Self {
                seq: 0,
                stream_id: STREAM,
            }
        }

        pub fn msg(
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
    pub fn apply(a: &mut TransactionAssembler, bytes: &[u8]) -> ApplyOutcome {
        let m =
            validate_message(bytes, &DecodeLimits::DEFAULT).expect("test message must be valid");
        a.apply(&m)
    }

    pub fn v(t: u64, w: u64, f: u64, p: u64) -> SnapshotVersion {
        SnapshotVersion::new(t, w, f, p)
    }

    pub const ZERO: SnapshotVersion = SnapshotVersion::ZERO;
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;
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
        assert_eq!(apply(&mut a, &m), ApplyOutcome::ExactDuplicate);
        assert_eq!(apply(&mut a, &m), ApplyOutcome::ExactDuplicate);
    }

    #[test]
    fn a_superseded_transaction_is_drained_not_orphaned() {
        // Phase 10B1 returned Duplicate for the Begin and then forgot the
        // transaction, so its remaining messages arrived as unexplained orphans
        // and the Commit produced a spurious SequenceGap. Measured, then fixed:
        // the transaction is drained without staging.
        let mut a = TransactionAssembler::resuming_at(v(5, 5, 5, 5), AssemblerLimits::strict());
        let mut p = Producer::new();
        let old = v(4, 4, 4, 4);

        let begin = p.msg(MessageType::SnapshotBegin, 1, ZERO, old, &[]);
        assert_eq!(apply(&mut a, &begin), ApplyOutcome::Superseded);
        assert_eq!(a.state(), AssemblerState::DiscardingSuperseded);
        assert_eq!(a.staged_bytes(), 0, "a drained transaction stages nothing");

        let chunk = p.msg(
            MessageType::SnapshotChunk,
            1,
            ZERO,
            old,
            &payload(SectionKind::EdgeNodes, 64),
        );
        assert_eq!(
            apply(&mut a, &chunk),
            ApplyOutcome::Superseded,
            "a following chunk must not become an orphan"
        );
        assert_eq!(a.staged_bytes(), 0, "still nothing staged");

        let commit = p.msg(MessageType::Commit, 1, ZERO, old, &[]);
        assert_eq!(apply(&mut a, &commit), ApplyOutcome::Superseded);
        assert_eq!(a.state(), AssemblerState::Idle);
        assert_eq!(
            a.current_version(),
            v(5, 5, 5, 5),
            "a superseded transaction must not change the version"
        );
    }

    #[test]
    fn a_superseded_drain_can_be_aborted_and_is_bounded() {
        let mut a = TransactionAssembler::resuming_at(v(5, 5, 5, 5), AssemblerLimits::strict());
        let mut p = Producer::new();
        let old = v(4, 4, 4, 4);
        let begin = p.msg(MessageType::SnapshotBegin, 1, ZERO, old, &[]);
        assert_eq!(apply(&mut a, &begin), ApplyOutcome::Superseded);
        let abort = p.msg(MessageType::Abort, 1, ZERO, old, &[]);
        assert_eq!(apply(&mut a, &abort), ApplyOutcome::Aborted);
        assert_eq!(a.state(), AssemblerState::Idle);

        // And the drain is bounded like any other transaction.
        let mut a = TransactionAssembler::resuming_at(
            v(5, 5, 5, 5),
            AssemblerLimits {
                max_chunks: 2,
                ..AssemblerLimits::strict()
            },
        );
        let mut p = Producer::new();
        let begin = p.msg(MessageType::SnapshotBegin, 1, ZERO, old, &[]);
        assert_eq!(apply(&mut a, &begin), ApplyOutcome::Superseded);
        let mut last = ApplyOutcome::Accepted;
        for _ in 0..4 {
            let c = p.msg(MessageType::SnapshotChunk, 1, ZERO, old, &[]);
            last = apply(&mut a, &c);
            if matches!(last, ApplyOutcome::ResyncRequired(_)) {
                break;
            }
        }
        assert!(
            matches!(
                last,
                ApplyOutcome::ResyncRequired(ResyncReason::StagingLimitExceeded { .. })
            ),
            "an endless superseded drain must be bounded, got {last:?}"
        );
    }

    #[test]
    fn a_drain_for_a_different_transaction_demands_resync() {
        let mut a = TransactionAssembler::resuming_at(v(5, 5, 5, 5), AssemblerLimits::strict());
        let mut p = Producer::new();
        let old = v(4, 4, 4, 4);
        let begin = p.msg(MessageType::SnapshotBegin, 1, ZERO, old, &[]);
        assert_eq!(apply(&mut a, &begin), ApplyOutcome::Superseded);
        let alien = p.msg(MessageType::SnapshotChunk, 777, ZERO, old, &[]);
        assert!(matches!(
            apply(&mut a, &alien),
            ApplyOutcome::ResyncRequired(ResyncReason::UnexpectedMessage { .. })
        ));
    }

    // --- Part A: content-aware replay, as permanent assertions --------------

    #[test]
    fn an_identical_message_at_a_seen_sequence_is_an_exact_duplicate() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let m = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert_eq!(apply(&mut a, &m), ApplyOutcome::Accepted);
        assert_eq!(apply(&mut a, &m), ApplyOutcome::ExactDuplicate);
        assert_eq!(apply(&mut a, &m), ApplyOutcome::ExactDuplicate);
    }

    #[test]
    fn a_differing_message_at_a_seen_sequence_is_a_conflict_not_a_duplicate() {
        // Four independent ways to differ; every one must be distinguished from
        // an exact replay. Phase 10B1 reported all four as Duplicate.
        let variants: [(&str, MessageType, u64, SnapshotVersion, &[u8]); 4] = [
            ("message type", MessageType::Abort, 1, ZERO, b""),
            ("transaction id", MessageType::Heartbeat, 999, ZERO, b""),
            (
                "version tuple",
                MessageType::Heartbeat,
                1,
                v(9, 9, 9, 9),
                b"",
            ),
            ("payload", MessageType::Heartbeat, 1, ZERO, b"different"),
        ];
        for (what, ty, txn, target, body) in variants {
            let mut a = TransactionAssembler::new(AssemblerLimits::strict());
            let mut p = Producer::new();
            let first = p.msg(MessageType::Heartbeat, 1, ZERO, ZERO, b"");
            assert_eq!(apply(&mut a, &first), ApplyOutcome::Accepted);
            p.seq = 0;
            let second = p.msg(ty, txn, ZERO, target, body);
            assert_eq!(
                apply(&mut a, &second),
                ApplyOutcome::ConflictingReplay { seq: 1 },
                "a differing {what} at a seen sequence must be a conflict"
            );
        }
    }

    #[test]
    fn a_replay_older_than_the_window_is_not_claimed_to_be_exact() {
        // The window is 16 in `strict`; push well past it.
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let first = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert_eq!(apply(&mut a, &first), ApplyOutcome::Accepted);
        for _ in 0..40 {
            let m = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
            assert_eq!(apply(&mut a, &m), ApplyOutcome::Accepted);
        }
        assert_eq!(
            apply(&mut a, &first),
            ApplyOutcome::ReplayOutsideWindow { seq: 1 },
            "an unauthenticatable replay must say so rather than claim exactness"
        );
    }

    #[test]
    fn a_wrapped_slot_collision_never_produces_a_false_match() {
        // Capacity 16: sequences 1 and 17 share a slot. A replay of 1 after 17
        // has been recorded must be reported as outside the window, never as an
        // exact duplicate of the entry that displaced it.
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let first = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert_eq!(apply(&mut a, &first), ApplyOutcome::Accepted);
        for _ in 0..16 {
            let m = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
            assert_eq!(apply(&mut a, &m), ApplyOutcome::Accepted);
        }
        assert_eq!(
            apply(&mut a, &first),
            ApplyOutcome::ReplayOutsideWindow { seq: 1 }
        );
    }

    #[test]
    fn a_zero_capacity_window_authenticates_nothing() {
        let mut a = TransactionAssembler::new(AssemblerLimits {
            replay_capacity: 0,
            ..AssemblerLimits::strict()
        });
        let mut p = Producer::new();
        let m = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert_eq!(apply(&mut a, &m), ApplyOutcome::Accepted);
        assert_eq!(
            apply(&mut a, &m),
            ApplyOutcome::ReplayOutsideWindow { seq: 1 },
            "with no retained history nothing can be authenticated as exact"
        );
        assert_eq!(a.replay_memory_bytes(), 0);
    }

    #[test]
    fn replay_memory_is_constant_with_stream_age() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let before = a.replay_memory_bytes();
        let mut p = Producer::new();
        for _ in 0..500 {
            let m = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
            assert_eq!(apply(&mut a, &m), ApplyOutcome::Accepted);
        }
        assert_eq!(
            a.replay_memory_bytes(),
            before,
            "replay history must not grow with the stream"
        );
    }

    #[test]
    fn a_resync_demanding_message_does_not_poison_replay_history() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let m1 = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert_eq!(apply(&mut a, &m1), ApplyOutcome::Accepted);

        // A gap: rejected, and must not be recorded.
        p.seq += 5;
        let gapped = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert!(matches!(
            apply(&mut a, &gapped),
            ApplyOutcome::ResyncRequired(ResyncReason::SequenceGap { .. })
        ));
        assert_eq!(a.last_seq(), Some(1), "a rejected message must not advance");

        // The rejected identity must not be retrievable as an exact duplicate.
        assert_eq!(
            apply(&mut a, &gapped),
            ApplyOutcome::ResyncRequired(ResyncReason::SequenceGap {
                expected: 2,
                got: 7
            }),
            "the rejected message was never accepted, so it is still a gap"
        );
    }

    // --- same-version transactions ------------------------------------------

    #[test]
    fn the_same_transaction_replayed_with_new_sequences_is_an_exact_duplicate() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let t = v(1, 1, 1, 0);

        let b = p.msg(MessageType::SnapshotBegin, 1, ZERO, t, &[]);
        assert_eq!(apply(&mut a, &b), ApplyOutcome::Accepted);
        let c = p.msg(MessageType::Commit, 1, ZERO, t, &[]);
        assert!(matches!(apply(&mut a, &c), ApplyOutcome::Committed(_)));
        let established = *a.current_commit_digest().expect("a commit was observed");

        // The producer replays the whole transaction. New sequence numbers, same
        // content: identity must not depend on stream position.
        let b2 = p.msg(MessageType::SnapshotBegin, 1, ZERO, t, &[]);
        assert_eq!(apply(&mut a, &b2), ApplyOutcome::Accepted);
        assert_eq!(a.state(), AssemblerState::RevalidatingReplay);
        assert_eq!(
            a.current_version(),
            t,
            "nothing may be replaced before identity is established"
        );
        let c2 = p.msg(MessageType::Commit, 1, ZERO, t, &[]);
        assert_eq!(apply(&mut a, &c2), ApplyOutcome::ExactDuplicate);
        assert_eq!(
            a.current_commit_digest(),
            Some(&established),
            "an exact replay must not disturb the committed identity"
        );
    }

    #[test]
    fn a_different_transaction_at_the_same_version_is_a_conflict() {
        // The dangerous case Phase 10B1 accepted silently: two different
        // transactions claiming one version.
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let t = v(1, 1, 1, 0);

        let b = p.msg(MessageType::SnapshotBegin, 1, ZERO, t, &[]);
        assert_eq!(apply(&mut a, &b), ApplyOutcome::Accepted);
        let c = p.msg(MessageType::Commit, 1, ZERO, t, &[]);
        assert!(matches!(apply(&mut a, &c), ApplyOutcome::Committed(_)));
        let established = *a.current_commit_digest().expect("committed");

        let b2 = p.msg(
            MessageType::SnapshotBegin,
            1,
            ZERO,
            t,
            &payload(SectionKind::EdgeNodes, 4),
        );
        assert_eq!(apply(&mut a, &b2), ApplyOutcome::Accepted);
        let c2 = p.msg(MessageType::Commit, 1, ZERO, t, &[]);
        match apply(&mut a, &c2) {
            ApplyOutcome::ResyncRequired(ResyncReason::ConflictingCommit {
                version,
                expected_digest,
                got_digest,
            }) => {
                assert_eq!(version, t);
                assert_eq!(expected_digest, established);
                assert_ne!(got_digest, established);
            }
            other => panic!("expected ConflictingCommit, got {other:?}"),
        }
        assert_eq!(a.current_version(), t, "a conflict must not change state");
        assert_eq!(a.current_commit_digest(), Some(&established));
    }

    #[test]
    fn transaction_digests_distinguish_content_order_and_count() {
        let t = v(1, 1, 1, 0);
        let commit_with = |bodies: &[Vec<u8>]| -> [u8; 32] {
            let mut a = TransactionAssembler::new(AssemblerLimits::strict());
            let mut p = Producer::new();
            let b = p.msg(MessageType::SnapshotBegin, 1, ZERO, t, &[]);
            assert_eq!(apply(&mut a, &b), ApplyOutcome::Accepted);
            for body in bodies {
                let c = p.msg(MessageType::SnapshotChunk, 1, ZERO, t, body);
                assert_eq!(apply(&mut a, &c), ApplyOutcome::Accepted);
            }
            let c = p.msg(MessageType::Commit, 1, ZERO, t, &[]);
            match apply(&mut a, &c) {
                ApplyOutcome::Committed(x) => *x.digest(),
                other => panic!("expected Committed, got {other:?}"),
            }
        };

        let a1 = payload(SectionKind::EdgeOffsets, 4);
        let a2 = payload(SectionKind::IncidenceWeights, 8);

        let base = commit_with(&[a1.clone(), a2.clone()]);
        let reordered = commit_with(&[a2.clone(), a1.clone()]);
        let fewer = commit_with(std::slice::from_ref(&a1));
        let more = commit_with(&[a1.clone(), a2.clone(), a1.clone()]);
        let empty = commit_with(&[]);

        assert_ne!(base, reordered, "chunk order must be committed");
        assert_ne!(base, fewer, "chunk count must be committed");
        assert_ne!(base, more, "chunk count must be committed");
        assert_ne!(base, empty, "an empty transaction must be distinguishable");
        assert_eq!(
            base,
            commit_with(&[a1, a2]),
            "the same content must give the same digest"
        );
    }

    // --- sequence exhaustion ------------------------------------------------

    #[test]
    fn the_sequence_boundary_is_handled_explicitly() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();

        // MAX - 1, then MAX: both legitimate.
        p.seq = u64::MAX - 2;
        let at_max_minus_one = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert_eq!(apply(&mut a, &at_max_minus_one), ApplyOutcome::Accepted);
        let at_max = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert_eq!(apply(&mut a, &at_max), ApplyOutcome::Accepted);
        assert_eq!(a.last_seq(), Some(u64::MAX));

        // The identical message is still an exact replay.
        assert_eq!(apply(&mut a, &at_max), ApplyOutcome::ExactDuplicate);

        // A different message at MAX is a conflict, not a duplicate.
        p.seq = u64::MAX - 1;
        let differing = p.msg(MessageType::Abort, 0, ZERO, ZERO, &[]);
        assert_eq!(
            apply(&mut a, &differing),
            ApplyOutcome::ConflictingReplay { seq: u64::MAX }
        );

        // Anything unauthenticatable cannot be a continuation: no successor to
        // u64::MAX exists, so the stream is exhausted.
        p.seq = 0;
        let old = p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]);
        assert_eq!(apply(&mut a, &old), ApplyOutcome::SequenceExhausted);
        assert_eq!(
            a.current_version(),
            ZERO,
            "exhaustion must not change state"
        );
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
                state: AssemblerState::Staging,
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

// ---------------------------------------------------------------------------
// Phase 10B1 CORRECTIVE, Part A: discriminating probes.
//
// These established the PRE-corrective behaviour empirically rather than
// assuming the review was right, and now serve as a live taxonomy dump: each
// prints what the CURRENT implementation returns for the ten audit cases. The
// pre-corrective transcript is preserved in the Phase 10B1 corrective report.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod part_a_probe {
    use super::tests_support::*;
    use super::*;

    fn label(o: &ApplyOutcome) -> String {
        match o {
            ApplyOutcome::Accepted => "Accepted".into(),
            ApplyOutcome::ExactDuplicate => "ExactDuplicate".into(),
            ApplyOutcome::ConflictingReplay { seq } => format!("ConflictingReplay{{seq:{seq}}}"),
            ApplyOutcome::Superseded => "Superseded".into(),
            ApplyOutcome::ReplayOutsideWindow { seq } => {
                format!("ReplayOutsideWindow{{seq:{seq}}}")
            }
            ApplyOutcome::SequenceExhausted => "SequenceExhausted".into(),
            ApplyOutcome::Aborted => "Aborted".into(),
            ApplyOutcome::Committed(_) => "Committed".into(),
            ApplyOutcome::ResyncRequired(r) => format!("ResyncRequired({r:?})"),
        }
    }

    #[test]
    #[ignore = "taxonomy dump: run with --ignored --nocapture to print all ten audit outcomes"]
    fn probe_all_ten() {
        let z = SnapshotVersion::ZERO;
        let t1 = v(1, 1, 1, 0);

        // 1..5 all reuse seq 1 after a message at seq 1 was accepted.
        let mut report: Vec<(&str, String)> = Vec::new();

        // 1. same seq, byte-identical
        {
            let mut a = TransactionAssembler::new(AssemblerLimits::strict());
            let mut p = Producer::new();
            let m = p.msg(MessageType::Heartbeat, 0, z, z, &[]);
            let _ = apply(&mut a, &m);
            report.push(("1 same seq, byte-identical", label(&apply(&mut a, &m))));
        }
        // 2. same seq, different message type
        {
            let mut a = TransactionAssembler::new(AssemblerLimits::strict());
            let mut p = Producer::new();
            let m1 = p.msg(MessageType::Heartbeat, 0, z, z, &[]);
            let _ = apply(&mut a, &m1);
            p.seq = 0;
            let m2 = p.msg(MessageType::Abort, 0, z, z, &[]);
            report.push(("2 same seq, different type", label(&apply(&mut a, &m2))));
        }
        // 3. same seq, different txn_id
        {
            let mut a = TransactionAssembler::new(AssemblerLimits::strict());
            let mut p = Producer::new();
            let m1 = p.msg(MessageType::Heartbeat, 1, z, z, &[]);
            let _ = apply(&mut a, &m1);
            p.seq = 0;
            let m2 = p.msg(MessageType::Heartbeat, 999, z, z, &[]);
            report.push(("3 same seq, different txn_id", label(&apply(&mut a, &m2))));
        }
        // 4. same seq, different version tuple
        {
            let mut a = TransactionAssembler::new(AssemblerLimits::strict());
            let mut p = Producer::new();
            let m1 = p.msg(MessageType::Heartbeat, 0, z, t1, &[]);
            let _ = apply(&mut a, &m1);
            p.seq = 0;
            let m2 = p.msg(MessageType::Heartbeat, 0, z, v(9, 9, 9, 9), &[]);
            report.push(("4 same seq, different versions", label(&apply(&mut a, &m2))));
        }
        // 5. same seq, different payload digest
        {
            let mut a = TransactionAssembler::new(AssemblerLimits::strict());
            let mut p = Producer::new();
            let m1 = p.msg(MessageType::Heartbeat, 0, z, z, b"AAAA");
            let _ = apply(&mut a, &m1);
            p.seq = 0;
            let m2 = p.msg(MessageType::Heartbeat, 0, z, z, b"BBBB");
            report.push(("5 same seq, different payload", label(&apply(&mut a, &m2))));
        }
        // 6. fresh seq, target == current, identical transaction
        {
            let mut a = TransactionAssembler::new(AssemblerLimits::strict());
            let mut p = Producer::new();
            let b = p.msg(MessageType::SnapshotBegin, 1, z, t1, &[]);
            let _ = apply(&mut a, &b);
            let c = p.msg(MessageType::Commit, 1, z, t1, &[]);
            let _ = apply(&mut a, &c);
            // replay the same transaction with fresh sequence numbers
            let b2 = p.msg(MessageType::SnapshotBegin, 1, z, t1, &[]);
            let at_begin = label(&apply(&mut a, &b2));
            let c2 = p.msg(MessageType::Commit, 1, z, t1, &[]);
            report.push(("6 same txn replayed: Begin", at_begin));
            report.push(("6 same txn replayed: Commit", label(&apply(&mut a, &c2))));
        }
        // 7. fresh seq, target == current, DIFFERENT transaction
        {
            let mut a = TransactionAssembler::new(AssemblerLimits::strict());
            let mut p = Producer::new();
            let b = p.msg(MessageType::SnapshotBegin, 1, z, t1, &[]);
            let _ = apply(&mut a, &b);
            let c = p.msg(MessageType::Commit, 1, z, t1, &[]);
            let _ = apply(&mut a, &c);
            // same target version, different txn id AND different content
            let b2 = p.msg(
                MessageType::SnapshotBegin,
                77,
                z,
                t1,
                &payload(SectionKind::EdgeNodes, 4),
            );
            let at_begin = label(&apply(&mut a, &b2));
            let c2 = p.msg(MessageType::Commit, 77, z, t1, &[]);
            report.push(("7 DIFFERENT txn, same target: Begin", at_begin));
            report.push((
                "7 DIFFERENT txn, same target: Commit",
                label(&apply(&mut a, &c2)),
            ));
        }
        // 8. target strictly dominated by current
        {
            let mut a = TransactionAssembler::resuming_at(v(5, 5, 5, 5), AssemblerLimits::strict());
            let mut p = Producer::new();
            let b = p.msg(MessageType::SnapshotBegin, 1, z, v(4, 4, 4, 4), &[]);
            let first = label(&apply(&mut a, &b));
            // The chunks of that superseded transaction still arrive.
            let ch = p.msg(MessageType::SnapshotChunk, 1, z, v(4, 4, 4, 4), &[]);
            let second = label(&apply(&mut a, &ch));
            let cm = p.msg(MessageType::Commit, 1, z, v(4, 4, 4, 4), &[]);
            let third = label(&apply(&mut a, &cm));
            report.push(("8 superseded Begin", first));
            report.push(("8 superseded following Chunk", second));
            report.push(("8 superseded following Commit", third));
        }
        // 9. replay older than any retained window
        {
            let mut a = TransactionAssembler::new(AssemblerLimits::strict());
            let mut p = Producer::new();
            let first = p.msg(MessageType::Heartbeat, 0, z, z, &[]);
            let _ = apply(&mut a, &first);
            for _ in 0..200 {
                let m = p.msg(MessageType::Heartbeat, 0, z, z, &[]);
                let _ = apply(&mut a, &m);
            }
            report.push(("9 replay 200 behind", label(&apply(&mut a, &first))));
        }
        // 10. last_seq == u64::MAX
        {
            let mut a = TransactionAssembler::new(AssemblerLimits::strict());
            let mut p = Producer::new();
            p.seq = u64::MAX - 1;
            let m = p.msg(MessageType::Heartbeat, 0, z, z, &[]); // seq = MAX
            let at_max = label(&apply(&mut a, &m));
            // Any continuation is impossible; what does it say?
            p.seq = u64::MAX - 1;
            let again = p.msg(MessageType::Heartbeat, 0, z, z, &[]); // seq = MAX, identical
            let after = label(&apply(&mut a, &again));
            // A DIFFERENT message at MAX: cannot be a continuation.
            p.seq = u64::MAX - 1;
            let differing = p.msg(MessageType::Abort, 0, z, z, &[]);
            report.push((
                "10 different message at MAX",
                label(&apply(&mut a, &differing)),
            ));
            // And one outside the window entirely.
            p.seq = 41;
            let old = p.msg(MessageType::Heartbeat, 0, z, z, &[]);
            report.push(("10 unauthenticatable at MAX", label(&apply(&mut a, &old))));
            report.push(("10 seq == u64::MAX accepted as", at_max));
            report.push(("10 continuation after MAX", after));
        }

        println!("=== OUTCOME TAXONOMY FOR THE TEN AUDIT CASES ===");
        for (name, outcome) in &report {
            println!("{name:<46} -> {outcome}");
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 10B1 corrective, Part H: model-based tests.
//
// The reference model is written independently of the production transition
// function -- it does not call `dispatch`, `begin`, `chunk` or `commit`, and it
// tracks only the properties being asserted. A model that reused the
// implementation would agree with it by construction and prove nothing.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod model {
    use super::tests_support::*;
    use super::*;
    use std::collections::HashMap;

    /// Deterministic generator. No external dependency, and the same sequence
    /// on every run and every host, so a failure is reproducible from the seed
    /// printed in the assertion.
    struct Lcg(u64);

    impl Lcg {
        const fn new(seed: u64) -> Self {
            Self(seed)
        }
        fn next(&mut self) -> u64 {
            // Numerical Recipes LCG constants; adequate for choosing among a
            // handful of branches, and not used for anything statistical.
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
        fn below(&mut self, n: u64) -> u64 {
            if n == 0 { 0 } else { self.next() % n }
        }
    }

    /// What the model tracks. Deliberately much less than the assembler: only
    /// the invariants under test.
    #[derive(Debug, Default)]
    struct Model {
        version: SnapshotVersion,
        /// Version -> the transaction digest that established it. Used to
        /// assert that one version never maps to two accepted identities.
        committed: HashMap<[u64; 4], [u8; 32]>,
        commits: usize,
    }

    fn key(v: SnapshotVersion) -> [u64; 4] {
        [v.topology, v.weights, v.features, v.params]
    }

    impl Model {
        /// The model does not predict outcomes; it records what the assembler
        /// reported and checks the properties that must hold regardless of path.
        fn observe(&mut self, outcome: &ApplyOutcome, assembler_version: SnapshotVersion) {
            match outcome {
                ApplyOutcome::Committed(t) => {
                    // A version must never map to two different identities.
                    let k = key(t.target_version());
                    if let Some(prev) = self.committed.get(&k) {
                        assert_eq!(
                            prev,
                            t.digest(),
                            "version {k:?} was committed with two different transaction digests"
                        );
                    }
                    self.committed.insert(k, *t.digest());
                    // The version must never regress.
                    assert!(
                        t.target_version().dominates(&self.version),
                        "version regressed: {:?} -> {:?}",
                        self.version,
                        t.target_version()
                    );
                    self.version = t.target_version();
                    self.commits += 1;
                }
                // Every non-commit outcome must leave the version alone.
                _ => assert_eq!(
                    assembler_version, self.version,
                    "a non-commit outcome changed the version: {outcome:?}"
                ),
            }
            assert_eq!(
                assembler_version, self.version,
                "assembler and model disagree about the current version"
            );
        }
    }

    /// One randomized run over a mixed message stream.
    fn run(seed: u64, steps: usize, limits: AssemblerLimits) {
        let mut rng = Lcg::new(seed);
        let mut a = TransactionAssembler::new(limits);
        let mut m = Model::default();
        let mut p = Producer::new();

        // Retained so the run can replay an earlier message verbatim.
        let mut history: Vec<Vec<u8>> = Vec::new();
        let mut txn: u64 = 1;
        let mut topology: u64 = 0;
        let mut open = false;

        for _ in 0..steps {
            let choice = rng.below(10);
            let bytes = match choice {
                // exact replay of an earlier message
                0 if !history.is_empty() => {
                    let i = rng.below(history.len() as u64) as usize;
                    history[i].clone()
                }
                // conflicting replay: reuse a sequence with different content
                1 if !history.is_empty() => {
                    let back = rng.below(3) + 1;
                    let s = p.seq.saturating_sub(back).max(1);
                    let saved = p.seq;
                    p.seq = s - 1;
                    let msg = p.msg(MessageType::Abort, 0xDEAD, ZERO, ZERO, b"conflict");
                    p.seq = saved;
                    msg
                }
                // begin
                2 | 3 if !open => {
                    open = true;
                    topology += 1;
                    let t = v(topology, topology, topology, 0);
                    p.msg(MessageType::SnapshotBegin, txn, ZERO, t, &[])
                }
                // superseded begin
                4 if !open && topology > 1 => {
                    open = true;
                    let t = v(topology - 1, topology - 1, topology - 1, 0);
                    p.msg(MessageType::SnapshotBegin, txn, ZERO, t, &[])
                }
                // chunk
                5 | 6 if open => {
                    let t = v(topology, topology, topology, 0);
                    p.msg(
                        MessageType::SnapshotChunk,
                        txn,
                        ZERO,
                        t,
                        &payload(SectionKind::WeightPatchRange, 2),
                    )
                }
                // commit
                7 | 8 if open => {
                    open = false;
                    let t = v(topology, topology, topology, 0);
                    let msg = p.msg(MessageType::Commit, txn, ZERO, t, &[]);
                    txn += 1;
                    msg
                }
                // abort
                9 if open => {
                    open = false;
                    let t = v(topology, topology, topology, 0);
                    let msg = p.msg(MessageType::Abort, txn, ZERO, t, &[]);
                    txn += 1;
                    msg
                }
                _ => p.msg(MessageType::Heartbeat, 0, ZERO, ZERO, &[]),
            };

            let out = apply(&mut a, &bytes);
            m.observe(&out, a.current_version());

            // Memory must stay within the configured bound whatever happens.
            assert!(
                a.replay_memory_bytes()
                    <= limits.replay_capacity * core::mem::size_of::<Option<MessageIdentity>>(),
                "replay memory exceeded its bound at seed {seed}"
            );
            assert!(
                a.staged_bytes() <= limits.max_staged_bytes,
                "staged bytes exceeded the limit at seed {seed}"
            );

            if matches!(
                out,
                ApplyOutcome::ResyncRequired(_) | ApplyOutcome::SequenceExhausted
            ) {
                a.reset_for_resync();
                p.seq = 0;
                history.clear();
                open = false;
            } else {
                history.push(bytes);
                if history.len() > 24 {
                    history.remove(0);
                }
            }
        }
    }

    #[test]
    fn randomized_streams_preserve_every_invariant() {
        for seed in 0..64u64 {
            run(seed, 200, AssemblerLimits::strict());
        }
    }

    #[test]
    fn randomized_streams_with_a_zero_capacity_window() {
        for seed in 0..16u64 {
            run(
                seed,
                120,
                AssemblerLimits {
                    replay_capacity: 0,
                    ..AssemblerLimits::strict()
                },
            );
        }
    }

    #[test]
    fn an_exact_replay_never_changes_state() {
        // Property, checked directly rather than through the random walk.
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let t = v(1, 1, 1, 0);
        let b = p.msg(MessageType::SnapshotBegin, 1, ZERO, t, &[]);
        assert_eq!(apply(&mut a, &b), ApplyOutcome::Accepted);
        let c = p.msg(MessageType::Commit, 1, ZERO, t, &[]);
        assert!(matches!(apply(&mut a, &c), ApplyOutcome::Committed(_)));

        let before = (
            a.current_version(),
            *a.current_commit_digest().expect("digest"),
        );
        for _ in 0..20 {
            assert_eq!(apply(&mut a, &c), ApplyOutcome::ExactDuplicate);
            assert_eq!(
                (
                    a.current_version(),
                    *a.current_commit_digest().expect("digest")
                ),
                before,
                "an exact replay changed state"
            );
        }
    }

    #[test]
    fn at_most_one_commit_is_emitted_per_transaction_identity() {
        let mut a = TransactionAssembler::new(AssemblerLimits::strict());
        let mut p = Producer::new();
        let t = v(1, 1, 1, 0);
        let mut commits = 0;

        for _ in 0..5 {
            let b = p.msg(MessageType::SnapshotBegin, 1, ZERO, t, &[]);
            let _ = apply(&mut a, &b);
            let c = p.msg(MessageType::Commit, 1, ZERO, t, &[]);
            if matches!(apply(&mut a, &c), ApplyOutcome::Committed(_)) {
                commits += 1;
            }
        }
        assert_eq!(
            commits, 1,
            "one transaction identity produced {commits} commits"
        );
    }

    #[test]
    fn exhaustive_short_sequences_never_panic_and_never_regress() {
        // All ordered pairs and triples over a representative alphabet, driven
        // through a real validator each time.
        let alphabet = [
            MessageType::SnapshotBegin,
            MessageType::SnapshotChunk,
            MessageType::Commit,
            MessageType::Abort,
            MessageType::Heartbeat,
            MessageType::DeltaBegin,
        ];
        let t = v(1, 1, 1, 0);
        for a1 in alphabet {
            for a2 in alphabet {
                for a3 in alphabet {
                    let mut asm = TransactionAssembler::new(AssemblerLimits::strict());
                    let mut p = Producer::new();
                    let mut version = SnapshotVersion::ZERO;
                    for ty in [a1, a2, a3] {
                        let bytes = p.msg(ty, 1, ZERO, t, &[]);
                        let out = apply(&mut asm, &bytes);
                        assert!(
                            asm.current_version().dominates(&version),
                            "version regressed on {ty:?}"
                        );
                        version = asm.current_version();
                        if matches!(
                            out,
                            ApplyOutcome::ResyncRequired(_) | ApplyOutcome::SequenceExhausted
                        ) {
                            asm.reset_for_resync();
                            p.seq = 0;
                        }
                    }
                }
            }
        }
    }
}
