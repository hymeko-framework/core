//! Public-API admission requirements; compile with rustc --test against Core.
//! Empty section tables exercise version/identity admission, not IR installation.
use hymeko::ir_stream::{
    ApplyOutcome, AssemblerLimits, DecodeLimits, EnvelopeDraft, MessageType, ResyncReason,
    SnapshotVersion, TransactionAssembler, encode_message, validate_message,
};

struct Producer {
    seq: u64,
}

impl Producer {
    fn send(
        &mut self,
        assembler: &mut TransactionAssembler,
        kind: MessageType,
        base: SnapshotVersion,
        target: SnapshotVersion,
    ) -> ApplyOutcome {
        self.seq += 1;
        let draft = EnvelopeDraft {
            stream_id: 17,
            seq: self.seq,
            txn_id: 23,
            base_version: base,
            target_version: target,
            ..EnvelopeDraft::new(kind)
        };
        let body: &[u8] = if kind.carries_sections() {
            &[0; 8]
        } else {
            &[]
        };
        let bytes = encode_message(&draft, body);
        let message = validate_message(&bytes, &DecodeLimits::DEFAULT)
            .expect("counterexample must pass the real wire validator");
        assembler.apply(&message)
    }

    fn transaction(
        &mut self,
        assembler: &mut TransactionAssembler,
        begin: MessageType,
        base: SnapshotVersion,
        target: SnapshotVersion,
    ) -> ApplyOutcome {
        let outcome = self.send(assembler, begin, base, target);
        if !matches!(outcome, ApplyOutcome::Accepted) {
            return outcome;
        }
        self.send(assembler, MessageType::Commit, base, target)
    }

    fn rejected(
        assembler: &mut TransactionAssembler,
        begin: MessageType,
        base: SnapshotVersion,
        target: SnapshotVersion,
        expected: ApplyOutcome,
    ) {
        let version = assembler.current_version();
        let digest = assembler.current_commit_digest().copied();
        let actual = Self { seq: 0 }.transaction(assembler, begin, base, target);
        assert_eq!(actual, expected);
        assert_eq!(assembler.current_version(), version);
        assert_eq!(assembler.current_commit_digest().copied(), digest);
    }

    fn committed(assembler: &mut TransactionAssembler, target: SnapshotVersion) {
        let outcome = Self { seq: 0 }.transaction(
            assembler,
            MessageType::SnapshotBegin,
            SnapshotVersion::ZERO,
            target,
        );
        assert!(matches!(outcome, ApplyOutcome::Committed(_)), "{outcome:?}");
        assert_eq!(assembler.current_version(), target);
        assert!(assembler.current_commit_digest().is_some());
    }

    fn unknown_replay(version: SnapshotVersion) {
        let mut assembler = TransactionAssembler::resuming_at(version, AssemblerLimits::strict());
        Self::rejected(
            &mut assembler,
            MessageType::SnapshotBegin,
            ZERO,
            version,
            ApplyOutcome::ReplayOutsideWindow { seq: 2 },
        );
    }

    fn incomparable(begin: MessageType, base: SnapshotVersion, target: SnapshotVersion) {
        let mut assembler = TransactionAssembler::resuming_at(V, AssemblerLimits::strict());
        Self::rejected(
            &mut assembler,
            begin,
            base,
            target,
            ApplyOutcome::ResyncRequired(ResyncReason::IncoherentVersionBump { base: V, target }),
        );
    }
}

const V: SnapshotVersion = SnapshotVersion::new(2, 2, 2, 0);
const ZERO: SnapshotVersion = SnapshotVersion::ZERO;

#[test]
fn resume_equal_version_without_identity_requires_resync() {
    Producer::unknown_replay(V);
}

#[test]
fn incomparable_snapshot_cannot_regress_topology() {
    Producer::incomparable(
        MessageType::SnapshotBegin,
        ZERO,
        SnapshotVersion::new(1, 3, 3, 0),
    );
}

#[test]
fn incomparable_delta_cannot_regress_weights() {
    Producer::incomparable(MessageType::DeltaBegin, V, SnapshotVersion::new(2, 1, 3, 0));
}

#[test]
fn resumed_zero_is_distinct_from_empty_bootstrap() {
    Producer::unknown_replay(ZERO);
}

#[test]
fn empty_bootstrap_can_install_zero_version() {
    let mut a = TransactionAssembler::new(AssemblerLimits::strict());
    Producer::committed(&mut a, ZERO);
}

#[test]
fn dominating_snapshot_advances_a_resumed_replica() {
    let mut a = TransactionAssembler::resuming_at(V, AssemblerLimits::strict());
    Producer::committed(&mut a, SnapshotVersion::new(3, 3, 3, 0));
}

#[test]
fn stale_delta_base_preserves_the_current_version() {
    let mut a = TransactionAssembler::resuming_at(V, AssemblerLimits::strict());
    let target = SnapshotVersion::new(3, 3, 3, 0);
    Producer::rejected(
        &mut a,
        MessageType::DeltaBegin,
        ZERO,
        target,
        ApplyOutcome::ResyncRequired(ResyncReason::StaleBase {
            have: V,
            wanted: ZERO,
        }),
    );
}
