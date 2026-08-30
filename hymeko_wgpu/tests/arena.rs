//! Phase 4 correctness: `TensorArena` buffer reuse.
//!
//! # What these tests are actually guarding
//!
//! A buffer pool fails silently. If a buffer is recycled while the GPU is still
//! reading it, or handed to two logical tensors at once, the symptom is *wrong
//! numbers*, not a crash or a validation error. So the tests here are built
//! around two ideas:
//!
//! 1. **Poison.** Every recycled buffer is deliberately filled with a value that
//!    could not arise from the computation. If any of it survives into a result,
//!    a test fails with a recognisable number rather than a plausible one.
//! 2. **Differential.** Every numeric result from the arena path is compared
//!    against the same computation run with freshly allocated buffers. The
//!    reference is the Phase 2/3 path that already has its own tests.

use hymeko::tensor::compute::{ComputeError, TensorShape};
use hymeko::tensor::representations::tensor_csr::TensorCsr;
use hymeko_wgpu::testing::SparseSpec;
use hymeko_wgpu::testing::backend_or_skip;
use hymeko_wgpu::{ActivationKind, ReusePolicy, SubmissionEpoch, TensorArenaConfig};

/// A value the computation cannot produce, so its appearance proves a leak.
const POISON: f32 = -98765.5;

fn small_csr() -> TensorCsr<f32> {
    SparseSpec::small().build()
}

// ---------------------------------------------------------------------------
// allocation identity and reuse
// ---------------------------------------------------------------------------

/// The core promise: same shape and usage, released, comes back as the same
/// allocation rather than a new one.
#[test]
fn test_same_shape_and_usage_is_reused() {
    let Some(backend) = backend_or_skip("reuse") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let shape = TensorShape::new(1024, 4);

    // Identity is established two ways, because wgpu 30 exposes no buffer id:
    //   1. structurally -- `buffers_created` does not increase, so no new
    //      allocation exists and the one returned must come from the pool;
    //   2. behaviourally -- a sentinel written before release is still present
    //      after re-acquire, proving it is the same device memory.
    const SENTINEL: f32 = 1234.5;
    {
        let a = arena.acquire(shape).unwrap();
        backend
            .resources()
            .write_tensor(&a, &vec![SENTINEL; shape.elements()])
            .unwrap();
        backend.wait(backend.submit(backend.encoder(None))).unwrap();
    }
    assert_eq!(arena.stats().reuse_misses, 1, "first acquire must allocate");
    assert_eq!(arena.stats().buffers_created, 1);

    // Nothing referenced it in a submitted command, so it is immediately safe.
    arena.poll_completion().unwrap();

    let b = arena.acquire(shape).unwrap();
    assert_eq!(arena.stats().reuse_hits, 1, "second acquire must reuse");
    assert_eq!(
        arena.stats().buffers_created,
        1,
        "reuse must not create a second buffer"
    );
    // Also documents the contract: a recycled buffer is NOT cleared.
    let contents = backend.read_tensor(&b).unwrap();
    assert!(
        contents.iter().all(|v| *v == SENTINEL),
        "re-acquired buffer is not the same memory, or was unexpectedly cleared"
    );
}

/// Two live leases must never be the same allocation. This is the invariant a
/// pool most obviously breaks.
#[test]
fn test_two_live_leases_never_alias() {
    let Some(backend) = backend_or_skip("alias") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let shape = TensorShape::new(256, 4);

    let a = arena.acquire(shape).unwrap();
    let b = arena.acquire(shape).unwrap();
    let c = arena.acquire(shape).unwrap();

    // Three live leases must be three allocations.
    assert_eq!(arena.stats().buffers_created, 3);
    assert_eq!(arena.stats().reuse_hits, 0);

    // The decisive check is at the data level, not the handle level: writing one
    // lease must not disturb another. Aliasing would show up here as a value
    // from the wrong write.
    let res = backend.resources();
    res.write_tensor(&a, &vec![1.0f32; 1024]).unwrap();
    res.write_tensor(&b, &vec![2.0f32; 1024]).unwrap();
    res.write_tensor(&c, &vec![3.0f32; 1024]).unwrap();
    backend.wait(backend.submit(backend.encoder(None))).unwrap();

    assert!(backend.read_tensor(&a).unwrap().iter().all(|v| *v == 1.0));
    assert!(backend.read_tensor(&b).unwrap().iter().all(|v| *v == 2.0));
    assert!(backend.read_tensor(&c).unwrap().iter().all(|v| *v == 3.0));
}

