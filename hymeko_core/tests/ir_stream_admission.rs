//! Public-API admission regressions, also runnable with rustc --test against Core.
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

#[test]
fn resync_does_not_turn_resumed_zero_into_bootstrap() {
    let mut assembler = TransactionAssembler::resuming_at(ZERO, AssemblerLimits::strict());
    assembler.reset_for_resync();
    Producer::rejected(
        &mut assembler,
        MessageType::SnapshotBegin,
        ZERO,
        ZERO,
        ApplyOutcome::ReplayOutsideWindow { seq: 2 },
    );
}

#[test]
fn known_identity_survives_resync_and_replay() {
    let mut assembler = TransactionAssembler::new(AssemblerLimits::strict());
    Producer::committed(&mut assembler, ZERO);
    assembler.reset_for_resync();
    Producer::rejected(
        &mut assembler,
        MessageType::SnapshotBegin,
        ZERO,
        ZERO,
        ApplyOutcome::ExactDuplicate,
    );
}

/// The expected verdicts come from the independently executable Lean model,
/// not a second Rust implementation of the admission guards. The same ordered
/// levels are exercised at ordinary values and at the u64 boundary.
#[test]
fn admission_matches_lean_vectors_and_preserves_committed_state() {
    const VECTORS: &str = include_str!("ir_stream_admission_vectors.tsv");
    for levels in [[0, 1, 2], [0, 1, u64::MAX]] {
        let version = |mut index: usize| {
            let mut axis = [0; 4];
            for value in &mut axis {
                *value = levels[index % 3];
                index /= 3;
            }
            SnapshotVersion::new(axis[0], axis[1], axis[2], axis[3])
        };
        let mut count = 0;
        let mut verdict_counts = [0usize; 5];
        for line in VECTORS.lines().filter(|line| !line.starts_with('#')) {
            let fields: Vec<_> = line.split_ascii_whitespace().collect();
            assert_eq!(fields.len(), 7, "malformed oracle row: {line}");
            let current = version(fields[1].parse().unwrap());
            let target = version(fields[2].parse().unwrap());
            let begin = match fields[3] {
                "S" => MessageType::SnapshotBegin,
                "D" => MessageType::DeltaBegin,
                other => panic!("unknown oracle transaction kind {other}"),
            };
            let base = if fields[4] == "1" { current } else { ZERO };
            let mut assembler = match fields[0] {
                "F" => TransactionAssembler::new(AssemblerLimits::strict()),
                "R" => TransactionAssembler::resuming_at(current, AssemblerLimits::strict()),
                other => panic!("unknown oracle state {other}"),
            };
            let outcome = Producer { seq: 0 }.transaction(&mut assembler, begin, base, target);
            let (code, slot) = match &outcome {
                ApplyOutcome::Committed(committed) => {
                    assert_eq!(committed.target_version(), target);
                    assert_eq!(assembler.current_version(), target);
                    assert_eq!(assembler.current_commit_digest(), Some(committed.digest()));
                    assert!(target.dominates(&current));
                    ("C", 0)
                }
                ApplyOutcome::Superseded => ("S", 1),
                ApplyOutcome::ReplayOutsideWindow { seq: 2 } => ("U", 2),
                ApplyOutcome::ResyncRequired(ResyncReason::StaleBase { have, wanted }) => {
                    assert_eq!((*have, *wanted), (current, base));
                    ("B", 3)
                }
                ApplyOutcome::ResyncRequired(ResyncReason::IncoherentVersionBump {
                    base: rejected_base,
                    target: rejected_target,
                }) => {
                    assert_eq!(*rejected_base, version(fields[6].parse().unwrap()));
                    assert_eq!(*rejected_target, target);
                    ("I", 4)
                }
                other => panic!("unexpected admission {other:?} for {line}"),
            };
            assert_eq!(code, fields[5], "levels={levels:?}, row={line}");
            if code != "C" {
                assert_eq!(assembler.current_version(), current, "{line}");
                assert_eq!(assembler.current_commit_digest(), None, "{line}");
            }
            verdict_counts[slot] += 1;
            count += 1;
        }
        assert_eq!(count, 26_406);
        assert_eq!(verdict_counts, [2525, 4860, 214, 1215, 17_592]);
    }
}
