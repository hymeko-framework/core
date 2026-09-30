//! Phase 8 lifecycle tests for [`ReadbackWorkspace`].
//!
//! Reuse is proved **structurally** — allocation counters and capacities — never
//! by a timing threshold. A test that asserted "the second read was faster"
//! would be flaky on a loaded machine and would pass for the wrong reason on a
//! fast one.
//!
//! These create a device per test, so on Linux they must run with
//! `--test-threads=1`; concurrent `WgpuBackend` construction deadlocks in the
//! Vulkan loader. That is a property of device creation, not of these tests.

use hymeko_wgpu::testing::shared_backend;
use hymeko_wgpu::{ComputeError, TensorShape, WgpuBackend};

fn pattern(elements: usize) -> Vec<f32> {
    (0..elements).map(|i| (i % 977) as f32).collect()
}

fn upload(backend: &WgpuBackend, label: &str, elements: usize) -> hymeko_wgpu::GpuTensor<f32> {
    backend
        .resources()
        .upload_tensor(label, TensorShape::new(elements, 1), &pattern(elements))
        .expect("upload")
}

// --- allocation lifecycle ---------------------------------------------------

#[test]
fn first_read_allocates_then_identical_sizes_allocate_nothing() {
    let Some(backend) = shared_backend("ws_first_alloc") else {
        return;
    };
    let n = 4_096usize;
    let t = upload(backend, "t", n);
    let mut ws = backend.create_readback_workspace();

    assert_eq!(ws.staging_allocations(), 0, "nothing allocated up front");
    assert_eq!(ws.staging_capacity(), 0);
    assert_eq!(ws.destination_capacity(), 0);

    let got = backend.read_tensor_into(&t, &mut ws).expect("first read");
    assert_eq!(got, pattern(n).as_slice());
    assert_eq!(ws.staging_allocations(), 1);
    let dest_cap = ws.destination_capacity();
    assert!(dest_cap >= n);

    // Four more identical reads: no staging allocation, no destination growth.
    for round in 0..4 {
        let got = backend
            .read_tensor_into(&t, &mut ws)
            .unwrap_or_else(|e| panic!("round {round}: {e}"));
        assert_eq!(got.len(), n);
        assert_eq!(
            ws.staging_allocations(),
            1,
            "round {round} allocated staging"
        );
        assert_eq!(
            ws.destination_capacity(),
            dest_cap,
            "round {round} regrew the destination"
        );
    }
}

#[test]
fn a_smaller_read_after_a_larger_one_reuses_both_buffers() {
    let Some(backend) = shared_backend("ws_smaller_after_larger") else {
        return;
    };
    let large = 32_768usize;
    let small = 512usize;
    let l = upload(backend, "l", large);
    let s = upload(backend, "s", small);
    let mut ws = backend.create_readback_workspace();

    let got = backend.read_tensor_into(&l, &mut ws).expect("large");
    assert_eq!(got.len(), large);
    let staging_cap = ws.staging_capacity();
    let dest_cap = ws.destination_capacity();
    assert_eq!(ws.staging_allocations(), 1);

    let got = backend.read_tensor_into(&s, &mut ws).expect("small");
    assert_eq!(
        got,
        pattern(small).as_slice(),
        "the small read must return exactly its own elements"
    );
    assert_eq!(
        got.len(),
        small,
        "spare capacity must never appear in the returned slice"
    );
    assert_eq!(
        ws.staging_allocations(),
        1,
        "a smaller read must not reallocate staging"
    );
    assert_eq!(
        ws.staging_capacity(),
        staging_cap,
        "staging must not shrink implicitly"
    );
    assert_eq!(
        ws.destination_capacity(),
        dest_cap,
        "the destination must not shrink implicitly"
    );
}

#[test]
fn growth_is_safe_and_returns_correct_bytes() {
    let Some(backend) = shared_backend("ws_growth") else {
        return;
    };
    let mut ws = backend.create_readback_workspace();
    let mut previous_capacity = 0u64;

    for &n in &[64usize, 1_024, 16_384, 100_003] {
        let t = upload(backend, "grow", n);
        let got = backend.read_tensor_into(&t, &mut ws).expect("read");
        assert_eq!(got, pattern(n).as_slice(), "n={n} returned wrong bytes");
        assert!(
            ws.staging_capacity() >= previous_capacity,
            "capacity went backwards at n={n}"
        );
        previous_capacity = ws.staging_capacity();
    }
}

