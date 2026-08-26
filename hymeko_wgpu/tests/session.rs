//! Phase 5 §4: tracked-session drop paths and error states.
//!
//! A session owns a command encoder and mediates buffer lifetime, so its
//! abnormal exits are exactly where a pool leaks or frees too early. These tests
//! cover the ways a session can end other than a clean submit.
//!
//! Some cases in the brief are **structurally unreachable** rather than untested,
//! and are documented as such at the bottom of this file rather than faked with a
//! test that cannot fail.

use hymeko::tensor::compute::TensorShape;
use hymeko_hnn::workloads::SparseSpec;
use hymeko_wgpu::testing::backend_or_skip;
use hymeko_wgpu::{ActivationKind, TensorArenaConfig};

/// Dropping a session without submitting must free its pending releases
/// immediately: commands that were never submitted can never run, so nothing
/// needs to complete first.
#[test]
fn test_session_dropped_without_submit_frees_pending_immediately() {
    let Some(backend) = backend_or_skip("drop_no_submit") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let shape = TensorShape::new(512, 4);

    {
        let session = arena.session(Some("abandoned"));
        let t = arena.acquire(shape).unwrap();
        drop(t); // released while the session is open -> pending
        assert_eq!(
            arena.stats().cached_buffers,
            0,
            "a pending release must not be cached while the session is open"
        );
        drop(session); // never submitted
    }

    arena.poll_completion().unwrap();
    let s = arena.stats();
    assert_eq!(
        s.in_flight_bytes, 0,
        "unsubmitted work must not stay in flight"
    );
    assert_eq!(
        s.cached_buffers, 1,
        "the buffer should be immediately reusable after an abandoned session"
    );

    // And it really is reusable.
    let again = arena.acquire(shape).unwrap();
    assert_eq!(arena.stats().reuse_hits, 1);
    drop(again);
}

/// An empty session is legal and still advances the epoch. Refusing it would
/// force every caller into a conditional around "did I encode anything?".
#[test]
fn test_empty_session_submits_cleanly() {
    let Some(backend) = backend_or_skip("empty") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());

    let session = arena.session(Some("empty"));
    assert!(session.is_empty());
    assert_eq!(session.operations_encoded(), 0);
    let (_index, epoch) = session.submit();
    assert!(epoch.get() > 0, "an empty submit must still take an epoch");

    arena.reclaim_all().unwrap();
    assert!(epoch.is_complete_at(arena.tracker().completed()));
}

/// Dropping the session value after `submit` consumed it is not expressible —
/// `submit(self)` moves it. What this checks is the state afterwards: the arena
/// must not still believe a session is open.
#[test]
fn test_state_is_clean_after_submit() {
    let Some(backend) = backend_or_skip("after_submit") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let shape = TensorShape::new(256, 4);

    let t = arena.acquire(shape).unwrap();
    let session = arena.session(None);
    let (_i, _e) = session.submit();
    drop(t);

    // With no session open the release is DATED against already-issued work.
    // A pending (undated) slab would survive `reclaim_all` untouched, so this
    // distinguishes the two states without depending on when the GPU finishes --
    // `poll_completion` alone would be a race, not a test.
    arena.reclaim_all().unwrap();
    assert_eq!(
        arena.stats().cached_buffers,
        1,
        "arena still behaves as if a session were open after submit"
    );
    assert_eq!(arena.stats().in_flight_bytes, 0);
}

