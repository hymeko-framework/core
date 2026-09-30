// APPROVED-CORE-EDIT: ir-stream-wire-codec (2026-08-28, Phase 10A brief).

//! Narrowly scoped benchmark for the IR-stream validator.
//!
//! One question: **what does hostile-input validation cost per message?**
//!
//! The validator is not a transformation — it borrows the input and copies
//! nothing — so its cost should be dominated by BLAKE3 over the payload, with a
//! fixed envelope and section-table overhead on top. These benchmarks separate
//! those two terms so a future change that quietly adds a copy is visible as a
//! departure from the digest rate rather than as an unexplained slowdown.
//!
//! Additional groups measure transaction admission and content-aware replay.
//! Transport and semantic IR installation/materialisation are outside this target.
//!
//! Throughput is declared in payload bytes, so the large cases are directly
//! comparable against BLAKE3's own rate.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use hymeko::ir_stream::section::{ENTRY_BYTES, TABLE_PREFIX_BYTES};
use hymeko::ir_stream::{
    ApplyOutcome, AssemblerLimits, DecodeLimits, ElementKind, EnvelopeDraft, MessageType,
    ResyncReason, SectionEntry, SectionKind, SnapshotVersion, TransactionAssembler, encode_message,
    validate_message,
};

fn envelope(msg_type: MessageType) -> EnvelopeDraft {
    EnvelopeDraft {
        stream_id: 1,
        seq: 1,
        txn_id: 1,
        target_version: SnapshotVersion::new(1, 1, 1, 0),
        ..EnvelopeDraft::new(msg_type)
    }
}

/// An empty section table: a section-bearing message always carries one.
fn empty_table() -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&0u32.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes());
    p
}

/// A payload holding `sections` equal-sized `f32` bodies totalling `body_bytes`.
fn sectioned_payload(sections: usize, body_bytes: usize) -> Vec<u8> {
    let table_bytes = TABLE_PREFIX_BYTES + sections * ENTRY_BYTES;
    let per = (body_bytes / sections.max(1) / 4) * 4; // whole f32s
    let mut p = Vec::with_capacity(table_bytes + per * sections);

    p.extend_from_slice(
        &u32::try_from(sections)
            .expect("section count")
            .to_le_bytes(),
    );
    p.extend_from_slice(&0u32.to_le_bytes());

    let mut offset = table_bytes as u64;
    for i in 0..sections {
        // Repeatable kinds only, so many sections in one message stay legal.
        let kind = if i % 2 == 0 {
            SectionKind::WeightPatchRange
        } else {
            SectionKind::ParamPatchRange
        };
        let entry = SectionEntry {
            kind,
            element_kind: ElementKind::F32,
            element_count: (per / 4) as u64,
            byte_offset: offset,
            byte_len: per as u64,
        };
        p.extend_from_slice(&entry.to_bytes());
        offset += per as u64;
    }
    p.resize(table_bytes + per * sections, 0);
    p
}

/// Payload-size sweep with a single section: the digest-dominated regime.
fn bench_validate_by_payload_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_stream_validate_payload");
    let limits = DecodeLimits::DEFAULT;

    for &kib in &[0usize, 1, 16, 256, 4096] {
        let payload = if kib == 0 {
            Vec::new()
        } else {
            sectioned_payload(1, kib * 1024)
        };
        let msg = if kib == 0 {
            encode_message(&envelope(MessageType::Heartbeat), &payload)
        } else {
            encode_message(&envelope(MessageType::SnapshotChunk), &payload)
        };

        // Validate once outside the timed loop: a benchmark that silently
        // measures an error path would report a flattering number.
        validate_message(&msg, &limits).expect("benchmark input must be valid");

        group.throughput(Throughput::Bytes(msg.len() as u64));
        if kib >= 1024 {
            group
                .sample_size(20)
                .measurement_time(Duration::from_secs(6));
        } else {
            group
                .sample_size(100)
                .measurement_time(Duration::from_secs(4));
        }
        group.bench_with_input(BenchmarkId::from_parameter(kib), &msg, |b, m| {
            b.iter(|| {
                let v = validate_message(black_box(m), black_box(&limits)).expect("valid message");
                black_box(v.sections().len())
            });
        });
    }
    group.finish();
}

