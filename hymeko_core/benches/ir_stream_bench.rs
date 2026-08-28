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
//! Deliberately not measured here: transport, transaction application,
//! materialisation. None of them exist yet.
//!
//! Throughput is declared in payload bytes, so the large cases are directly
//! comparable against BLAKE3's own rate.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use hymeko::ir_stream::envelope::{HEADER_BYTES, SCHEMA_VERSION};
use hymeko::ir_stream::section::{ENTRY_BYTES, TABLE_PREFIX_BYTES};
use hymeko::ir_stream::{
    DecodeLimits, ElementKind, MessageType, SectionEntry, SectionKind, SnapshotVersion,
    ValidatedEnvelope, encode_message, validate_message,
};

fn envelope(msg_type: MessageType) -> ValidatedEnvelope {
    ValidatedEnvelope {
        schema_version: SCHEMA_VERSION,
        header_bytes: HEADER_BYTES,
        msg_type,
        flags: 0,
        stream_id: 1,
        seq: 1,
        txn_id: 1,
        base_version: SnapshotVersion::ZERO,
        target_version: SnapshotVersion::new(1, 1, 1, 0),
        payload_len: 0,
        payload_hash: [0; 32],
    }
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

criterion_group!(
    benches,
    bench_validate_by_payload_size,
    bench_validate_by_section_count,
    bench_rejection_is_cheap
);
criterion_main!(benches);