/// Different byte sizes are different resources and must not be confused.
#[test]
fn test_different_sizes_do_not_share_allocations() {
    let Some(backend) = backend_or_skip("sizes") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());

    drop(arena.acquire(TensorShape::new(1024, 4)).unwrap());
    arena.poll_completion().unwrap();

    // A larger request cannot be served by the cached smaller buffer.
    let big = arena.acquire(TensorShape::new(4096, 4)).unwrap();
    assert_eq!(
        arena.stats().reuse_hits,
        0,
        "a smaller slab was reused for a larger request"
    );
    assert_eq!(arena.stats().buffers_created, 2);
    assert!(big.allocation_bytes() >= 4096 * 4 * 4);
}

/// Shapes that differ but have the same byte size legitimately share an
/// allocation — the buffer is untyped bytes, and the shape lives on the tensor.
#[test]
fn test_same_bytes_different_shape_is_reused() {
    let Some(backend) = backend_or_skip("shape") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());

    drop(arena.acquire(TensorShape::new(64, 16)).unwrap());
    arena.poll_completion().unwrap();

    let t = arena.acquire(TensorShape::new(128, 8)).unwrap();
    assert_eq!(
        arena.stats().reuse_hits,
        1,
        "equal-byte allocation should be reused"
    );
    assert_eq!(
        t.shape(),
        TensorShape::new(128, 8),
        "shape must follow the request"
    );
}

/// Usage flags are fixed at buffer creation, so they partition the pool.
#[test]
fn test_different_usage_is_not_reused() {
    let Some(backend) = backend_or_skip("usage") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let shape = TensorShape::new(64, 4);

    drop(arena.acquire(shape).unwrap());
    arena.poll_completion().unwrap();

    let other = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
    let t = arena.acquire_with_usage(shape, other).unwrap();
    assert_eq!(
        arena.stats().reuse_hits,
        0,
        "a buffer created with different usage flags must not be reused"
    );
    assert_eq!(t.usage(), other);
}

/// A zero-sized tensor must remain valid: wgpu rejects zero-length buffers, so
/// the arena pads, and the kernels bound themselves by their params block.
#[test]
fn test_zero_sized_tensor_is_valid() {
    let Some(backend) = backend_or_skip("zero") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());

    let z = arena.acquire(TensorShape::new(0, 8)).unwrap();
    assert_eq!(z.shape().elements(), 0, "logical shape stays empty");
    assert!(
        z.allocation_bytes() > 0,
        "allocation must not be zero-length"
    );
    assert!(backend.read_tensor(&z).unwrap().is_empty());

    let z2 = arena.acquire(TensorShape::new(8, 0)).unwrap();
    assert_eq!(z2.shape().elements(), 0);
}

/// `SizeClass` trades internal fragmentation for hit rate; it must actually hit.
#[test]
fn test_size_class_policy_reuses_a_larger_slab() {
    let Some(backend) = backend_or_skip("sizeclass") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig {
        reuse_policy: ReusePolicy::SizeClass,
        ..TensorArenaConfig::default()
    });

    // 1000 f32 = 4000 B -> class 4096. 900 f32 = 3600 B -> same class.
    drop(arena.acquire(TensorShape::new(1000, 1)).unwrap());
    arena.poll_completion().unwrap();
    let t = arena.acquire(TensorShape::new(900, 1)).unwrap();

    assert_eq!(arena.stats().reuse_hits, 1, "same size class should reuse");
    assert!(
        t.allocation_bytes() >= 900 * 4,
        "reused slab must still be large enough"
    );
}