/// Section-count sweep at a fixed payload size: the table-overhead regime.
///
/// Isolates the per-entry validation cost from the digest, which is constant
/// across this sweep.
fn bench_validate_by_section_count(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_stream_validate_sections");
    let limits = DecodeLimits::DEFAULT;
    const BODY_BYTES: usize = 256 * 1024;

    for &sections in &[1usize, 8, 64, 512] {
        let payload = sectioned_payload(sections, BODY_BYTES);
        let msg = encode_message(&envelope(MessageType::SnapshotChunk), &payload);
        let v = validate_message(&msg, &limits).expect("benchmark input must be valid");
        assert_eq!(v.sections().len(), sections);

        group.throughput(Throughput::Elements(sections as u64));
        group
            .sample_size(100)
            .measurement_time(Duration::from_secs(4));
        group.bench_with_input(BenchmarkId::from_parameter(sections), &msg, |b, m| {
            b.iter(|| {
                let v = validate_message(black_box(m), black_box(&limits)).expect("valid message");
                black_box(v.sections().len())
            });
        });
    }
    group.finish();
}

/// The rejection paths, which must be *cheaper* than acceptance.
///
/// A validator whose failure path costs more than its success path is a
/// denial-of-service amplifier: an attacker sends garbage and pays less than
/// the receiver. These cases must all short-circuit before the digest.
fn bench_rejection_is_cheap(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_stream_reject");
    let limits = DecodeLimits::DEFAULT;
    group
        .sample_size(100)
        .measurement_time(Duration::from_secs(4));

    let payload = sectioned_payload(1, 256 * 1024);
    let good = encode_message(&envelope(MessageType::SnapshotChunk), &payload);

    let mut bad_magic = good.clone();
    bad_magic[0] ^= 0xFF;

    let mut oversized = good.clone();
    oversized[104..112].copy_from_slice(&u64::MAX.to_le_bytes());

    let mut corrupt = good.clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 0x01;

    for (name, msg) in [
        ("bad_magic", &bad_magic),
        ("oversized_payload", &oversized),
        ("corrupt_payload", &corrupt),
    ] {
        assert!(
            validate_message(msg, &limits).is_err(),
            "{name} must actually be rejected"
        );
        group.throughput(Throughput::Bytes(msg.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), msg, |b, m| {
            b.iter(|| black_box(validate_message(black_box(m), black_box(&limits)).is_err()));
        });
    }
    group.finish();
}

// --- Phase 10B1: the transaction assembler ---------------------------------

/// What does staging cost per transaction, and how does it scale with chunks?
///
/// The assembler copies section bodies into owned buffers, so its cost should
/// be a memcpy of the staged bytes plus a small fixed per-message cost. These
/// separate the two, so a future change that adds a per-section scan is visible
/// as a departure from the copy rate.
fn bench_assembler_transaction(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_stream_assemble");
    let limits = DecodeLimits::DEFAULT;
    let target = SnapshotVersion::new(1, 1, 1, 0);

    // A begin, N chunks and a commit, pre-encoded so the benchmark measures the
    // assembler rather than the encoder.
    let build = |chunks: usize, body_bytes: usize| -> Vec<Vec<u8>> {
        let mut seq = 0u64;
        let mut out = Vec::new();
        let mut next = |msg_type: MessageType, payload: &[u8]| -> Vec<u8> {
            seq += 1;
            let d = EnvelopeDraft {
                stream_id: 1,
                seq,
                txn_id: 1,
                target_version: target,
                ..EnvelopeDraft::new(msg_type)
            };
            encode_message(&d, payload)
        };
        let empty = {
            let mut p = Vec::new();
            p.extend_from_slice(&0u32.to_le_bytes());
            p.extend_from_slice(&0u32.to_le_bytes());
            p
        };
        out.push(next(MessageType::SnapshotBegin, &empty));
        let body = sectioned_payload(1, body_bytes);
        for _ in 0..chunks {
            out.push(next(MessageType::SnapshotChunk, &body));
        }
        out.push(next(MessageType::Commit, &[]));
        out
    };

    for &(chunks, kib) in &[(1usize, 64usize), (8, 64), (64, 64), (8, 512)] {
        let msgs = build(chunks, kib * 1024);
        let staged: usize = msgs.iter().map(std::vec::Vec::len).sum();

        group.throughput(Throughput::Bytes(staged as u64));
        group
            .sample_size(50)
            .measurement_time(Duration::from_secs(4));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{chunks}x{kib}KiB")),
            &msgs,
            |b, msgs| {
                // Construction and teardown stay INSIDE the timed region,
                // exactly as in the Phase 10B1 baseline, so before/after is
                // like-for-like. Hoisting them out excludes BOTH, which made the
                // corrective look roughly twice as fast for a reason unrelated
                // to message processing. The construction cost the corrective
                // added is reported on its own in `ir_stream_assembler_new`.
                b.iter(|| {
                    let mut a = TransactionAssembler::new(AssemblerLimits::DEFAULT);
                    let mut committed = 0usize;
                    for m in msgs {
                        let v = validate_message(black_box(m), &limits).expect("valid");
                        if let ApplyOutcome::Committed(t) = a.apply(&v) {
                            committed += t.sections().len();
                        }
                    }
                    black_box(committed)
                });
            },
        );
    }
    group.finish();
}

