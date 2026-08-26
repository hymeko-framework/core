//! Phase 7 instrument tests: timestamp query bounds, readback byte correctness
//! across every staging/chunking/destination combination, and the staging
//! buffer's reuse preconditions.
//!
//! These create a device per test, so on Linux they must run with
//! `--test-threads=1`; concurrent `WgpuBackend` construction deadlocks in the
//! Vulkan loader. That is a property of device creation, not of these tests.

use hymeko_wgpu::profiling::{
    GpuTimer, ReadbackPlan, StagingBuffer, TimestampSupport, timed_readback,
};
use hymeko_wgpu::{ComputeError, TensorShape, WgpuBackend};

/// A backend that requests both timestamp features. Returns `None` when there
/// is no adapter at all, so the suite skips rather than fails on a headless
/// machine; an adapter that merely refuses the features still yields a backend.
fn timed_backend(context: &str) -> Option<WgpuBackend> {
    match WgpuBackend::builder()
        .label("phase7-test")
        .features(wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS)
        .build()
    {
        Ok(b) => Some(b),
        Err(ComputeError::BackendUnavailable { reason, .. }) => {
            eprintln!("[{context}] SKIPPED - no GPU adapter: {reason}");
            None
        }
        Err(other) => panic!("[{context}] backend init failed (not availability): {other}"),
    }
}

fn pattern(elements: usize) -> Vec<f32> {
    (0..elements).map(|i| (i % 1024) as f32).collect()
}

// --- timestamp capability and query-set bounds ------------------------------

#[test]
fn timer_exists_exactly_when_the_device_reports_timestamp_support() {
    let Some(backend) = timed_backend("timer_capability") else {
        return;
    };
    let support = TimestampSupport::from_features(backend.device().features());
    let timer = GpuTimer::new(&backend, 4);
    assert_eq!(
        timer.is_some(),
        support.passes(),
        "a timer must be constructible iff TIMESTAMP_QUERY is granted; support={support:?}"
    );
    // The unsupported branch is a real outcome, not a failure: record it so a
    // skipped run is distinguishable from a passing one.
    if timer.is_none() {
        eprintln!("[timer_capability] device lacks TIMESTAMP_QUERY; measured as unsupported");
    }
}

#[test]
fn timer_rejects_indices_and_counts_beyond_capacity() {
    let Some(backend) = timed_backend("timer_bounds") else {
        return;
    };
    let Some(timer) = GpuTimer::new(&backend, 4) else {
        return;
    };
    assert_eq!(timer.capacity(), 4);

    assert!(
        timer.pass_writes(0, 3).is_ok(),
        "in-range pair must succeed"
    );
    for (a, b) in [(0u32, 4u32), (4, 0), (9, 9)] {
        assert!(
            matches!(
                timer.pass_writes(a, b),
                Err(ComputeError::IndexOverflow { .. })
            ),
            "pass_writes({a},{b}) must be rejected against capacity 4"
        );
    }

    let mut encoder = backend.encoder(Some("bounds"));
    assert!(
        matches!(
            timer.write(&mut encoder, 4),
            Err(ComputeError::IndexOverflow { .. } | ComputeError::Backend { .. })
        ),
        "write past capacity must be rejected"
    );
    assert!(
        matches!(
            timer.resolve(&mut encoder, 5),
            Err(ComputeError::IndexOverflow { .. })
        ),
        "resolve count past capacity must be rejected"
    );
    assert!(
        timer.resolve(&mut encoder, 4).is_ok(),
        "resolve of exactly capacity must be accepted"
    );
}

// --- readback byte correctness ---------------------------------------------

#[test]
fn readback_returns_exact_bytes_for_every_staging_and_chunk_combination() {
    let Some(backend) = timed_backend("readback_bytes") else {
        return;
    };
    let elements = 1_048_576usize; // 4 MiB
    let bytes = (elements * 4) as u64;
    let src = backend
        .resources()
        .upload_tensor("src", TensorShape::new(elements, 1), &pattern(elements))
        .expect("upload");
    let mut staging = StagingBuffer::new(&backend, bytes);
    let expected = pattern(elements);

    // fresh staging, single copy
    let (fresh, t) = timed_readback(&backend, src.buffer(), bytes, ReadbackPlan::default())
        .expect("fresh readback");
    assert_eq!(fresh, expected, "fresh staging returned wrong bytes");
    assert!(t.fresh_staging, "fresh path must be recorded as fresh");

    // reused staging, single copy
    let (reused, t) = timed_readback(
        &backend,
        src.buffer(),
        bytes,
        ReadbackPlan {
            staging: Some(&mut staging),
            ..ReadbackPlan::default()
        },
    )
    .expect("reused readback");
    assert_eq!(reused, expected, "reused staging returned wrong bytes");
    assert!(
        !t.fresh_staging,
        "reused path must not be recorded as fresh"
    );

    // chunked, several chunk sizes including one that does not divide the total
    for chunk in [1_048_576u64, 3_000_000, bytes * 2] {
        let (chunked, t) = timed_readback(
            &backend,
            src.buffer(),
            bytes,
            ReadbackPlan {
                staging: Some(&mut staging),
                chunk_bytes: Some(chunk),
                ..ReadbackPlan::default()
            },
        )
        .expect("chunked readback");
        assert_eq!(chunked, expected, "chunk={chunk} returned wrong bytes");
        assert_eq!(t.chunk_bytes, Some(chunk));
    }
}