#[test]
fn reserve_moves_both_allocations_out_of_the_first_read() {
    let Some(backend) = shared_backend("ws_reserve") else {
        return;
    };
    let n = 8_192usize;
    let t = upload(backend, "t", n);
    let mut ws = backend.create_readback_workspace();
    ws.reserve(backend, n).expect("reserve");

    assert_eq!(ws.staging_allocations(), 1, "reserve allocates staging");
    assert!(ws.staging_capacity() >= (n * 4) as u64);
    assert!(ws.destination_capacity() >= n);
    let dest_cap = ws.destination_capacity();

    let got = backend.read_tensor_into(&t, &mut ws).expect("read");
    assert_eq!(got, pattern(n).as_slice());
    assert_eq!(
        ws.staging_allocations(),
        1,
        "a reserved workspace must not allocate again on first use"
    );
    assert_eq!(ws.destination_capacity(), dest_cap);
}

// --- edge cases -------------------------------------------------------------

#[test]
fn zero_element_tensor_yields_an_empty_slice() {
    let Some(backend) = shared_backend("ws_zero") else {
        return;
    };
    let empty = backend
        .resources()
        .zeroed_tensor("empty", TensorShape::new(0, 8))
        .expect("zero tensor");
    let mut ws = backend.create_readback_workspace();

    let got = backend
        .read_tensor_into(&empty, &mut ws)
        .expect("zero read");
    assert!(got.is_empty(), "a zero-element tensor must read back empty");
    assert_eq!(
        ws.staging_allocations(),
        0,
        "a zero-length read must not touch the device"
    );

    // And the workspace still works afterwards.
    let t = upload(backend, "after", 128);
    let got = backend
        .read_tensor_into(&t, &mut ws)
        .expect("read after zero");
    assert_eq!(got.len(), 128);
}

#[test]
fn non_page_and_non_chunk_multiple_lengths_round_trip_exactly() {
    let Some(backend) = shared_backend("ws_odd_lengths") else {
        return;
    };
    let mut ws = backend.create_readback_workspace();
    // 1: minimum; 3: sub-word; 1023/1025: astride a power of two;
    // 3_145_733: 20 bytes past an 8 MiB boundary.
    for &n in &[1usize, 3, 1_023, 1_025, 4_097, 3_145_733] {
        let t = upload(backend, "odd", n);
        let got = backend.read_tensor_into(&t, &mut ws).expect("read");
        assert_eq!(got.len(), n, "n={n} wrong length");
        assert_eq!(got, pattern(n).as_slice(), "n={n} wrong bytes");
    }
}

#[test]
fn a_stale_destination_cannot_masquerade_as_a_result() {
    let Some(backend) = shared_backend("ws_stale") else {
        return;
    };
    let big = 4_096usize;
    let small = 16usize;
    let b = upload(backend, "b", big);
    let s = upload(backend, "s", small);
    let mut ws = backend.create_readback_workspace();

    let first = backend.read_tensor_into(&b, &mut ws).expect("big").to_vec();
    assert_eq!(first.len(), big);

    // The destination still holds `big` elements of capacity and the previous
    // contents. The small read must expose neither.
    let got = backend.read_tensor_into(&s, &mut ws).expect("small");
    assert_eq!(got.len(), small);
    assert_eq!(got, pattern(small).as_slice());
}

// --- correctness against the legacy API ------------------------------------

#[test]
fn reusable_and_legacy_apis_agree_bit_for_bit() {
    let Some(backend) = shared_backend("ws_vs_legacy") else {
        return;
    };
    let mut ws = backend.create_readback_workspace();
    for &n in &[0usize, 1, 997, 65_536] {
        let t = if n == 0 {
            backend
                .resources()
                .zeroed_tensor("z", TensorShape::new(0, 4))
                .expect("zero")
        } else {
            upload(backend, "t", n)
        };
        let legacy = backend.read_tensor(&t).expect("legacy");
        let reused = backend.read_tensor_into(&t, &mut ws).expect("reusable");
        assert_eq!(
            legacy.as_slice(),
            reused,
            "n={n}: the two APIs must produce identical bytes"
        );
    }
}