// ---------------------------------------------------------------------------
// in-flight safety — the part that fails silently if it is wrong
// ---------------------------------------------------------------------------

/// A buffer released while its work is still in flight must NOT be handed out
/// again before completion.
#[test]
fn test_in_flight_buffer_is_not_recycled_early() {
    let Some(backend) = backend_or_skip("inflight") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let csr = small_csr();
    let fd = 32;
    let matrix = backend.resources().upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x = SparseSpec::small().dense_block(fd);
    let input = backend
        .resources()
        .upload_tensor("x", shape.input_shape(fd), &x)
        .unwrap();

    let epoch = {
        let out = arena.acquire(shape.output_shape(fd)).unwrap();
        let spmm = backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&input)
            .output(&out)
            .prepare()
            .unwrap();

        let mut session = arena.session(Some("inflight"));
        spmm.encode(&mut session).unwrap();
        // Submit, then drop the lease while the GPU may still be working.
        session.submit().1
        // `out` and `spmm` drop here.
    };

    assert!(epoch > SubmissionEpoch::NEVER);
    let stats = arena.stats();
    assert_eq!(
        stats.cached_buffers, 0,
        "a released-but-in-flight buffer must not be on the free list"
    );
    assert!(
        stats.in_flight_bytes > 0,
        "the released buffer should be accounted as in flight"
    );

    // After completion it becomes reusable.
    arena.reclaim_all().unwrap();
    let after = arena.stats();
    assert_eq!(
        after.in_flight_bytes, 0,
        "completion should clear in-flight bytes"
    );
    assert_eq!(after.cached_buffers, 1, "the buffer should now be cached");

    let reused = arena.acquire(shape.output_shape(fd)).unwrap();
    assert_eq!(arena.stats().reuse_hits, 1);
    drop(reused);
}

/// The order `acquire -> encode -> drop lease -> submit` is natural to write and
/// is exactly the one a naive pool gets wrong: the buffer is released before the
/// commands referencing it are submitted.
#[test]
fn test_release_before_submit_is_not_recycled_early() {
    let Some(backend) = backend_or_skip("early_release") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let csr = small_csr();
    let fd = 8;
    let matrix = backend.resources().upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x = SparseSpec::small().dense_block(fd);
    let input = backend
        .resources()
        .upload_tensor("x", shape.input_shape(fd), &x)
        .unwrap();

    let mut session = arena.session(Some("early"));
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
        spmm.encode(&mut session).unwrap();
        // Lease dropped HERE, before the submit below.
    }

    // While the encoder is open, the released buffer must be pending, never free.
    assert_eq!(
        arena.stats().cached_buffers,
        0,
        "a buffer released while an encoder is open must not be reusable yet"
    );

    // Acquiring now must NOT return that buffer.
    let other = arena.acquire(shape.output_shape(fd)).unwrap();
    assert_eq!(
        arena.stats().reuse_hits,
        0,
        "the pending buffer was recycled while an encoder still referenced it"
    );
    drop(other);

    session.submit_and_wait().unwrap();
    arena.reclaim_all().unwrap();
    assert!(
        arena.stats().cached_buffers > 0,
        "buffers should be reclaimable after submit"
    );
}

/// Several submits in sequence, with the watermark advancing across them.
#[test]
fn test_multiple_sequential_submits_advance_the_watermark() {
    let Some(backend) = backend_or_skip("submits") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let shape = TensorShape::new(512, 4);

    let mut epochs = Vec::new();
    for _ in 0..4 {
        let t = arena.acquire(shape).unwrap();
        let session = arena.session(None);
        // A trivial but real command so the submit is not empty.
        backend
            .resources()
            .write_tensor(&t, &vec![1.0f32; 2048])
            .unwrap();

        epochs.push(session.submit().1);
    }

    for w in epochs.windows(2) {
        assert!(
            w[1] > w[0],
            "epochs must increase: {:?} then {:?}",
            w[0],
            w[1]
        );
    }
    arena.reclaim_all().unwrap();
    let done = arena.tracker().completed();
    assert!(
        epochs.last().unwrap().is_complete_at(done),
        "after reclaim_all the last epoch must be complete"
    );
}

