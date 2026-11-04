//! P47 Amendment 3, K1: the Catmull-Rom membership on the device at the shapes the CPU profile
//! names (`nagare/run_outputs/2026-10-11-n2-profile/`), grid 6.
//!
//! # Cells
//!
//! | id | contents |
//! |---|---|
//! | `resident_forward` | forward encoded, submitted, waited; operands stay on the device |
//! | `resident_pullback_chunkN` | the two pullback passes at `N` rows a unit, submitted, waited |
//! | `roundtrip_forward` | upload of `x`, forward, readback of the output: a per-op offload |
//!
//! The resident cells are what a chain that stays on the device pays; the round trip is what a
//! per-op offload pays. Compare with the CPU kernel's criterion numbers in
//! `nagare/run_outputs/2026-10-11-n2-profile/cpu_spline_bench.txt`.
//!
//! Run: `cargo bench -p hymeko_wgpu --bench catmull_rom_bench`

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

use hymeko_tensor::compute::TensorShape;
use hymeko_wgpu::WgpuBackend;
use hymeko_wgpu::testing::{bench_backend, field};

const GRID: usize = 6;
const SHAPES: [(&str, usize, usize); 3] = [
    ("mnist_level4_12544x32", 12_544, 32),
    ("mnist_pixels_50176x1", 50_176, 1),
    ("cifar_level_65536x32", 65_536, 32),
];

/// Encode with `encode`, submit and wait: one resident cell iteration.
fn submit_wait(backend: &WgpuBackend, label: &str, encode: impl FnOnce(&mut wgpu::CommandEncoder)) {
    let mut encoder = backend.encoder(Some(label));
    encode(&mut encoder);
    backend.wait(backend.submit(encoder)).unwrap();
}

fn catmull_rom(c: &mut Criterion) {
    let Some(backend) = bench_backend("hymeko-catmull-rom-bench") else {
        return;
    };
    let mut group = c.benchmark_group("catmull_rom");
    group.measurement_time(Duration::from_secs(4));
    for (name, rows, channels) in SHAPES {
        let shape = TensorShape::new(rows, channels);
        let control_shape = TensorShape::new(channels, GRID);
        let x_host = field(rows * channels, 11, 1.2);
        let resources = backend.resources();
        let x = resources.upload_tensor("x", shape, &x_host).unwrap();
        let control = resources
            .upload_tensor("control", control_shape, &field(channels * GRID, 23, 1.0))
            .unwrap();
        let grad_y = resources
            .upload_tensor("grad_y", shape, &field(rows * channels, 37, 1.0))
            .unwrap();
        let output = resources.zeroed_tensor("y", shape).unwrap();
        let grad_x = resources.zeroed_tensor("grad_x", shape).unwrap();
        let grad_control = resources
            .zeroed_tensor("grad_control", control_shape)
            .unwrap();
        let forward = backend
            .operations()
            .catmull_rom_forward()
            .input(&x)
            .control(&control)
            .output(&output)
            .prepare()
            .unwrap();
        group.bench_function(BenchmarkId::new("resident_forward", name), |b| {
            b.iter(|| submit_wait(&backend, "bench-forward", |e| forward.encode(e).unwrap()));
        });
        for chunk_rows in [16, 32, 64, 128] {
            let pullback = backend
                .operations()
                .catmull_rom_pullback()
                .input(&x)
                .control(&control)
                .grad_output(&grad_y)
                .grad_input(&grad_x)
                .grad_control(&grad_control)
                .chunk_rows(chunk_rows)
                .prepare()
                .unwrap();
            let id = format!("resident_pullback_chunk{chunk_rows}");
            group.bench_function(BenchmarkId::new(id, name), |b| {
                b.iter(|| submit_wait(&backend, "bench-pullback", |e| pullback.encode(e).unwrap()));
            });
        }
        group.bench_function(BenchmarkId::new("roundtrip_forward", name), |b| {
            b.iter(|| {
                let x_device = resources.upload_tensor("x", shape, &x_host).unwrap();
                let y_device = resources.zeroed_tensor("y", shape).unwrap();
                let op = backend
                    .operations()
                    .catmull_rom_forward()
                    .input(&x_device)
                    .control(&control)
                    .output(&y_device)
                    .prepare()
                    .unwrap();
                submit_wait(&backend, "bench-roundtrip", |e| op.encode(e).unwrap());
                black_box(backend.read_tensor(&y_device).unwrap())
            });
        });
    }
    group.finish();
}

criterion_group!(benches, catmull_rom);
criterion_main!(benches);
