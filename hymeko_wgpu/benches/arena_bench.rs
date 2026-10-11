//! Phase 4 §9: what the arena costs.
//!
//! The arena exists for memory (§8 measured 4× on the 8-layer chain). This file
//! answers the other half: **does it cost runtime?**
//!
//! Two acceptance targets from the brief:
//!
//! 1. a cache-hit `acquire`/release must be sub-microsecond, or negligible
//!    against the ~80 µs submit floor Phase 3 measured;
//! 2. the resident chain must not regress by more than ~3 %.
//!
//! # Cells
//!
//! | id | contents |
//! |---|---|
//! | `acquire/hit` | acquire + release with a warm free list |
//! | `acquire/miss` | acquire + release with an empty free list (creates a buffer) |
//! | `acquire/poll` | `poll_completion` alone — the deferred-recycling tax |
//! | `chain/naive` | 8 layers, one fresh buffer per layer |
//! | `chain/pingpong` | 8 layers, two hand-managed buffers |
//! | `chain/arena` | 8 layers through the arena |
//! | `encode/*` | encoding only, no submit — isolates host-side cost |
//! | `inference/*` | repeated chains, which is where reuse should pay |
//!
//! `naive`, `pingpong` and `arena` are measured in the **same process, in
//! sequence**, so the comparison that decides acceptance is within-run. Phase 3
//! established that cross-run deltas on this host carry ~3 % confidence
//! intervals and cannot settle a question of this size.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hymeko_tensor::compute::TensorShape;
use hymeko_wgpu::testing::SparseSpec;
use hymeko_wgpu::testing::{Fixture, encode_chain};
use hymeko_wgpu::{ActivationKind, GpuTensor, TensorArenaConfig, WgpuBackend};
use std::hint::black_box;

const LAYERS: usize = 8;

fn backend() -> Option<WgpuBackend> {
    match WgpuBackend::builder().label("arena-bench").build() {
        Ok(b) => Some(b),
        Err(e) => {
            eprintln!("SKIPPING arena benchmarks - no GPU adapter: {e}");
            None
        }
    }
}

fn tune(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>, heavy: bool) {
    if heavy {
        group
            .sample_size(10)
            .measurement_time(Duration::from_secs(8));
    } else {
        group
            .sample_size(30)
            .measurement_time(Duration::from_secs(5));
    }
}

// ---------------------------------------------------------------------------
// lease overhead
// ---------------------------------------------------------------------------