// ---------------------------------------------------------------------------
// budget and eviction
// ---------------------------------------------------------------------------

/// A byte budget must bound the free list, and must evict only free buffers.
#[test]
fn test_byte_budget_evicts_and_never_touches_live_leases() {
    let Some(backend) = backend_or_skip("budget") else {
        return;
    };
    let shape = TensorShape::new(1024, 1); // 4096 B each
    let arena = backend.tensor_arena(TensorArenaConfig {
        max_cached_bytes: 8192, // room for exactly two
        max_cached_buffers: 64,
        reuse_policy: ReusePolicy::ExactSize,
    });

    let live = arena.acquire(shape).unwrap();
    // Five buffers alive at once, then all released together. Releasing them
    // one at a time would reuse the same cached slab and never exceed the
    // budget -- the ceiling is on the free list, not on total churn.
    {
        let _batch: Vec<_> = (0..5).map(|_| arena.acquire(shape).unwrap()).collect();
    }
    arena.poll_completion().unwrap();

    let s = arena.stats();
    assert!(
        s.cached_bytes <= 8192,
        "free list exceeded the byte budget: {} B",
        s.cached_bytes
    );
    assert!(s.evictions > 0, "budget was never enforced");
    assert!(
        s.active_bytes >= 4096,
        "the live lease was evicted; only free buffers may be evicted"
    );
    // The live lease still works.
    backend
        .resources()
        .write_tensor(&live, &vec![7.0f32; 1024])
        .unwrap();
    backend.wait(backend.submit(backend.encoder(None))).unwrap();
    assert!(
        backend
            .read_tensor(&live)
            .unwrap()
            .iter()
            .all(|v| *v == 7.0)
    );
}

/// The buffer-count ceiling is enforced independently of the byte ceiling.
#[test]
fn test_max_cached_buffers_is_enforced() {
    let Some(backend) = backend_or_skip("count") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig {
        max_cached_bytes: u64::MAX,
        max_cached_buffers: 2,
        reuse_policy: ReusePolicy::ExactSize,
    });

    // Held simultaneously, then all released: releasing one at a time would
    // simply reuse the single cached buffer and never reach the ceiling.
    // Distinct sizes so they occupy distinct buckets.
    {
        let _leases: Vec<_> = [16usize, 32, 64, 128, 256]
            .into_iter()
            .map(|n| arena.acquire(TensorShape::new(n, 1)).unwrap())
            .collect();
    }
    arena.poll_completion().unwrap();

    let s = arena.stats();
    assert!(
        s.cached_buffers <= 2,
        "free list held {} buffers against a ceiling of 2",
        s.cached_buffers
    );
    assert!(s.evictions > 0);
}

/// `trim` releases cached memory without disturbing live leases.
#[test]
fn test_trim_clears_cache_only() {
    let Some(backend) = backend_or_skip("trim") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let shape = TensorShape::new(256, 4);

    let live = arena.acquire(shape).unwrap();
    drop(arena.acquire(shape).unwrap());
    arena.poll_completion().unwrap();
    assert!(arena.stats().cached_buffers > 0);

    arena.trim();
    let s = arena.stats();
    assert_eq!(s.cached_buffers, 0);
    assert_eq!(s.cached_bytes, 0);
    assert!(s.active_bytes > 0, "trim must not release live leases");
    drop(live);
}

// ---------------------------------------------------------------------------
// device identity
// ---------------------------------------------------------------------------