/// Rejection paths must stay cheap, for the same anti-amplification reason the
/// codec benchmark checks: a diverging producer must not cost the consumer more
/// than a well-behaved one.
fn bench_assembler_rejection(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_stream_assemble_reject");
    let limits = DecodeLimits::DEFAULT;
    group
        .sample_size(100)
        .measurement_time(Duration::from_secs(4));

    let empty = {
        let mut p = Vec::new();
        p.extend_from_slice(&0u32.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes());
        p
    };
    let mk = |seq: u64, msg_type: MessageType, payload: &[u8]| {
        let d = EnvelopeDraft {
            stream_id: 1,
            seq,
            txn_id: 1,
            target_version: SnapshotVersion::new(1, 1, 1, 0),
            ..EnvelopeDraft::new(msg_type)
        };
        encode_message(&d, payload)
    };

    let first = mk(1, MessageType::SnapshotBegin, &empty);
    let gap = mk(99, MessageType::SnapshotChunk, &empty);
    let orphan = mk(2, MessageType::Commit, &[]);

    group.bench_function("sequence_gap", |b| {
        b.iter(|| {
            let mut a = TransactionAssembler::new(AssemblerLimits::DEFAULT);
            let v1 = validate_message(&first, &limits).expect("valid");
            let _ = a.apply(&v1);
            let v2 = validate_message(black_box(&gap), &limits).expect("valid");
            black_box(matches!(a.apply(&v2), ApplyOutcome::ResyncRequired(_)))
        });
    });

    group.bench_function("orphan_commit", |b| {
        b.iter(|| {
            let mut a = TransactionAssembler::new(AssemblerLimits::DEFAULT);
            let v = validate_message(black_box(&orphan), &limits).expect("valid");
            black_box(matches!(a.apply(&v), ApplyOutcome::ResyncRequired(_)))
        });
    });

    group.finish();
}

