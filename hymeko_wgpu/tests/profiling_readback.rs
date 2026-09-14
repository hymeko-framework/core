//! Phase 7 instrument tests: timestamp query bounds, readback byte correctness
//! across every staging/chunking/destination combination, and the staging
//! buffer's reuse preconditions.
//! These exercise live profiling instrumentation and require explicit
//! selection with `-- --ignored --exact`; ordinary GPU correctness tests live
//! in `readback_workspace.rs` and the operation-specific suites.
//!
//! # One configured device for the whole suite
//!
//! An earlier version of this header said these tests "create a device per test, so on Linux they
//! must run with `--test-threads=1`". The first half was true and the conclusion was the wrong fix:
//! the suite does not need a *fresh* device per test, it needs a *configured* one — and all eight
//! tests ask for exactly the same configuration, `TIMESTAMP_QUERY | TIMESTAMP_QUERY_INSIDE_ENCODERS`.
//!
//! So the suite now holds one `BackendCell` and every test shares the device it builds. What stays
//! per test is everything that carries the evidence: each test makes its own query set, staging
//! storage, encoder and readback destination. Only the *device* is shared, and it is immutable
//! through a `&'static` reference.
//!
//! The feature request is unchanged, including its intersection behaviour: an adapter that refuses
//! the timestamp features still yields a backend, and `TimestampSupport` reports what it actually
//! got. `one_timestamp_backend_is_built_for_the_whole_suite` measures the construction count rather
//! than asserting it.

use hymeko_wgpu::profiling::{GpuTimer, ReadbackPlan, TimestampSupport, timed_readback};
use hymeko_wgpu::{ComputeError, StagingStorage, TensorShape, WgpuBackend};

/// The suite's single timestamp-enabled device.
///
/// The skip/failure classification is NOT re-implemented here — `BackendCell::get_or_init` owns it,
/// the same one `shared_backend` uses. This function supplies only what is specific to this suite:
/// the label and the feature request.
static TIMED: hymeko_wgpu::testing::BackendCell = hymeko_wgpu::testing::BackendCell::new();

fn timed_backend(context: &str) -> Option<&'static WgpuBackend> {
    TIMED.get_or_init(context, || {
        WgpuBackend::builder()
            .label("phase7-test")
            .features(
                wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
            )
            .build()
    })
}

/// The construction receipt: eight tests, **one** timestamp-enabled device.
///
/// This measures rather than asserts. It is also discriminating against the previous
/// implementation: that one built the device directly in `timed_backend`, never through a
/// `BackendCell`, so this counter would read 0 and the equality below would fail.
#[test]
fn one_timestamp_backend_is_built_for_the_whole_suite() {
    let Some(_) = timed_backend("receipt") else {
        eprintln!("SKIPPED - no GPU adapter");
        return;
    };
    assert_eq!(
        TIMED.builds(),
        1,
        "the suite built {} timestamp backends; the cell must build exactly one",
        TIMED.builds()
    );
    eprintln!(
        "RECEIPT profiling_readback: timestamp backend builds = {}",
        TIMED.builds()
    );
}

fn pattern(elements: usize) -> Vec<f32> {
    (0..elements).map(|i| (i % 1024) as f32).collect()
}

// --- timestamp capability and query-set bounds ------------------------------

