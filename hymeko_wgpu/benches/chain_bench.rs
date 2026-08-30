//! Phase 3: activation cells, SpMM→activation composition, and the intermediate
//! crossover sweep.
//!
//! # Cells
//!
//! | id | contents |
//! |---|---|
//! | `A_cpu_activation` | CPU elementwise, single-threaded |
//! | `B_gpu_activation_submit` | GPU activation, own encoder + submit + wait |
//! | `C_gpu_activation_encoded` | GPU activation encoded into a shared encoder |
//! | `D_spmm_readback` | SpMM + readback |
//! | `E_two_submits_readback` | SpMM submit, activation submit, readback |
//! | `F_one_submit_readback` | SpMM + activation, one submit, readback |
//! | `G_one_submit_resident` | SpMM + activation, one submit, **no readback** |
//!
//! `E` vs `F` isolates the cost of a second submit. `F` vs `G` isolates
//! readback. `G` is the number that matters for a chain that stays on device.
//!
//! Cell `H` (fused SpMM+activation kernel) lives in `fused_bench.rs` and is only
//! meaningful against `G` measured here.
//!
//! Run: `cargo bench -p hymeko_wgpu --bench chain_bench --offline`

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use hymeko::tensor::compute::TensorShape;
use hymeko_wgpu::testing::{DegreeShape, SparseSpec};
use hymeko_wgpu::{ActivationKind, ComputeError, WgpuBackend, WorkgroupShape};

fn backend() -> Option<WgpuBackend> {
    match WgpuBackend::builder().label("hymeko-chain-bench").build() {
        Ok(b) => {
            eprintln!("GPU: {}", b.capabilities().summary());
            Some(b)
        }
        Err(ComputeError::BackendUnavailable { reason, .. }) => {
            eprintln!("GPU cells SKIPPED - no adapter: {reason}");
            None
        }
        Err(e) => panic!("backend init failed: {e}"),
    }
}