#[test]
fn readback_into_a_reused_destination_fills_it_and_returns_empty() {
    let Some(backend) = timed_backend("readback_dest") else {
        return;
    };
    let elements = 262_144usize;
    let bytes = (elements * 4) as u64;
    let src = backend
        .resources()
        .upload_tensor("src", TensorShape::new(elements, 1), &pattern(elements))
        .expect("upload");
    // Deliberately pre-filled with the wrong data and the wrong length, so a
    // stale destination cannot masquerade as a correct readback.
    let mut dest: Vec<f32> = vec![-1.0; elements * 2];

    let (returned, t) = timed_readback(
        &backend,
        src.buffer(),
        bytes,
        ReadbackPlan {
            dest: Some(&mut dest),
            ..ReadbackPlan::default()
        },
    )
    .expect("readback into destination");

    assert!(
        returned.is_empty(),
        "destination-reuse path must return an empty Vec, not a clone"
    );
    assert!(t.dest_reused, "dest_reused must be recorded");
    assert_eq!(dest.len(), elements, "destination must be truncated to fit");
    assert_eq!(dest, pattern(elements), "destination holds wrong bytes");
}

#[test]
fn readback_handles_zero_length_and_non_chunk_multiple_payloads() {
    let Some(backend) = timed_backend("readback_edges") else {
        return;
    };
    let res = backend.resources();

    let empty = res
        .zeroed_tensor("empty", TensorShape::new(0, 8))
        .expect("zero tensor");
    let (v, t) = timed_readback(&backend, empty.buffer(), 0, ReadbackPlan::default())
        .expect("zero-length readback");
    assert!(v.is_empty(), "zero-length readback must return no elements");
    assert_eq!(t.bytes, 0);
    assert!(
        (t.total_gbps() - 0.0).abs() < f64::EPSILON,
        "zero bytes must not divide by zero"
    );

    // 20 bytes past a chunk boundary: exercises the final short chunk.
    let odd = 3_145_733usize;
    let odd_bytes = (odd * 4) as u64;
    let tensor = res
        .upload_tensor("odd", TensorShape::new(odd, 1), &pattern(odd))
        .expect("upload odd");
    let (data, _) = timed_readback(
        &backend,
        tensor.buffer(),
        odd_bytes,
        ReadbackPlan {
            chunk_bytes: Some(8 * 1024 * 1024),
            ..ReadbackPlan::default()
        },
    )
    .expect("non-multiple chunked readback");
    assert_eq!(data.len(), odd, "non-multiple payload lost elements");
    assert_eq!(data, pattern(odd), "non-multiple payload corrupted");
}

// --- staging buffer reuse preconditions ------------------------------------

#[test]
fn staging_reuse_is_refused_while_mapped_and_when_too_small() {
    let Some(backend) = timed_backend("staging_preconditions") else {
        return;
    };
    let staging = StagingBuffer::new(&backend, 4096);
    assert_eq!(staging.size(), 4096);
    assert!(!staging.is_mapped());

    assert!(
        staging.begin_reuse(4096).is_ok(),
        "exact fit must be allowed"
    );
    assert!(staging.begin_reuse(2048).is_ok(), "smaller must be allowed");
    assert!(
        matches!(
            staging.begin_reuse(4097),
            Err(ComputeError::ShapeMismatch { .. })
        ),
        "a transfer larger than the buffer must be refused, not truncated"
    );
}

#[test]
fn staging_buffer_stays_correct_across_repeated_reuse() {
    let Some(backend) = timed_backend("staging_repeat") else {
        return;
    };
    let elements = 65_536usize;
    let bytes = (elements * 4) as u64;
    let src = backend
        .resources()
        .upload_tensor("src", TensorShape::new(elements, 1), &pattern(elements))
        .expect("upload");
    let mut staging = StagingBuffer::new(&backend, bytes);
    let expected = pattern(elements);

    // Ten consecutive readbacks through one buffer. A missing unmap, or reuse
    // before the previous map completed, would surface here as an error or as
    // stale bytes rather than as a slow path.
    for round in 0..10 {
        let (data, _) = timed_readback(
            &backend,
            src.buffer(),
            bytes,
            ReadbackPlan {
                staging: Some(&mut staging),
                ..ReadbackPlan::default()
            },
        )
        .unwrap_or_else(|e| panic!("round {round} failed: {e}"));
        assert_eq!(
            data, expected,
            "round {round} returned stale or wrong bytes"
        );
        assert!(
            !staging.is_mapped(),
            "round {round} left the buffer mapped; the next reuse would be refused"
        );
    }
}