/// The content-aware replay paths, added by the Phase 10B1 corrective.
///
/// Each must be O(1) and cheap: the whole point of a bounded window is that
/// authenticating a replay costs a modulo and a comparison, not a scan. A
/// conflicting replay must not cost more than an exact one, or a diverging
/// producer becomes a denial-of-service lever.
fn bench_replay_paths(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_stream_replay");
    let limits = DecodeLimits::DEFAULT;
    group
        .sample_size(100)
        .measurement_time(Duration::from_secs(3));

    let empty = empty_table();
    let mk = |seq: u64, ty: MessageType, txn: u64, payload: &[u8]| {
        let d = EnvelopeDraft {
            stream_id: 1,
            seq,
            txn_id: txn,
            target_version: SnapshotVersion::new(1, 1, 1, 0),
            ..EnvelopeDraft::new(ty)
        };
        encode_message(&d, payload)
    };

    // A warmed assembler holding a window of accepted heartbeats.
    let warm = |cap: usize| {
        let mut a = TransactionAssembler::new(AssemblerLimits {
            replay_capacity: cap,
            ..AssemblerLimits::DEFAULT
        });
        let mut msgs = Vec::new();
        for seq in 1..=64u64 {
            let m = mk(seq, MessageType::Heartbeat, 0, &[]);
            let v = validate_message(&m, &limits).expect("valid");
            let _ = a.apply(&v);
            msgs.push(m);
        }
        (a, msgs)
    };

    // Exact duplicate: inside the window, identical.
    {
        let (mut a, msgs) = warm(1024);
        let target = msgs[32].clone();
        assert_eq!(
            a.apply(&validate_message(&target, &limits).expect("valid")),
            ApplyOutcome::ExactDuplicate
        );
        group.bench_function("exact_duplicate", |b| {
            b.iter(|| {
                let v = validate_message(black_box(&target), &limits).expect("valid");
                black_box(matches!(a.apply(&v), ApplyOutcome::ExactDuplicate))
            });
        });
    }

    // Conflicting duplicate: inside the window, different content.
    {
        let (mut a, _msgs) = warm(1024);
        let conflicting = mk(32, MessageType::Abort, 0xDEAD, &[]);
        assert!(matches!(
            a.apply(&validate_message(&conflicting, &limits).expect("valid")),
            ApplyOutcome::ConflictingReplay { .. }
        ));
        group.bench_function("conflicting_duplicate", |b| {
            b.iter(|| {
                let v = validate_message(black_box(&conflicting), &limits).expect("valid");
                black_box(matches!(
                    a.apply(&v),
                    ApplyOutcome::ConflictingReplay { .. }
                ))
            });
        });
    }

    // Window miss: older than the retained capacity.
    {
        let (mut a, msgs) = warm(8);
        let old = msgs[0].clone();
        assert!(matches!(
            a.apply(&validate_message(&old, &limits).expect("valid")),
            ApplyOutcome::ReplayOutsideWindow { .. }
        ));
        group.bench_function("replay_window_miss", |b| {
            b.iter(|| {
                let v = validate_message(black_box(&old), &limits).expect("valid");
                black_box(matches!(
                    a.apply(&v),
                    ApplyOutcome::ReplayOutsideWindow { .. }
                ))
            });
        });
    }

    // Superseded drain: a whole transaction consumed without staging.
    {
        let body = sectioned_payload(1, 256 * 1024);
        let mut seq = 0u64;
        let mut next = |ty: MessageType, payload: &[u8]| {
            seq += 1;
            let d = EnvelopeDraft {
                stream_id: 1,
                seq,
                txn_id: 9,
                target_version: SnapshotVersion::new(1, 1, 1, 0),
                ..EnvelopeDraft::new(ty)
            };
            encode_message(&d, payload)
        };
        let drain: Vec<Vec<u8>> = vec![
            next(MessageType::SnapshotBegin, &empty),
            next(MessageType::SnapshotChunk, &body),
            next(MessageType::Commit, &[]),
        ];
        let staged: usize = drain.iter().map(std::vec::Vec::len).sum();
        group.throughput(Throughput::Bytes(staged as u64));
        group.bench_function("superseded_drain", |b| {
            b.iter(|| {
                // Resuming past the target makes the whole transaction
                // superseded, so it is drained rather than staged.
                let mut a = TransactionAssembler::resuming_at(
                    SnapshotVersion::new(9, 9, 9, 9),
                    AssemblerLimits::DEFAULT,
                );
                let mut n = 0usize;
                for m in &drain {
                    let v = validate_message(black_box(m), &limits).expect("valid");
                    if matches!(a.apply(&v), ApplyOutcome::Superseded) {
                        n += 1;
                    }
                }
                black_box(n)
            });
        });
    }

    group.finish();
}

/// Replay-window capacity must not affect steady-state cost.
///
/// Lookup is `seq % capacity` plus one comparison, so the only thing capacity
/// should change is the allocation performed once at construction. If this
/// group slopes with capacity, the lookup is not O(1).
/// What an assembler costs to CREATE.
///
/// Reported separately because the corrective made it non-trivial: the replay
/// window is allocated and zeroed at construction. This is a per-STREAM cost,
/// paid once before any message, and it is measured here so it cannot hide
/// inside a per-message number.
fn bench_assembler_construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_stream_assembler_new");
    group
        .sample_size(100)
        .measurement_time(Duration::from_secs(3));
    for &cap in &[0usize, 16, 256, 1024, 65536] {
        group.bench_with_input(BenchmarkId::from_parameter(cap), &cap, |b, &cap| {
            b.iter(|| {
                black_box(TransactionAssembler::new(AssemblerLimits {
                    replay_capacity: black_box(cap),
                    ..AssemblerLimits::DEFAULT
                }))
            });
        });
    }
    group.finish();
}