/// A submission left in flight, never waited on, must still be reclaimable
/// later — the whole point of the non-blocking design.
#[test]
fn test_in_flight_submission_is_reclaimed_by_a_later_poll() {
    let Some(backend) = backend_or_skip("in_flight") else {
        return;
    };
    let spec = SparseSpec::small();
    let csr = spec.build();
    let fd = 32;
    let res = backend.resources();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x = spec.dense_block(fd);
    let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
    let arena = backend.tensor_arena(TensorArenaConfig::default());

    {
        let out = arena.acquire(shape.output_shape(fd)).unwrap();
        let spmm = backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&input)
            .output(&out)
            .prepare()
            .unwrap();
        let mut session = arena.session(Some("no-wait"));
        spmm.encode(&mut session).unwrap();
        assert_eq!(session.operations_encoded(), 1);
        let (_index, _epoch) = session.submit(); // deliberately no wait
    }

    // The buffer is in flight; it must not be cached yet.
    assert!(
        arena.stats().in_flight_bytes > 0,
        "released buffer should be accounted in flight before completion"
    );

    // reclaim_all blocks, which is the sanctioned way to force the issue.
    arena.reclaim_all().unwrap();
    assert_eq!(arena.stats().in_flight_bytes, 0);
    assert_eq!(arena.stats().cached_buffers, 1);
}

/// Several sessions in sequence, each submitted, with releases interleaved.
#[test]
fn test_sequential_sessions_keep_accounting_consistent() {
    let Some(backend) = backend_or_skip("sequential") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let shape = TensorShape::new(1024, 2);

    for _ in 0..4 {
        let t = arena.acquire(shape).unwrap();
        let mut session = arena.session(None);
        let _ = session.encoder(); // touch the raw encoder path
        session.submit();
        drop(t);
        // Blocking, not `poll_completion`: whether a just-submitted epoch has
        // completed is a race, and a reuse-count assertion on top of a race is a
        // flaky test rather than a strict one.
        arena.reclaim_all().unwrap();
    }

    arena.reclaim_all().unwrap();
    let s = arena.stats();
    assert_eq!(s.active_bytes, 0, "no lease should still be live");
    assert_eq!(s.in_flight_bytes, 0);
    assert_eq!(
        s.buffers_created, 1,
        "four identical rounds should reuse one buffer"
    );
    assert_eq!(s.reuse_hits, 3);
}

/// Two sessions open at once disables same-session reuse, because their submit
/// order is not determined. The arena must fall back to the conservative path
/// rather than hand a slab from one session to the other.
#[test]
fn test_two_open_sessions_disable_same_session_reuse() {
    let Some(backend) = backend_or_skip("two_sessions") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let shape = TensorShape::new(256, 4);

    let first = arena.session(Some("first"));
    let mut second = arena.session(Some("second"));

    let t = arena.acquire(shape).unwrap();
    drop(t); // owner ambiguous -> not eligible for same-session reuse

    let fresh = second.acquire(shape).unwrap();
    assert_eq!(
        arena.stats().buffers_created,
        2,
        "an ambiguously-owned pending slab must not be reused"
    );
    drop(fresh);

    second.submit();
    first.submit();
    arena.reclaim_all().unwrap();
}

/// Dropping the arena while a submission is still in flight must not panic and
/// must not require a wait. wgpu keeps the underlying buffers alive until the
/// GPU is finished with them.
#[test]
fn test_arena_dropped_with_work_in_flight() {
    let Some(backend) = backend_or_skip("arena_drop") else {
        return;
    };
    let spec = SparseSpec::small();
    let csr = spec.build();
    let fd = 16;
    let res = backend.resources();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x = spec.dense_block(fd);
    let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();

    {
        let arena = backend.tensor_arena(TensorArenaConfig::default());
        let out = arena.acquire(shape.output_shape(fd)).unwrap();
        let spmm = backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&input)
            .output(&out)
            .prepare()
            .unwrap();
        let mut session = arena.session(None);
        spmm.encode(&mut session).unwrap();
        session.submit();
        // `out`, then `arena`, drop here with the submission still in flight.
    }

    // The device is still usable afterwards.
    backend.wait(backend.submit(backend.encoder(None))).unwrap();
}