#[test]
#[ignore = "GPU profiling instrumentation; opt in with --ignored --exact"]
fn timer_exists_exactly_when_the_device_reports_timestamp_support() {
    let Some(backend) = timed_backend("timer_capability") else {
        return;
    };
    let support = TimestampSupport::from_features(backend.device().features());
    let timer = GpuTimer::new(backend, 4);
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
#[ignore = "GPU profiling instrumentation; opt in with --ignored --exact"]
fn timer_rejects_indices_and_counts_beyond_capacity() {
    let Some(backend) = timed_backend("timer_bounds") else {
        return;
    };
    let Some(timer) = GpuTimer::new(backend, 4) else {
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
#[ignore = "GPU profiling instrumentation; opt in with --ignored --exact"]
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
    let mut staging = StagingStorage::new();
    let expected = pattern(elements);

    // fresh staging, single copy
    let (fresh, t) = timed_readback(backend, src.buffer(), bytes, ReadbackPlan::default())
        .expect("fresh readback");
    assert_eq!(fresh, expected, "fresh staging returned wrong bytes");
    assert!(t.fresh_staging, "fresh path must be recorded as fresh");

    // reused staging, single copy
    let (reused, t) = timed_readback(
        backend,
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
            backend,
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
#[ignore = "GPU profiling instrumentation; opt in with --ignored --exact"]
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
        backend,
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
#[ignore = "GPU profiling instrumentation; opt in with --ignored --exact"]
fn readback_handles_zero_length_and_non_chunk_multiple_payloads() {
    let Some(backend) = timed_backend("readback_edges") else {
        return;
    };
    let res = backend.resources();

    let empty = res
        .zeroed_tensor("empty", TensorShape::new(0, 8))
        .expect("zero tensor");
    let (v, t) = timed_readback(backend, empty.buffer(), 0, ReadbackPlan::default())
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
        backend,
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
// --- staging storage growth and reuse ---------------------------------------

#[test]
#[ignore = "GPU profiling instrumentation; opt in with --ignored --exact"]
fn staging_storage_grows_on_demand_and_never_shrinks() {
    let Some(backend) = timed_backend("staging_growth") else {
        return;
    };
    let mut staging = StagingStorage::new();
    assert_eq!(staging.capacity(), 0, "nothing is allocated up front");
    assert_eq!(staging.allocations(), 0);

    let small = 4_096usize;
    let large = 65_536usize;
    let res = backend.resources();
    let s = res
        .upload_tensor("s", TensorShape::new(small, 1), &pattern(small))
        .expect("upload small");
    let l = res
        .upload_tensor("l", TensorShape::new(large, 1), &pattern(large))
        .expect("upload large");

    let read = |staging: &mut StagingStorage, buf, bytes| {
        timed_readback(
            backend,
            buf,
            bytes,
            ReadbackPlan {
                staging: Some(staging),
                ..ReadbackPlan::default()
            },
        )
        .expect("readback")
        .0
    };

    // First read allocates.
    let got = read(&mut staging, s.buffer(), (small * 4) as u64);
    assert_eq!(got, pattern(small));
    assert_eq!(staging.allocations(), 1);
    let after_small = staging.capacity();
    assert!(after_small >= (small * 4) as u64);

    // Same size again: no new allocation.
    let got = read(&mut staging, s.buffer(), (small * 4) as u64);
    assert_eq!(got, pattern(small));
    assert_eq!(
        staging.allocations(),
        1,
        "an identical-size readback must not allocate"
    );

    // Larger: grows exactly once more.
    let got = read(&mut staging, l.buffer(), (large * 4) as u64);
    assert_eq!(got, pattern(large));
    assert_eq!(staging.allocations(), 2, "growth costs one allocation");
    let after_large = staging.capacity();
    assert!(after_large >= (large * 4) as u64);

    // Smaller after larger: reuses the larger buffer, does not shrink.
    let got = read(&mut staging, s.buffer(), (small * 4) as u64);
    assert_eq!(got, pattern(small));
    assert_eq!(
        staging.allocations(),
        2,
        "a smaller readback after a larger one must not reallocate"
    );
    assert_eq!(
        staging.capacity(),
        after_large,
        "capacity must not shrink implicitly"
    );
}

#[test]
#[ignore = "GPU profiling instrumentation; opt in with --ignored --exact"]
fn staging_storage_can_be_presized() {
    let Some(backend) = timed_backend("staging_presize") else {
        return;
    };
    let staging = StagingStorage::with_capacity(backend, 1 << 20);
    assert!(staging.capacity() >= (1 << 20));
    assert_eq!(staging.allocations(), 1);
}

#[test]
#[ignore = "GPU profiling instrumentation; opt in with --ignored --exact"]
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
    let mut staging = StagingStorage::new();
    let expected = pattern(elements);

    // Ten consecutive readbacks through one buffer. A missing unmap would fail
    // the very next map with a validation error, so this is a direct test that
    // the drain primitive unmaps on every path — not a timing observation.
    for round in 0..10 {
        let (data, _) = timed_readback(
            backend,
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
    }
    assert_eq!(
        staging.allocations(),
        1,
        "ten same-size readbacks must allocate exactly once"
    );
}