/// An arena belongs to exactly one backend. Two independently built backends are
/// distinct devices, and the arena must say so.
#[test]
fn test_arena_is_bound_to_one_backend() {
    let Some(first) = backend_or_skip("device_a") else {
        return;
    };
    let Some(second) = backend_or_skip("device_b") else {
        return;
    };
    let arena = first.tensor_arena(TensorArenaConfig::default());

    assert!(arena.owns_device(&first));
    assert!(!arena.owns_device(&second));
    assert!(arena.ensure_same_device(&first).is_ok());
    assert!(matches!(
        arena.ensure_same_device(&second),
        Err(ComputeError::Backend { .. })
    ));
}

// ---------------------------------------------------------------------------
// numerical equivalence against the non-arena path
// ---------------------------------------------------------------------------

/// SpMM -> activation through the arena must equal the same chain on freshly
/// allocated buffers, bit for bit.
#[test]
fn test_spmm_activation_chain_matches_non_arena_path() {
    let Some(backend) = backend_or_skip("chain_equiv") else {
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

    // Reference: Phase 3 path, fresh buffers.
    let reference = {
        let mid = res.zeroed_tensor("mid", shape.output_shape(fd)).unwrap();
        let spmm = backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&input)
            .output(&mid)
            .prepare()
            .unwrap();
        let act = backend
            .operations()
            .activation()
            .kind(ActivationKind::Relu)
            .input(&mid)
            .in_place(&mid)
            .prepare()
            .unwrap();
        let mut enc = backend.encoder(None);
        spmm.encode(&mut enc).unwrap();
        act.encode(&mut enc).unwrap();
        backend.wait(backend.submit(enc)).unwrap();
        backend.read_tensor(&mid).unwrap()
    };

    // Arena path, on a buffer deliberately poisoned first.
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let got = {
        let mid = arena.acquire(shape.output_shape(fd)).unwrap();
        res.write_tensor(&mid, &vec![POISON; csr.num_rows * fd])
            .unwrap();
        let spmm = backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&input)
            .output(&mid)
            .prepare()
            .unwrap();
        let act = backend
            .operations()
            .activation()
            .kind(ActivationKind::Relu)
            .input(&mid)
            .in_place(&mid)
            .prepare()
            .unwrap();
        let mut session = arena.session(None);
        spmm.encode(&mut session).unwrap();
        act.encode(&mut session).unwrap();
        session.submit_and_wait().unwrap();
        arena.reclaim_all().unwrap();
        backend.read_tensor(&mid).unwrap()
    };

    assert_eq!(
        got.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        reference.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        "arena path diverged from the freshly-allocated reference"
    );
    assert!(
        !got.contains(&POISON),
        "poison survived into the result; the output was not fully written"
    );
}