fn scales() -> [(&'static str, SparseSpec); 3] {
    [
        ("small", SparseSpec::small()),
        ("medium", SparseSpec::medium()),
        ("large", SparseSpec::large()),
    ]
}

const FEATURE_DIMS: [usize; 4] = [1, 8, 32, 128];

fn tune(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    label: &str,
    fd: usize,
) {
    if label == "large" || fd >= 128 {
        group
            .sample_size(10)
            .measurement_time(Duration::from_secs(8));
    } else {
        group
            .sample_size(30)
            .measurement_time(Duration::from_secs(5));
    }
}

/// CPU elementwise reference for cell A.
fn cpu_relu_into(src: &[f32], dst: &mut [f32]) {
    for (d, &s) in dst.iter_mut().zip(src.iter()) {
        *d = if s > 0.0 { s } else { 0.0 };
    }
}

// ---------------------------------------------------------------------------
// A / B / C — activation in isolation
// ---------------------------------------------------------------------------

fn bench_activation(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("act");

    for (label, spec) in scales() {
        let csr = spec.build();
        for fd in FEATURE_DIMS {
            let elements = csr.num_rows * fd;
            let host: Vec<f32> = (0..elements).map(|i| (i % 211) as f32 - 105.0).collect();
            let shape = TensorShape::new(csr.num_rows, fd);

            let res = backend.resources();
            let input = res.upload_tensor("in", shape, &host).unwrap();
            let output = res.zeroed_tensor("out", shape).unwrap();
            let op = backend
                .operations()
                .activation()
                .kind(ActivationKind::Relu)
                .input(&input)
                .output(&output)
                .prepare()
                .unwrap();

            let mut cpu_dst = vec![0.0f32; elements];

            group.throughput(Throughput::Elements(elements as u64));
            tune(&mut group, label, fd);
            let id = format!("{label}/fd{fd}");

            group.bench_function(BenchmarkId::new("A_cpu_activation", &id), |b| {
                b.iter(|| cpu_relu_into(black_box(&host), black_box(&mut cpu_dst)));
            });

            // Own encoder + submit + wait: the cost of a standalone GPU op.
            group.bench_function(BenchmarkId::new("B_gpu_activation_submit", &id), |b| {
                b.iter(|| {
                    let mut enc = backend.encoder(None);
                    op.encode(&mut enc).unwrap();
                    backend.wait(backend.submit(enc)).unwrap();
                });
            });

            // Encode only: the marginal cost of adding this op to an encoder
            // that is going to be submitted anyway.
            group.bench_function(BenchmarkId::new("C_gpu_activation_encoded", &id), |b| {
                b.iter(|| {
                    let mut enc = backend.encoder(None);
                    op.encode(black_box(&mut enc)).unwrap();
                    black_box(&enc);
                });
            });
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// D / E / F / G — composition
// ---------------------------------------------------------------------------

fn bench_chain(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("chain");

    for (label, spec) in scales() {
        let csr = spec.build();
        let matrix = backend.resources().upload_csr_from("a", &csr).unwrap();
        let shape = matrix.shape();

        for fd in FEATURE_DIMS {
            let x = spec.dense_block(fd);
            let res = backend.resources();
            let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
            let mid = res.zeroed_tensor("mid", shape.output_shape(fd)).unwrap();
            let out = res.zeroed_tensor("out", shape.output_shape(fd)).unwrap();

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
                .output(&out)
                .prepare()
                .unwrap();

            group.throughput(Throughput::Elements((csr.nnz() * fd) as u64));
            tune(&mut group, label, fd);
            let id = format!("{label}/fd{fd}");

            // D: SpMM alone + readback.
            group.bench_function(BenchmarkId::new("D_spmm_readback", &id), |b| {
                b.iter(|| {
                    let mut enc = backend.encoder(None);
                    spmm.encode(&mut enc).unwrap();
                    backend.wait(backend.submit(enc)).unwrap();
                    black_box(backend.read_tensor(&mid).unwrap())
                });
            });

            // E: two separate submits, then readback.
            group.bench_function(BenchmarkId::new("E_two_submits_readback", &id), |b| {
                b.iter(|| {
                    let mut e1 = backend.encoder(None);
                    spmm.encode(&mut e1).unwrap();
                    backend.wait(backend.submit(e1)).unwrap();

                    let mut e2 = backend.encoder(None);
                    act.encode(&mut e2).unwrap();
                    backend.wait(backend.submit(e2)).unwrap();

                    black_box(backend.read_tensor(&out).unwrap())
                });
            });

            // F: both dispatches in ONE submit, then readback.
            group.bench_function(BenchmarkId::new("F_one_submit_readback", &id), |b| {
                b.iter(|| {
                    let mut enc = backend.encoder(None);
                    spmm.encode(&mut enc).unwrap();
                    act.encode(&mut enc).unwrap();
                    backend.wait(backend.submit(enc)).unwrap();
                    black_box(backend.read_tensor(&out).unwrap())
                });
            });

            // G: one submit, result stays resident. The figure that matters for
            // a multi-layer chain.
            group.bench_function(BenchmarkId::new("G_one_submit_resident", &id), |b| {
                b.iter(|| {
                    let mut enc = backend.encoder(None);
                    spmm.encode(&mut enc).unwrap();
                    act.encode(&mut enc).unwrap();
                    backend.wait(backend.submit(enc)).unwrap();
                });
            });

            // H: the same result from ONE fused dispatch, resident.
            //
            // Directly comparable to G — same inputs, same output extent, same
            // residency, one dispatch instead of two. The difference is one
            // launch plus one full read+write of the intermediate.
            //
            // Measured only after the standalone activation cells above, so the
            // saving can be attributed rather than guessed at.
            let fused = backend
                .operations()
                .spmm()
                .matrix(&matrix)
                .input(&input)
                .output(&out)
                .fused_activation(ActivationKind::Relu)
                .prepare()
                .unwrap();
            group.bench_function(BenchmarkId::new("H_fused_resident", &id), |b| {
                b.iter(|| {
                    let mut enc = backend.encoder(None);
                    fused.encode(&mut enc).unwrap();
                    backend.wait(backend.submit(enc)).unwrap();
                });
            });
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Activation workgroup sweep: 64 / 128 / 256
// ---------------------------------------------------------------------------

fn bench_activation_workgroup(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("act_workgroup");

    for (label, spec) in [
        ("medium", SparseSpec::medium()),
        ("large", SparseSpec::large()),
    ] {
        let csr = spec.build();
        for fd in [8usize, 32] {
            let elements = csr.num_rows * fd;
            let host: Vec<f32> = (0..elements).map(|i| (i % 211) as f32 - 105.0).collect();
            let shape = TensorShape::new(csr.num_rows, fd);
            let res = backend.resources();
            let input = res.upload_tensor("in", shape, &host).unwrap();
            let output = res.zeroed_tensor("out", shape).unwrap();

            group.throughput(Throughput::Elements(elements as u64));
            tune(&mut group, label, fd);

            for wg in hymeko_wgpu::ACTIVATION_WORKGROUP_SWEEP {
                let op = backend
                    .operations()
                    .activation()
                    .kind(ActivationKind::Relu)
                    .input(&input)
                    .output(&output)
                    .workgroup(wg)
                    .prepare()
                    .unwrap();
                group.bench_function(
                    BenchmarkId::new(format!("wg{wg}"), format!("{label}/fd{fd}")),
                    |b| {
                        b.iter(|| {
                            let mut enc = backend.encoder(None);
                            op.encode(&mut enc).unwrap();
                            backend.wait(backend.submit(enc)).unwrap();
                        });
                    },
                );
            }
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Intermediate crossover sweep
// ---------------------------------------------------------------------------

/// Logarithmically spaced row counts between Phase 2's `small` (1 000) and
/// `medium` (50 000), which were 100x apart in nnz and left the crossover
/// bracketed but unlocated.
///
/// Same sparsity model and seed as the standard specs, so these points sit on
/// the same curve rather than forming a second, incomparable series.
fn crossover_specs() -> Vec<(String, SparseSpec)> {
    [
        1_500usize, 2_500, 4_000, 6_500, 10_000, 16_000, 25_000, 40_000,
    ]
    .into_iter()
    .map(|rows| {
        (
            format!("r{rows}"),
            SparseSpec {
                rows,
                cols: rows,
                avg_nnz_per_row: 16,
                shape: DegreeShape::Uniform,
                seed: 0x5EED_0002,
            },
        )
    })
    .collect()
}

fn bench_crossover(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("crossover");
    group
        .sample_size(30)
        .measurement_time(Duration::from_secs(5));

    for (label, spec) in crossover_specs() {
        let csr = spec.build();
        let matrix = backend.resources().upload_csr_from("a", &csr).unwrap();
        let shape = matrix.shape();

        for fd in [8usize, 32] {
            let x = spec.dense_block(fd);
            let res = backend.resources();
            let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
            let out = res.zeroed_tensor("y", shape.output_shape(fd)).unwrap();
            let mut cpu_y = vec![0.0f32; csr.num_rows * fd];

            let op = backend
                .operations()
                .spmm()
                .matrix(&matrix)
                .input(&input)
                .output(&out)
                .prepare()
                .unwrap();

            group.throughput(Throughput::Elements((csr.nnz() * fd) as u64));
            let id = format!("{label}/fd{fd}");

            group.bench_function(BenchmarkId::new("cpu_serial", &id), |b| {
                b.iter(|| {
                    black_box(&csr).spmm_into_serial(
                        black_box(&x),
                        black_box(fd),
                        black_box(&mut cpu_y),
                    )
                });
            });
            group.bench_function(BenchmarkId::new("cpu_rayon", &id), |b| {
                b.iter(|| {
                    black_box(&csr).spmm_into_parallel(
                        black_box(&x),
                        black_box(fd),
                        black_box(&mut cpu_y),
                    )
                });
            });
            // Resident only -- one-shot time must never be used as the
            // crossover comparator.
            group.bench_function(BenchmarkId::new("gpu_resident", &id), |b| {
                b.iter(|| {
                    let mut enc = backend.encoder(None);
                    op.encode(&mut enc).unwrap();
                    backend.wait(backend.submit(enc)).unwrap();
                });
            });
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Feature-dim-aware SpMM workgroup hypothesis
// ---------------------------------------------------------------------------

/// Re-tests the Phase 2 observation (8x8 wins at fd=8, 16x8 at fd=32) across
/// more feature dimensions, to see whether a simple rule is justified.
fn bench_fd_workgroup(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("spmm_fd_workgroup");
    group
        .sample_size(20)
        .measurement_time(Duration::from_secs(5));

    // Both problem sizes, because `spmm_workgroup_for` is a function of the
    // feature dimension ALONE. If the winner also moves with the row count,
    // that rule is under-specified and this sweep is what exposes it.
    //
    // All three shapes are measured back to back inside one run, so this is a
    // within-run A/B. Cross-run comparisons on this host carry ~3% confidence
    // intervals and cannot settle a 1-10% question.
    for (label, spec) in [
        ("medium", SparseSpec::medium()),
        ("large", SparseSpec::large()),
    ] {
        let csr = spec.build();
        let matrix = backend.resources().upload_csr_from("a", &csr).unwrap();
        let shape = matrix.shape();

        for fd in [1usize, 4, 8, 16, 32, 64, 128] {
            let x = spec.dense_block(fd);
            let res = backend.resources();
            let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
            let out = res.zeroed_tensor("y", shape.output_shape(fd)).unwrap();

            group.throughput(Throughput::Elements((csr.nnz() * fd) as u64));

            for wg in WorkgroupShape::sweep() {
                let op = backend
                    .operations()
                    .spmm()
                    .matrix(&matrix)
                    .input(&input)
                    .output(&out)
                    .workgroup(wg)
                    .prepare()
                    .unwrap();
                group.bench_function(
                    BenchmarkId::new(format!("wg{wg}"), format!("{label}/fd{fd}")),
                    |b| {
                        b.iter(|| {
                            let mut enc = backend.encoder(None);
                            op.encode(&mut enc).unwrap();
                            backend.wait(backend.submit(enc)).unwrap();
                        });
                    },
                );
            }
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_activation,
    bench_chain,
    bench_activation_workgroup,
    bench_crossover,
    bench_fd_workgroup
);
criterion_main!(benches);