fn bench_lease(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("acquire");
    group
        .sample_size(100)
        .measurement_time(Duration::from_secs(5));

    // A shape big enough to be realistic, small enough that allocation cost is
    // the buffer object rather than the memory.
    let shape = TensorShape::new(65_536, 4);

    // Hit: the free list already holds a matching slab, because the previous
    // iteration released one and nothing was ever submitted.
    {
        let arena = backend.tensor_arena(TensorArenaConfig::default());
        drop(arena.acquire(shape).unwrap());
        arena.poll_completion().unwrap();
        group.bench_function("hit", |b| {
            b.iter(|| {
                let t = arena.acquire(black_box(shape)).unwrap();
                black_box(t.allocation_bytes())
                // released here
            });
        });
    }

    // Miss: every iteration allocates, because the arena is told to cache
    // nothing. This is the cost the hit path avoids.
    {
        let arena = backend.tensor_arena(TensorArenaConfig {
            max_cached_bytes: 0,
            max_cached_buffers: 0,
            ..TensorArenaConfig::default()
        });
        group.bench_function("miss", |b| {
            b.iter(|| {
                let t = arena.acquire(black_box(shape)).unwrap();
                black_box(t.allocation_bytes())
            });
        });
    }

    // The non-blocking poll on its own.
    {
        let arena = backend.tensor_arena(TensorArenaConfig::default());
        group.bench_function("poll", |b| {
            b.iter(|| black_box(arena.poll_completion().unwrap()));
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// the 8-layer chain, three ways
// ---------------------------------------------------------------------------

/// Build a device fixture from a workload spec.
///
/// The spec-to-CSR step stays here rather than in `testing::Fixture`, because
/// `SparseSpec` lives in `hymeko_hnn`, which the library must never depend on.
fn fixture(backend: &WgpuBackend, spec: &SparseSpec, fd: usize) -> Fixture {
    Fixture::build(backend, &spec.build(), &spec.dense_block(fd), fd)
}

/// Encode an 8-layer chain over two pooled leases, alternating.
///
/// A separate function from `encode_pingpong` because the operands are
/// `ArenaTensor`, which makes the operations `Tracked`, which makes their
/// `encode` take the session rather than a raw encoder. That is the type-level
/// guarantee working as designed; the duplication is its visible cost.
/// Encode an 8-layer chain over the two supplied buffers, alternating.
fn encode_pingpong(
    backend: &WgpuBackend,
    fx: &Fixture,
    a: &GpuTensor<f32>,
    b: &GpuTensor<f32>,
    enc: &mut wgpu::CommandEncoder,
) {
    let ops: Vec<_> = (0..LAYERS)
        .map(|i| {
            let (src, dst) = match i {
                0 => (&fx.input, a),
                _ if i % 2 == 1 => (a, b),
                _ => (b, a),
            };
            let spmm = backend
                .operations()
                .spmm()
                .matrix(&fx.matrix)
                .input(src)
                .output(dst)
                .prepare()
                .unwrap();
            let act = backend
                .operations()
                .activation()
                .kind(ActivationKind::Relu)
                .input(dst)
                .in_place(dst)
                .prepare()
                .unwrap();
            (spmm, act)
        })
        .collect();
    for (s, act) in &ops {
        s.encode(enc).unwrap();
        act.encode(enc).unwrap();
    }
}

fn bench_chain(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("chain");

    for (label, spec) in [
        ("small", SparseSpec::small()),
        ("medium", SparseSpec::medium()),
        ("large", SparseSpec::large()),
    ] {
        for fd in [8usize, 32, 128] {
            let fx = fixture(&backend, &spec, fd);
            let heavy = label == "large" || fd >= 128;
            tune(&mut group, heavy);
            group.throughput(Throughput::Elements((fx.csr_nnz * fd * LAYERS) as u64));
            let id = format!("{label}/fd{fd}");
            let res = backend.resources();

            // --- naive: one buffer per layer, allocated once outside the loop
            // so the comparison is about execution, not allocation.
            let naive: Vec<_> = (0..LAYERS)
                .map(|i| res.zeroed_tensor(&format!("h{i}"), fx.out_shape).unwrap())
                .collect();
            group.bench_function(BenchmarkId::new("naive", &id), |b| {
                b.iter(|| {
                    let mut enc = backend.encoder(None);
                    let ops: Vec<_> = (0..LAYERS)
                        .map(|i| {
                            let src = if i == 0 { &fx.input } else { &naive[i - 1] };
                            let spmm = backend
                                .operations()
                                .spmm()
                                .matrix(&fx.matrix)
                                .input(src)
                                .output(&naive[i])
                                .prepare()
                                .unwrap();
                            let act = backend
                                .operations()
                                .activation()
                                .kind(ActivationKind::Relu)
                                .input(&naive[i])
                                .in_place(&naive[i])
                                .prepare()
                                .unwrap();
                            (spmm, act)
                        })
                        .collect();
                    for (s, act) in &ops {
                        s.encode(&mut enc).unwrap();
                        act.encode(&mut enc).unwrap();
                    }
                    backend.wait(backend.submit(enc)).unwrap();
                });
            });

            // --- pingpong: the hand-written minimum
            let pa = res.zeroed_tensor("a", fx.out_shape).unwrap();
            let pb = res.zeroed_tensor("b", fx.out_shape).unwrap();
            group.bench_function(BenchmarkId::new("pingpong", &id), |b| {
                b.iter(|| {
                    let mut enc = backend.encoder(None);
                    encode_pingpong(&backend, &fx, &pa, &pb, &mut enc);
                    backend.wait(backend.submit(enc)).unwrap();
                });
            });

            // --- arena: acquire and release every iteration, which is the
            // pattern an inference loop actually uses.
            let arena = backend.tensor_arena(TensorArenaConfig::default());
            group.bench_function(BenchmarkId::new("arena", &id), |b| {
                b.iter(|| {
                    let a = arena.acquire(fx.out_shape).unwrap();
                    let bb = arena.acquire(fx.out_shape).unwrap();
                    let mut session = arena.session(None);
                    encode_chain(
                        &backend,
                        &fx,
                        &a,
                        &bb,
                        LAYERS,
                        ActivationKind::Relu,
                        &mut session,
                    );
                    session.submit_and_wait().unwrap();
                });
            });
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// encode-only: isolates host cost from device execution
// ---------------------------------------------------------------------------

fn bench_encode(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("encode");
    group
        .sample_size(30)
        .measurement_time(Duration::from_secs(5));

    let spec = SparseSpec::medium();
    for fd in [8usize, 128] {
        let fx = fixture(&backend, &spec, fd);
        let res = backend.resources();
        let id = format!("medium/fd{fd}");

        let pa = res.zeroed_tensor("a", fx.out_shape).unwrap();
        let pb = res.zeroed_tensor("b", fx.out_shape).unwrap();
        group.bench_function(BenchmarkId::new("pingpong", &id), |b| {
            b.iter(|| {
                let mut enc = backend.encoder(None);
                encode_pingpong(&backend, &fx, &pa, &pb, &mut enc);
                black_box(&enc);
            });
        });

        let arena = backend.tensor_arena(TensorArenaConfig::default());
        group.bench_function(BenchmarkId::new("arena", &id), |b| {
            b.iter(|| {
                let a = arena.acquire(fx.out_shape).unwrap();
                let bb = arena.acquire(fx.out_shape).unwrap();
                let mut session = arena.session(None);
                encode_chain(
                    &backend,
                    &fx,
                    &a,
                    &bb,
                    LAYERS,
                    ActivationKind::Relu,
                    &mut session,
                );
                black_box(session.operations_encoded());
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// repeated inference: where reuse is supposed to pay
// ---------------------------------------------------------------------------

fn bench_inference(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("inference");
    group
        .sample_size(10)
        .measurement_time(Duration::from_secs(8));
    const ROUNDS: usize = 4;

    let spec = SparseSpec::medium();
    for fd in [32usize, 128] {
        let fx = fixture(&backend, &spec, fd);
        let res = backend.resources();
        let id = format!("medium/fd{fd}");

        // Allocating fresh buffers every round is what a caller without an arena
        // does when the chain shape is not known in advance.
        group.bench_function(BenchmarkId::new("fresh_alloc", &id), |b| {
            b.iter(|| {
                for _ in 0..ROUNDS {
                    let a = res.zeroed_tensor("a", fx.out_shape).unwrap();
                    let bb = res.zeroed_tensor("b", fx.out_shape).unwrap();
                    let mut enc = backend.encoder(None);
                    encode_pingpong(&backend, &fx, &a, &bb, &mut enc);
                    backend.wait(backend.submit(enc)).unwrap();
                }
            });
        });

        let arena = backend.tensor_arena(TensorArenaConfig::default());
        group.bench_function(BenchmarkId::new("arena", &id), |b| {
            b.iter(|| {
                for _ in 0..ROUNDS {
                    let a = arena.acquire(fx.out_shape).unwrap();
                    let bb = arena.acquire(fx.out_shape).unwrap();
                    let mut session = arena.session(None);
                    encode_chain(
                        &backend,
                        &fx,
                        &a,
                        &bb,
                        LAYERS,
                        ActivationKind::Relu,
                        &mut session,
                    );
                    session.submit_and_wait().unwrap();
                }
            });
        });
    }
    group.finish();
}

criterion_group!(
    arena_benches,
    bench_lease,
    bench_session,
    bench_chain,
    bench_encode,
    bench_inference,
    bench_fanout,
    bench_encode_only
);
criterion_main!(arena_benches);

// ---------------------------------------------------------------------------
// Phase 5: tracked-session overhead and fan-out
// ---------------------------------------------------------------------------

fn bench_session(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("session");
    group
        .sample_size(100)
        .measurement_time(Duration::from_secs(5));

    let arena = backend.tensor_arena(TensorArenaConfig::default());

    // Creating and abandoning a session: the bookkeeping cost with no GPU work.
    group.bench_function("create_drop", |b| {
        b.iter(|| {
            let s = arena.session(None);
            black_box(s.is_empty())
        });
    });

    // Creating and submitting an empty session: adds the epoch issue, the
    // completion-callback registration, and an empty command buffer.
    group.bench_function("create_submit", |b| {
        b.iter(|| {
            let s = arena.session(None);
            black_box(s.submit().1)
        });
    });

    // Same-session acquire, which is the fan-out path.
    let shape = TensorShape::new(65_536, 4);
    group.bench_function("session_acquire", |b| {
        b.iter(|| {
            let mut s = arena.session(None);
            let t = s.acquire(black_box(shape)).unwrap();
            black_box(t.allocation_bytes())
        });
    });

    group.finish();
}

/// Fan-out: SpMM into a shared tensor, then two independent activation branches.
///
/// The merge stage is deliberately **not** benchmarked: its kernel is test-only
/// (`tests/shaders/merge_add.wgsl`), and duplicating it here to add one dispatch
/// would widen the crate's kernel surface for a number that measures the merge,
/// not the arena. The liveness claim the merge exists to prove is asserted in
/// `tests/diamond.rs`; what this cell measures is the *overhead* of running a
/// branching topology through sessions rather than raw encoders.
fn bench_fanout(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("fanout");

    for (label, spec) in [
        ("medium", SparseSpec::medium()),
        ("large", SparseSpec::large()),
    ] {
        for fd in [32usize, 128] {
            let fx = fixture(&backend, &spec, fd);
            tune(&mut group, label == "large" || fd >= 128);
            group.throughput(Throughput::Elements((fx.csr_nnz * fd) as u64));
            let id = format!("{label}/fd{fd}");
            let res = backend.resources();

            // Manual: three caller-owned buffers, raw encoder.
            let shared = res.zeroed_tensor("shared", fx.out_shape).unwrap();
            let left = res.zeroed_tensor("left", fx.out_shape).unwrap();
            let right = res.zeroed_tensor("right", fx.out_shape).unwrap();
            group.bench_function(BenchmarkId::new("manual", &id), |b| {
                b.iter(|| {
                    let spmm = backend
                        .operations()
                        .spmm()
                        .matrix(&fx.matrix)
                        .input(&fx.input)
                        .output(&shared)
                        .prepare()
                        .unwrap();
                    let relu = backend
                        .operations()
                        .activation()
                        .kind(ActivationKind::Relu)
                        .input(&shared)
                        .output(&left)
                        .prepare()
                        .unwrap();
                    let leaky = backend
                        .operations()
                        .activation()
                        .kind(ActivationKind::LeakyRelu { alpha: 0.01 })
                        .input(&shared)
                        .output(&right)
                        .prepare()
                        .unwrap();
                    let mut enc = backend.encoder(None);
                    spmm.encode(&mut enc).unwrap();
                    relu.encode(&mut enc).unwrap();
                    leaky.encode(&mut enc).unwrap();
                    backend.wait(backend.submit(enc)).unwrap();
                });
            });

            // Arena: leases acquired and released each iteration, tracked session.
            let arena = backend.tensor_arena(TensorArenaConfig::default());
            group.bench_function(BenchmarkId::new("arena", &id), |b| {
                b.iter(|| {
                    let a_shared = arena.acquire(fx.out_shape).unwrap();
                    let a_left = arena.acquire(fx.out_shape).unwrap();
                    let a_right = arena.acquire(fx.out_shape).unwrap();
                    let mut session = arena.session(None);
                    let spmm = backend
                        .operations()
                        .spmm()
                        .matrix(&fx.matrix)
                        .input(&fx.input)
                        .output(&a_shared)
                        .prepare()
                        .unwrap();
                    let relu = backend
                        .operations()
                        .activation()
                        .kind(ActivationKind::Relu)
                        .input(&a_shared)
                        .output(&a_left)
                        .prepare()
                        .unwrap();
                    let leaky = backend
                        .operations()
                        .activation()
                        .kind(ActivationKind::LeakyRelu { alpha: 0.01 })
                        .input(&a_shared)
                        .output(&a_right)
                        .prepare()
                        .unwrap();
                    spmm.encode(&mut session).unwrap();
                    relu.encode(&mut session).unwrap();
                    leaky.encode(&mut session).unwrap();
                    session.submit_and_wait().unwrap();
                });
            });
        }
    }
    group.finish();
}

/// Pure `encode` cost, with `prepare` hoisted out of the measured loop.
///
/// The `encode/*` group above measures prepare+encode together, which is what a
/// caller pays per iteration but *not* what Phase 3's 0.62 µs/op figure refers
/// to. Phase 3 measured recording a dispatch into an encoder for an
/// already-prepared operation, and that is the number the Phase 5 acceptance
/// criterion is stated against — so it is measured the same way here.
///
/// Divide by `LAYERS * 2` for per-operation cost.
fn bench_encode_only(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("encode_only");
    group
        .sample_size(100)
        .measurement_time(Duration::from_secs(5));

    let spec = SparseSpec::medium();
    let fd = 32;
    let fx = fixture(&backend, &spec, fd);
    let res = backend.resources();

    // --- untracked: caller-owned buffers, raw encoder -----------------------
    let pa = res.zeroed_tensor("a", fx.out_shape).unwrap();
    let pb = res.zeroed_tensor("b", fx.out_shape).unwrap();
    let plain_ops: Vec<_> = (0..LAYERS)
        .map(|i| {
            let (src, dst) = match i {
                0 => (&fx.input, &pa),
                _ if i % 2 == 1 => (&pa, &pb),
                _ => (&pb, &pa),
            };
            let spmm = backend
                .operations()
                .spmm()
                .matrix(&fx.matrix)
                .input(src)
                .output(dst)
                .prepare()
                .unwrap();
            let act = backend
                .operations()
                .activation()
                .kind(ActivationKind::Relu)
                .in_place(dst)
                .prepare()
                .unwrap();
            (spmm, act)
        })
        .collect();

    group.bench_function("untracked", |b| {
        b.iter(|| {
            let mut enc = backend.encoder(None);
            for (s, a) in &plain_ops {
                s.encode(&mut enc).unwrap();
                a.encode(&mut enc).unwrap();
            }
            black_box(&enc);
        });
    });

    // --- tracked: pooled buffers, session ----------------------------------
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let la = arena.acquire(fx.out_shape).unwrap();
    let lb = arena.acquire(fx.out_shape).unwrap();
    let pooled_ops: Vec<_> = (0..LAYERS)
        .map(|i| {
            let (src, dst) = match i {
                0 => (None, &la),
                _ if i % 2 == 1 => (Some(&la), &lb),
                _ => (Some(&lb), &la),
            };
            let spmm = match src {
                None => backend
                    .operations()
                    .spmm()
                    .matrix(&fx.matrix)
                    .input(&fx.input)
                    .output(dst)
                    .prepare()
                    .unwrap(),
                Some(s) => backend
                    .operations()
                    .spmm()
                    .matrix(&fx.matrix)
                    .input(s)
                    .output(dst)
                    .prepare()
                    .unwrap(),
            };
            let act = backend
                .operations()
                .activation()
                .kind(ActivationKind::Relu)
                .in_place(dst)
                .prepare()
                .unwrap();
            (spmm, act)
        })
        .collect();

    group.bench_function("tracked", |b| {
        b.iter(|| {
            let mut session = arena.session(None);
            for (s, a) in &pooled_ops {
                s.encode(&mut session).unwrap();
                a.encode(&mut session).unwrap();
            }
            black_box(session.operations_encoded());
        });
    });

    group.finish();
}