/// An 8-layer resident chain — the workload the arena exists for.
///
/// Runs twice: once with a distinct buffer per layer, once through the arena,
/// and requires the results to be identical while the arena creates far fewer
/// buffers.
#[test]
fn test_eight_layer_chain_matches_and_reuses() {
    let Some(backend) = backend_or_skip("eight_layer") else {
        return;
    };
    const LAYERS: usize = 8;
    let spec = SparseSpec::small();
    let csr = spec.build();
    let fd = 16;
    let res = backend.resources();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    // Square-ish so the output of one layer can feed the next.
    assert_eq!(
        csr.num_rows, csr.num_cols,
        "the chain test needs a square matrix to feed layer n+1 from layer n"
    );
    let x0 = spec.dense_block(fd);
    let input = res.upload_tensor("x", shape.input_shape(fd), &x0).unwrap();

    // --- reference: one fresh buffer per layer -----------------------------
    let reference = {
        let mut buffers = Vec::new();
        for i in 0..LAYERS {
            buffers.push(
                res.zeroed_tensor(&format!("h{i}"), shape.output_shape(fd))
                    .unwrap(),
            );
        }
        let mut enc = backend.encoder(Some("naive"));
        let mut ops = Vec::new();
        for i in 0..LAYERS {
            let src = if i == 0 { &input } else { &buffers[i - 1] };
            let spmm = backend
                .operations()
                .spmm()
                .matrix(&matrix)
                .input(src)
                .output(&buffers[i])
                .prepare()
                .unwrap();
            let act = backend
                .operations()
                .activation()
                .kind(ActivationKind::Relu)
                .input(&buffers[i])
                .in_place(&buffers[i])
                .prepare()
                .unwrap();
            ops.push((spmm, act));
        }
        for (spmm, act) in &ops {
            spmm.encode(&mut enc).unwrap();
            act.encode(&mut enc).unwrap();
        }
        backend.wait(backend.submit(enc)).unwrap();
        backend.read_tensor(&buffers[LAYERS - 1]).unwrap()
    };

    // --- arena: ping-pong across two leases --------------------------------
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let got = {
        let a = arena.acquire(shape.output_shape(fd)).unwrap();
        let b = arena.acquire(shape.output_shape(fd)).unwrap();
        // Poison both, so any unwritten element shows up as POISON.
        res.write_tensor(&a, &vec![POISON; csr.num_rows * fd])
            .unwrap();
        res.write_tensor(&b, &vec![POISON; csr.num_rows * fd])
            .unwrap();

        let mut session = arena.session(Some("arena-chain"));
        let mut ops = Vec::new();
        for i in 0..LAYERS {
            // Layer 0 reads the caller-owned input; every later layer reads a
            // lease. The two have different scopes, so the source cannot be one
            // variable -- which is the type system making the provenance
            // difference visible rather than letting it blur.
            let (src, dst) = match i {
                0 => (None, &a),
                _ if i % 2 == 1 => (Some(&a), &b),
                _ => (Some(&b), &a),
            };
            let spmm = match src {
                None => backend
                    .operations()
                    .spmm()
                    .matrix(&matrix)
                    .input(&input)
                    .output(dst)
                    .prepare()
                    .unwrap(),
                Some(s) => backend
                    .operations()
                    .spmm()
                    .matrix(&matrix)
                    .input(s)
                    .output(dst)
                    .prepare()
                    .unwrap(),
            };
            let act = backend
                .operations()
                .activation()
                .kind(ActivationKind::Relu)
                .input(dst)
                .in_place(dst)
                .prepare()
                .unwrap();
            ops.push((spmm, act));
        }
        for (spmm, act) in &ops {
            spmm.encode(&mut session).unwrap();
            act.encode(&mut session).unwrap();
        }
        session.submit_and_wait().unwrap();
        arena.reclaim_all().unwrap();
        // Layer 0 writes `a`, then the parity alternates, so the final odd
        // layer (7) writes `b`.
        backend.read_tensor(&b).unwrap()
    };

    assert_eq!(
        got.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        reference.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        "8-layer arena chain diverged from the one-buffer-per-layer reference"
    );
    assert!(
        !got.contains(&POISON),
        "poison leaked into the chain output"
    );
    assert_eq!(
        arena.stats().buffers_created,
        2,
        "an 8-layer chain must need only the two ping-pong buffers"
    );
}

