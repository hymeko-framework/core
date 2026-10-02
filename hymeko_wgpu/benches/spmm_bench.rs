//! CPU vs GPU SpMM benchmarks.
//!
//! # Cell separation
//!
//! The phase brief requires that pipeline compilation, buffer allocation,
//! upload, encoding, submit, GPU completion, readback and the CPU reference are
//! never conflated. Each is isolated in its own group:
//!
//! | cell | contents |
//! |---|---|
//! | `A cpu_serial` | `spmm_into_serial` |
//! | `B cpu_rayon` | `spmm_into_parallel` |
//! | `C gpu_oneshot` | CSR upload + X upload + prepare + dispatch + readback |
//! | `D gpu_static_csr` | CSR resident; X upload + dispatch + readback |
//! | `E gpu_resident_readback` | CSR + X resident; dispatch + readback |
//! | `E resident_reused_readback` | same work, retained staging and destination |
//! | `E reused_single_wait` | compute then readback, one queue completion wait |
//! | `F gpu_resident` | CSR + X + Y resident; encode + submit + wait only |
//! | `pipeline_creation` | cold library cache (driver cache retained) and warm prepare |
//! | `workgroup_sweep` | 8x8 / 16x8 / 16x16 at one cell |
//!
//! Cell G (kernel-only timestamp) is reported by `spmm_probe` rather than here;
//! Criterion measures host wall time, and folding a GPU timestamp into it would
//! conflate the two clocks.
//!
//! Run: `cargo bench -p hymeko_wgpu --offline`

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use hymeko_tensor::representations::tensor_csr::TensorCsr;
use hymeko_wgpu::testing::SparseSpec;
use hymeko_wgpu::{ComputeError, KernelId, PipelineCache, WgpuBackend, WorkgroupShape};

/// Feature dimensions swept at every scale.
const FEATURE_DIMS: [usize; 4] = [1, 8, 32, 128];