fn bench_replay_capacity_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_stream_replay_capacity");
    let limits = DecodeLimits::DEFAULT;
    group
        .sample_size(100)
        .measurement_time(Duration::from_secs(3));

    for &cap in &[16usize, 256, 4096, 65536] {
        let mut a = TransactionAssembler::new(AssemblerLimits {
            replay_capacity: cap,
            ..AssemblerLimits::DEFAULT
        });
        let mut msgs = Vec::new();
        for seq in 1..=8u64 {
            let d = EnvelopeDraft {
                stream_id: 1,
                seq,
                ..EnvelopeDraft::new(MessageType::Heartbeat)
            };
            let m = encode_message(&d, &[]);
            let v = validate_message(&m, &limits).expect("valid");
            let _ = a.apply(&v);
            msgs.push(m);
        }
        let target = msgs[4].clone();
        assert_eq!(
            a.apply(&validate_message(&target, &limits).expect("valid")),
            ApplyOutcome::ExactDuplicate
        );
        group.bench_with_input(BenchmarkId::from_parameter(cap), &target, |b, t| {
            b.iter(|| {
                let v = validate_message(black_box(t), &limits).expect("valid");
                black_box(matches!(a.apply(&v), ApplyOutcome::ExactDuplicate))
            });
        });
    }
    group.finish();
}

/// Codec plus admission for the distributed resume/monotonicity contract.
/// Empty section tables are framing, not complete semantic snapshots. Each
/// iteration gets a fresh assembler outside the timed closure; its staged
/// allocation/commit work is included, construction is measured in its own group.
fn bench_distributed_admission(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_stream_admission");
    group
        .sample_size(50)
        .measurement_time(Duration::from_secs(3));
    let zero = SnapshotVersion::ZERO;
    let current = SnapshotVersion::new(2, 2, 2, 0);
    let next = SnapshotVersion::new(3, 3, 3, 0);
    let incomparable = SnapshotVersion::new(1, 3, 3, 0);
    let snapshot = MessageType::SnapshotBegin;
    for (name, resumed, base, target, begin, expected) in [
        ("fresh_zero", None, zero, zero, snapshot, 'C'),
        ("resumed_zero", Some(zero), zero, zero, snapshot, 'U'),
        ("resumed_equal", Some(current), zero, current, snapshot, 'U'),
        (
            "dominating_snapshot",
            Some(current),
            zero,
            next,
            snapshot,
            'C',
        ),
        (
            "incomparable_snapshot",
            Some(current),
            zero,
            incomparable,
            snapshot,
            'I',
        ),
        (
            "stale_delta",
            Some(current),
            zero,
            next,
            MessageType::DeltaBegin,
            'B',
        ),
    ] {
        let frames: Vec<_> = [begin, MessageType::Commit]
            .into_iter()
            .enumerate()
            .map(|(i, kind)| {
                let draft = EnvelopeDraft {
                    stream_id: 17,
                    seq: i as u64 + 1,
                    txn_id: 23,
                    base_version: base,
                    target_version: target,
                    ..EnvelopeDraft::new(kind)
                };
                encode_message(
                    &draft,
                    if kind.carries_sections() {
                        &[0; 8]
                    } else {
                        &[]
                    },
                )
            })
            .collect();
        let make = || {
            resumed.map_or_else(
                || TransactionAssembler::new(AssemblerLimits::strict()),
                |v| TransactionAssembler::resuming_at(v, AssemblerLimits::strict()),
            )
        };
        let apply = |assembler: &mut TransactionAssembler| {
            let mut outcome = ApplyOutcome::Accepted;
            for frame in &frames {
                let message = validate_message(black_box(frame), &DecodeLimits::DEFAULT)
                    .expect("valid frame");
                outcome = assembler.apply(&message);
                if !matches!(outcome, ApplyOutcome::Accepted) {
                    break;
                }
            }
            outcome
        };
        let mut probe = make();
        let verdict = match apply(&mut probe) {
            ApplyOutcome::Committed(_) => 'C',
            ApplyOutcome::ReplayOutsideWindow { .. } => 'U',
            ApplyOutcome::ResyncRequired(ResyncReason::IncoherentVersionBump { .. }) => 'I',
            ApplyOutcome::ResyncRequired(ResyncReason::StaleBase { .. }) => 'B',
            other => panic!("{name}: unexpected preflight {other:?}"),
        };
        assert_eq!(verdict, expected, "{name}");
        assert_eq!(
            probe.current_version(),
            if expected == 'C' {
                target
            } else {
                resumed.unwrap_or(zero)
            }
        );
        assert_eq!(probe.current_commit_digest().is_some(), expected == 'C');
        group.bench_function(name, |b| {
            b.iter_batched(
                make,
                |mut assembler| black_box(apply(&mut assembler)),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_validate_by_payload_size,
    bench_validate_by_section_count,
    bench_rejection_is_cheap,
    bench_assembler_transaction,
    bench_assembler_rejection,
    bench_replay_paths,
    bench_replay_capacity_scaling,
    bench_assembler_construction,
    bench_distributed_admission
);
criterion_main!(benches);