/// Repeated inferences on one arena must reuse rather than grow, and must keep
/// producing the same answer.
#[test]
fn test_repeated_chains_on_one_arena_reuse_and_stay_correct() {
    let Some(backend) = backend_or_skip("repeat") else {
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
    let arena = backend.tensor_arena(TensorArenaConfig::default());

    let mut results = Vec::new();
    for iteration in 0..5 {
        let out = arena.acquire(shape.output_shape(fd)).unwrap();
        res.write_tensor(&out, &vec![POISON; csr.num_rows * fd])
            .unwrap();
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
        session.submit_and_wait().unwrap();
        arena.reclaim_all().unwrap();
        let v = backend.read_tensor(&out).unwrap();
        assert!(!v.contains(&POISON), "iteration {iteration} leaked poison");
        results.push(v);
    }

    for (i, r) in results.iter().enumerate().skip(1) {
        assert_eq!(
            r.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            results[0].iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            "iteration {i} disagreed with iteration 0"
        );
    }

    assert_eq!(
        arena.stats().buffers_created,
        1,
        "five identical iterations should reuse one buffer"
    );
    assert_eq!(arena.stats().reuse_hits, 4);
}

/// Rewriting the input between dispatches must be observed: the bind group binds
/// buffers, not contents, and the arena must not have cached a stale result.
#[test]
fn test_input_rewrite_is_observed_through_the_arena() {
    let Some(backend) = backend_or_skip("rewrite") else {
        return;
    };
    let spec = SparseSpec::small();
    let csr = spec.build();
    let fd = 4;
    let res = backend.resources();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let input = res
        .upload_tensor("x", shape.input_shape(fd), &vec![1.0f32; csr.num_cols * fd])
        .unwrap();
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
    session.submit_and_wait().unwrap();
    arena.reclaim_all().unwrap();
    let first = backend.read_tensor(&out).unwrap();

    // Double the input; the same prepared op must now produce doubled output.
    res.write_tensor(&input, &vec![2.0f32; csr.num_cols * fd])
        .unwrap();
    let mut session = arena.session(None);
    spmm.encode(&mut session).unwrap();
    session.submit_and_wait().unwrap();
    arena.reclaim_all().unwrap();
    let second = backend.read_tensor(&out).unwrap();

    for (i, (a, b)) in first.iter().zip(second.iter()).enumerate() {
        assert!(
            (b - 2.0 * a).abs() <= 1e-4 * a.abs().max(1.0),
            "element {i}: {b} is not twice {a}; the rewrite was not observed"
        );
    }
}

/// The arena must not perturb pipeline caching — it deals in buffers, and
/// buffers are not part of the pipeline key.
#[test]
fn test_pipeline_cache_behaviour_is_unchanged_by_the_arena() {
    let Some(backend) = backend_or_skip("pipeline") else {
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
    let arena = backend.tensor_arena(TensorArenaConfig::default());

    assert!(backend.pipelines().is_empty());

    let first = arena.acquire(shape.output_shape(fd)).unwrap();
    let _op1 = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&first)
        .prepare()
        .unwrap();
    let after_first = backend.pipelines().len();
    assert_eq!(after_first, 1);

    let second = arena.acquire(shape.output_shape(fd)).unwrap();
    let _op2 = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&second)
        .prepare()
        .unwrap();
    assert_eq!(
        backend.pipelines().len(),
        after_first,
        "a different arena buffer must not create a new pipeline"
    );
}

// ---------------------------------------------------------------------------
// accounting
// ---------------------------------------------------------------------------

/// The statistics must add up; they are the basis of every memory claim in the
/// Phase 4 report, so they get asserted rather than trusted.
#[test]
fn test_stats_accounting_is_consistent() {
    let Some(backend) = backend_or_skip("stats") else {
        return;
    };
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let shape = TensorShape::new(1024, 2); // 8192 B
    let bytes = 1024 * 2 * 4;

    assert_eq!(arena.stats(), Default::default());

    let a = arena.acquire(shape).unwrap();
    let s = arena.stats();
    assert_eq!(s.active_bytes, bytes);
    assert_eq!(s.in_flight_bytes, 0);
    assert_eq!(s.cached_bytes, 0);
    assert_eq!(s.device_bytes(), bytes);
    assert_eq!(s.peak_arena_bytes, bytes);

    let b = arena.acquire(shape).unwrap();
    assert_eq!(arena.stats().active_bytes, 2 * bytes);
    assert_eq!(arena.stats().peak_arena_bytes, 2 * bytes);

    drop(a);
    drop(b);
    arena.poll_completion().unwrap();
    let s = arena.stats();
    assert_eq!(s.active_bytes, 0);
    assert_eq!(s.in_flight_bytes, 0);
    assert_eq!(s.cached_bytes, 2 * bytes);
    assert_eq!(s.cached_buffers, 2);
    assert_eq!(
        s.peak_arena_bytes,
        2 * bytes,
        "peak must not decay when buffers move between states"
    );
    assert_eq!(s.hit_rate(), Some(0.0));
    assert_eq!(s.total_allocated_bytes, 2 * bytes);
}