#[test]
fn sequential_reads_of_different_tensors_do_not_bleed() {
    let Some(backend) = shared_backend("ws_sequential") else {
        return;
    };
    let a = backend
        .resources()
        .upload_tensor("a", TensorShape::new(4, 1), &[1.0, 2.0, 3.0, 4.0])
        .expect("a");
    let b = backend
        .resources()
        .upload_tensor("b", TensorShape::new(4, 1), &[9.0, 8.0, 7.0, 6.0])
        .expect("b");
    let mut ws = backend.create_readback_workspace();

    assert_eq!(
        backend.read_tensor_into(&a, &mut ws).expect("a"),
        [1.0, 2.0, 3.0, 4.0]
    );
    assert_eq!(
        backend.read_tensor_into(&b, &mut ws).expect("b"),
        [9.0, 8.0, 7.0, 6.0]
    );
    assert_eq!(
        backend.read_tensor_into(&a, &mut ws).expect("a again"),
        [1.0, 2.0, 3.0, 4.0]
    );
}

// --- independence and provenance -------------------------------------------

#[test]
fn independent_workspaces_do_not_share_state() {
    let Some(backend) = shared_backend("ws_independent") else {
        return;
    };
    let a = upload(backend, "a", 1_024);
    let b = upload(backend, "b", 8_192);
    let mut wa = backend.create_readback_workspace();
    let mut wb = backend.create_readback_workspace();

    let ra = backend.read_tensor_into(&a, &mut wa).expect("a").to_vec();
    let rb = backend.read_tensor_into(&b, &mut wb).expect("b").to_vec();

    assert_eq!(ra, pattern(1_024));
    assert_eq!(rb, pattern(8_192));
    assert_eq!(wa.staging_allocations(), 1);
    assert_eq!(wb.staging_allocations(), 1);
    assert_ne!(
        wa.staging_capacity(),
        wb.staging_capacity(),
        "two workspaces must hold their own, differently sized buffers"
    );
}

#[test]
fn an_arena_tensor_reads_back_without_any_raw_buffer_access() {
    let Some(backend) = shared_backend("ws_arena") else {
        return;
    };
    let arena = backend.tensor_arena(hymeko_wgpu::TensorArenaConfig::default());
    let out = arena.acquire(TensorShape::new(256, 1)).expect("acquire");
    backend
        .resources()
        .write_tensor(&out, &pattern(256))
        .expect("write");

    let mut ws = backend.create_readback_workspace();
    // `out` is an ArenaTensor; this compiles only through the sealed
    // TensorOperand trait, never through a public raw buffer accessor.
    let got = backend.read_tensor_into(&out, &mut ws).expect("arena read");
    assert_eq!(got, pattern(256).as_slice());
}

// --- limits and failure behaviour ------------------------------------------

#[test]
fn a_transfer_beyond_the_device_limit_is_refused_by_name() {
    let Some(backend) = shared_backend("ws_limit") else {
        return;
    };
    let max = backend.capabilities().max_buffer_size;
    // A shape whose byte length exceeds the device maximum, without ever
    // allocating it: validation happens before any device call.
    let Ok(elements) = usize::try_from(max / 4 + 1) else {
        eprintln!("[ws_limit] SKIPPED - device limit exceeds usize");
        return;
    };
    let mut ws = backend.create_readback_workspace();
    // Reached through the public API rather than a test-only accessor: reserve
    // performs the same extent validation a read does.
    let err = ws
        .reserve(backend, elements)
        .expect_err("a transfer past max_buffer_size must be refused");
    assert!(
        matches!(err, ComputeError::ShapeMismatch { .. }),
        "expected ShapeMismatch naming the device limit, got {err:?}"
    );
}

#[test]
fn a_workspace_remains_usable_after_a_rejected_read() {
    let Some(backend) = shared_backend("ws_after_failure") else {
        return;
    };
    let mut ws = backend.create_readback_workspace();
    let good = upload(backend, "good", 512);

    // Warm the workspace so the failure below has state to corrupt.
    assert_eq!(
        backend
            .read_tensor_into(&good, &mut ws)
            .expect("warm")
            .len(),
        512
    );
    let allocations = ws.staging_allocations();

    // A reserve past the device limit is rejected without touching the device.
    let max = backend.capabilities().max_buffer_size;
    if let Ok(too_many) = usize::try_from(max / 4 + 1) {
        assert!(
            ws.reserve(backend, too_many).is_err(),
            "an over-limit reserve must be refused"
        );
    }

    // The workspace still reads correctly and did not silently reallocate.
    let got = backend
        .read_tensor_into(&good, &mut ws)
        .expect("after failure");
    assert_eq!(got, pattern(512).as_slice());
    assert_eq!(
        ws.staging_allocations(),
        allocations,
        "a rejected request must not disturb existing staging"
    );
}