fn scales() -> [(&'static str, SparseSpec); 3] {
    [
        ("small", SparseSpec::small()),
        ("medium", SparseSpec::medium()),
        ("large", SparseSpec::large()),
    ]
}

/// One backend for the whole run — device acquisition is not under measurement.
fn backend() -> Option<WgpuBackend> {
    match WgpuBackend::builder()
        .label("hymeko-bench")
        .features(wgpu::Features::TIMESTAMP_QUERY)
        .build()
    {
        Ok(b) => {
            eprintln!("GPU: {}", b.capabilities().summary());
            if b.capabilities().device_type == "Cpu" {
                eprintln!("GPU cells SKIPPED: software CPU adapter; not a GPU result");
                return None;
            }
            eprintln!(
                "timestamp_query: {} (period {} ns)",
                b.capabilities().timestamp_query,
                b.capabilities().timestamp_period_ns
            );
            Some(b)
        }
        Err(ComputeError::BackendUnavailable { reason, .. }) => {
            eprintln!("GPU cells SKIPPED - no adapter: {reason}");
            None
        }
        Err(e) => panic!("backend init failed: {e}"),
    }
}

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

fn cell_id(label: &str, fd: usize) -> String {
    format!("{label}/fd{fd}")
}

// ---------------------------------------------------------------------------
// A / B — CPU reference cells
// ---------------------------------------------------------------------------

fn bench_cpu(c: &mut Criterion) {
    let mut group = c.benchmark_group("spmm_cpu");

    for (label, spec) in scales() {
        let csr = spec.build();
        for fd in FEATURE_DIMS {
            let x = spec.dense_block(fd);
            let mut y = vec![0.0f32; csr.num_rows * fd];

            group.throughput(Throughput::Elements((csr.nnz() * fd) as u64));
            tune(&mut group, label, fd);

            group.bench_function(BenchmarkId::new("A_serial", cell_id(label, fd)), |b| {
                b.iter(|| {
                    black_box(&csr).spmm_into_serial(
                        black_box(&x),
                        black_box(fd),
                        black_box(&mut y),
                    )
                });
            });
            group.bench_function(BenchmarkId::new("B_rayon", cell_id(label, fd)), |b| {
                b.iter(|| {
                    black_box(&csr).spmm_into_parallel(
                        black_box(&x),
                        black_box(fd),
                        black_box(&mut y),
                    )
                });
            });
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// C — GPU one-shot: everything, including upload and readback
// ---------------------------------------------------------------------------

fn bench_gpu_oneshot(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("spmm_gpu_oneshot");

    for (label, spec) in scales() {
        let csr = spec.build();
        for fd in FEATURE_DIMS {
            let x = spec.dense_block(fd);

            group.throughput(Throughput::Elements((csr.nnz() * fd) as u64));
            tune(&mut group, label, fd);

            group.bench_function(BenchmarkId::new("C_oneshot", cell_id(label, fd)), |b| {
                b.iter(|| {
                    let res = backend.resources();
                    let matrix = res.upload_csr_from("a", black_box(&csr)).unwrap();
                    let input = res
                        .upload_tensor("x", matrix.shape().input_shape(fd), black_box(&x))
                        .unwrap();
                    let output = res
                        .zeroed_tensor("y", matrix.shape().output_shape(fd))
                        .unwrap();
                    let op = backend
                        .operations()
                        .spmm()
                        .matrix(&matrix)
                        .input(&input)
                        .output(&output)
                        .prepare()
                        .unwrap();
                    let mut enc = backend.encoder(None);
                    op.encode(&mut enc).unwrap();
                    let idx = backend.submit(enc);
                    backend.wait(idx).unwrap();
                    black_box(backend.read_tensor(&output).unwrap())
                });
            });
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// D — CSR resident, X uploaded per call, readback per call
// ---------------------------------------------------------------------------

fn bench_gpu_static_csr(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("spmm_gpu_static_csr");

    for (label, spec) in scales() {
        let csr = spec.build();
        let matrix = backend.resources().upload_csr_from("a", &csr).unwrap();

        for fd in FEATURE_DIMS {
            let x = spec.dense_block(fd);
            let res = backend.resources();
            let input = res
                .upload_tensor("x", matrix.shape().input_shape(fd), &x)
                .unwrap();
            let output = res
                .zeroed_tensor("y", matrix.shape().output_shape(fd))
                .unwrap();
            let op = backend
                .operations()
                .spmm()
                .matrix(&matrix)
                .input(&input)
                .output(&output)
                .prepare()
                .unwrap();

            group.throughput(Throughput::Elements((csr.nnz() * fd) as u64));
            tune(&mut group, label, fd);

            group.bench_function(BenchmarkId::new("D_static_csr", cell_id(label, fd)), |b| {
                b.iter(|| {
                    // X re-uploaded each call; CSR and pipeline stay resident.
                    backend
                        .resources()
                        .write_tensor(&input, black_box(&x))
                        .unwrap();
                    let mut enc = backend.encoder(None);
                    op.encode(&mut enc).unwrap();
                    let idx = backend.submit(enc);
                    backend.wait(idx).unwrap();
                    black_box(backend.read_tensor(&output).unwrap())
                });
            });
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// E / F — fully resident, with and without readback
// ---------------------------------------------------------------------------

fn bench_gpu_resident(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("spmm_gpu_resident");

    for (label, spec) in scales() {
        let csr = spec.build();
        let matrix = backend.resources().upload_csr_from("a", &csr).unwrap();

        for fd in FEATURE_DIMS {
            let x = spec.dense_block(fd);
            let res = backend.resources();
            let input = res
                .upload_tensor("x", matrix.shape().input_shape(fd), &x)
                .unwrap();
            let output = res
                .zeroed_tensor("y", matrix.shape().output_shape(fd))
                .unwrap();
            let op = backend
                .operations()
                .spmm()
                .matrix(&matrix)
                .input(&input)
                .output(&output)
                .prepare()
                .unwrap();

            group.throughput(Throughput::Elements((csr.nnz() * fd) as u64));
            tune(&mut group, label, fd);

            let mut readback = backend.create_readback_workspace();
            let submit = || {
                let mut encoder = backend.encoder(None);
                op.encode(&mut encoder).unwrap();
                backend.submit(encoder)
            };
            submit();
            let got = backend.read_tensor_into(&output, &mut readback).unwrap();
            let mut expected = vec![0.0; csr.num_rows * fd];
            csr.spmm_into_serial(&x, fd, &mut expected);
            assert_eq!(got.len(), expected.len());
            assert!(
                got.iter().zip(&expected).all(|(&g, &r)| {
                    g.is_finite() && r.is_finite() && (g - r).abs() <= 1e-5 + 1e-5 * r.abs()
                }),
                "resident setup must agree with the Core CPU oracle"
            );
            drop(expected);

            // E: dispatch + readback, nothing uploaded. Verify before timing.
            group.bench_function(
                BenchmarkId::new("E_resident_readback", cell_id(label, fd)),
                |b| {
                    b.iter(|| {
                        let idx = submit();
                        backend.wait(idx).unwrap();
                        black_box(backend.read_tensor(&output).unwrap())
                    });
                },
            );

            // The two variants share resident data and allocation policy.
            // A readback submission follows compute on the same queue, so its
            // completion also proves compute completed when the first wait is omitted.
            for (name, wait_compute) in [
                ("E_resident_reused_readback", true),
                ("E_reused_single_wait", false),
            ] {
                group.bench_function(BenchmarkId::new(name, cell_id(label, fd)), |b| {
                    b.iter(|| {
                        let idx = submit();
                        if wait_compute {
                            backend.wait(idx).unwrap();
                        }
                        black_box(backend.read_tensor_into(&output, &mut readback).unwrap());
                    });
                });
            }

            // F: encode + submit + completion only. This is the number that
            // matters for a chain of operations that never leaves the device.
            group.bench_function(BenchmarkId::new("F_resident", cell_id(label, fd)), |b| {
                b.iter(|| {
                    let idx = submit();
                    backend.wait(idx).unwrap();
                });
            });
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Workgroup sweep — the configurations the brief requires comparing
// ---------------------------------------------------------------------------

fn bench_workgroup_sweep(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("spmm_workgroup");

    for (label, spec) in [
        ("medium", SparseSpec::medium()),
        ("large", SparseSpec::large()),
    ] {
        let csr = spec.build();
        let matrix = backend.resources().upload_csr_from("a", &csr).unwrap();

        for fd in [8usize, 32] {
            let x = spec.dense_block(fd);
            let res = backend.resources();
            let input = res
                .upload_tensor("x", matrix.shape().input_shape(fd), &x)
                .unwrap();
            let output = res
                .zeroed_tensor("y", matrix.shape().output_shape(fd))
                .unwrap();

            group.throughput(Throughput::Elements((csr.nnz() * fd) as u64));
            tune(&mut group, label, fd);

            for wg in WorkgroupShape::sweep() {
                let op = backend
                    .operations()
                    .spmm()
                    .matrix(&matrix)
                    .input(&input)
                    .output(&output)
                    .workgroup(wg)
                    .prepare()
                    .unwrap();

                group.bench_function(
                    BenchmarkId::new(format!("wg{wg}"), cell_id(label, fd)),
                    |b| {
                        b.iter(|| {
                            let mut enc = backend.encoder(None);
                            op.encode(&mut enc).unwrap();
                            let idx = backend.submit(enc);
                            backend.wait(idx).unwrap();
                        });
                    },
                );
            }
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Pipeline creation — measured separately, never inside a kernel cell
// ---------------------------------------------------------------------------

fn bench_pipeline_creation(c: &mut Criterion) {
    let Some(backend) = backend() else { return };
    let mut group = c.benchmark_group("spmm_prepare");
    group
        .sample_size(30)
        .measurement_time(Duration::from_secs(5));

    let spec = SparseSpec::small();
    let csr: TensorCsr<f32> = spec.build();
    let matrix = backend.resources().upload_csr_from("a", &csr).unwrap();
    let fd = 32;
    let x = spec.dense_block(fd);
    let res = backend.resources();
    let input = res
        .upload_tensor("x", matrix.shape().input_shape(fd), &x)
        .unwrap();
    let output = res
        .zeroed_tensor("y", matrix.shape().output_shape(fd))
        .unwrap();

    // Warm the cache first so this measures prepare() with a pipeline HIT:
    // bind group + params buffer + validation, no shader compilation.
    let _warm = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&output)
        .prepare()
        .unwrap();

    // prepare writes parameters to the queue. Complete each iteration so pending
    // upload staging cannot accumulate throughout Criterion's adaptive batches.
    group.bench_function("prepare_warm_cache_completed", |b| {
        b.iter(|| {
            black_box(
                backend
                    .operations()
                    .spmm()
                    .matrix(black_box(&matrix))
                    .input(black_box(&input))
                    .output(black_box(&output))
                    .prepare()
                    .unwrap(),
            );
            let idx = backend.submit(backend.encoder(None));
            backend.wait(idx).unwrap();
        });
    });

    group.bench_function("pipeline_cold_library_cache", |b| {
        b.iter(|| {
            let cache = PipelineCache::new();
            black_box(
                cache
                    .bundle(backend.device(), KernelId::Spmm, 16, 8)
                    .unwrap(),
            );
        });
    });

    // Completed transfer spans include allocation/queue submission as named;
    // these are host wall-time boundaries, not kernel timestamp measurements.
    group.bench_function("upload_csr_completed", |b| {
        b.iter(|| {
            let uploaded = backend
                .resources()
                .upload_csr_from("a-upload", black_box(&csr))
                .unwrap();
            let idx = backend.submit(backend.encoder(None));
            backend.wait(idx).unwrap();
            black_box(uploaded);
        });
    });
    group.bench_function("upload_input_completed", |b| {
        b.iter(|| {
            backend
                .resources()
                .write_tensor(&input, black_box(&x))
                .unwrap();
            let idx = backend.submit(backend.encoder(None));
            backend.wait(idx).unwrap();
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_cpu,
    bench_gpu_oneshot,
    bench_gpu_static_csr,
    bench_gpu_resident,
    bench_workgroup_sweep,
    bench_pipeline_creation
);
criterion_main!(benches);