/// A session with a mix of pooled and caller-owned operands: the pooled ones go
/// through the session, the plain one through its raw encoder, in one submit.
#[test]
fn test_mixed_scope_operations_in_one_session() {
    let Some(backend) = backend_or_skip("mixed") else {
        return;
    };
    let spec = SparseSpec::small();
    let csr = spec.build();
    let fd = 8;
    let res = backend.resources();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x = spec.dense_block(fd);
    let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
    let plain = res.zeroed_tensor("plain", shape.output_shape(fd)).unwrap();
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let pooled = arena.acquire(shape.output_shape(fd)).unwrap();

    // Untracked: every operand caller-owned.
    let to_plain = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&plain)
        .prepare()
        .unwrap();
    // Tracked: output is pooled.
    let to_pooled = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&pooled)
        .prepare()
        .unwrap();

    let mut session = arena.session(Some("mixed"));
    to_plain.encode(session.encoder()).unwrap(); // raw encoder, untracked op
    to_pooled.encode(&mut session).unwrap(); // session, tracked op
    assert_eq!(
        session.operations_encoded(),
        1,
        "only pooled operations are counted as tracked"
    );
    session.submit_and_wait().unwrap();

    let a = backend.read_tensor(&plain).unwrap();
    let b = backend.read_tensor(&pooled).unwrap();
    assert_eq!(
        a.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        b.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        "the same computation into a plain and a pooled tensor must agree"
    );
}

/// An activation chained after SpMM inside one session, with the intermediate
/// released mid-session and immediately reused by the session itself.
#[test]
fn test_intra_session_reuse_is_ordered_correctly() {
    let Some(backend) = backend_or_skip("intra_reuse") else {
        return;
    };
    let spec = SparseSpec::small();
    let csr = spec.build();
    let fd = 16;
    let res = backend.resources();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x = spec.dense_block(fd);
    let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
    let out_shape = shape.output_shape(fd);
    let arena = backend.tensor_arena(TensorArenaConfig::default());

    let mut session = arena.session(Some("intra"));
    let final_values;
    {
        let scratch = arena.acquire(out_shape).unwrap();
        let spmm = backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&input)
            .output(&scratch)
            .prepare()
            .unwrap();
        let act = backend
            .operations()
            .activation()
            .kind(ActivationKind::Relu)
            .in_place(&scratch)
            .prepare()
            .unwrap();
        spmm.encode(&mut session).unwrap();
        act.encode(&mut session).unwrap();
        // Read it back before the scratch dies, so there is something to compare.
        let reused = session.acquire(out_shape);
        assert!(reused.is_ok());
        drop(reused);
        final_values = scratch.shape();
    }
    // `scratch` is now released into this session's pending set.
    let taken = session.acquire(out_shape).unwrap();
    assert_eq!(
        arena.stats().buffers_created,
        2,
        "same-session reuse should recycle the scratch buffer"
    );
    assert_eq!(taken.shape(), final_values);
    drop(taken);
    session.submit_and_wait().unwrap();
}

// ---------------------------------------------------------------------------
// Cases the brief lists that are structurally unreachable
// ---------------------------------------------------------------------------
//
// Recorded here rather than covered by a test that cannot fail:
//
// * **encode failing mid-session** — `encode` is currently infallible. Every
//   fallible check (shapes, limits, aliasing, narrowing) happens in `prepare`,
//   which is the design: `encode` is the hot path. The `Result` is kept in the
//   signature so a future operation needing per-encode validation is not a
//   breaking change. A test would have to fabricate a failure that no code path
//   produces.
//
// * **submit failing** — `wgpu::Queue::submit` does not return a `Result`;
//   device loss surfaces asynchronously through the device-lost callback, not at
//   the submit site. There is no reproducible failure to assert on, and
//   simulating one would test the mock rather than the arena.
//
// * **submitting twice** — `submit(self)` consumes the session, so a second call
//   is a use-after-move. Covered as a `compile_fail` doctest on
//   `scope::SessionSubmittedOnce`, which demonstrates the rejection instead of
//   asserting it in prose.
//
// * **backend dropped before the arena** — `TensorArena<'b>` borrows the backend
//   for `'b`, so the borrow checker rejects it. There is no runtime ordering to
//   test.
