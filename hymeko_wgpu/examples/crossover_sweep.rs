//! CPU/GPU crossover sweep — single-host **provisional** baseline.
//!
//! # Why this is not a Criterion benchmark
//!
//! The sweep needs three things Criterion does not give:
//!
//! 1. **Interleaved A/B inside one process.** Criterion measures one
//!    `bench_function` to completion before starting the next, so two variants
//!    of the same cell are separated by seconds of unrelated work. On a host
//!    whose load swings between 4 % and 28 %, that separation is exactly where
//!    the noise enters. Here the four variants of a cell run back-to-back,
//!    repetition by repetition.
//! 2. **A reversed second pass.** Running the variant order forwards and then
//!    backwards makes thermal and load drift *visible*: if a variant looks
//!    faster only when measured first, the ordering exposes it.
//! 3. **A timestamp per cell**, so an external CPU-load sampler can be joined
//!    against the measurements afterwards.
//!
//! # Output
//!
//! Tab-separated, one row per (cell, variant, direction), to stdout. Columns are
//! printed in a header line. Raw Criterion output is deliberately not produced —
//! only this compact, joinable table.
//!
//! # Honest limits
//!
//! Measured on a host with 4–28 % ambient load. Differences below **3 %** are
//! not interpretable and the report must not draw conclusions from them. The
//! crossover is reported as a **bracket**, never as a single row count.
//!
//! Run:
//! ```text
//! cargo run -p hymeko_wgpu --release --example crossover_sweep > crossover.tsv
//! ```

use std::time::{Duration, Instant};

use hymeko::tensor::compute::TensorShape;
use hymeko::tensor::representations::tensor_csr::TensorCsr;
use hymeko_hnn::workloads::{DegreeShape, SparseSpec};
use hymeko_wgpu::{TensorArenaConfig, WgpuBackend};

/// Row counts spanning the whole regime of interest, logarithmically spaced.
const ROWS: [usize; 12] = [
    256, 512, 1_000, 2_000, 3_000, 5_000, 8_000, 12_000, 20_000, 50_000, 100_000, 200_000,
];
const FEATURE_DIMS: [usize; 3] = [8, 32, 128];
const DEGREES: [usize; 2] = [8, 32];
const SEED: u64 = 0x005E_EDC0_FFEE_u64;

/// Wall-clock budget per (cell, variant). Repetitions adapt to fill it, so a
/// cheap cell gets many samples and an expensive one does not stall the sweep.
const TARGET: Duration = Duration::from_millis(400);
const MIN_REPS: usize = 5;
const MAX_REPS: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    CpuSerial,
    CpuRayon,
    GpuResident,
    GpuReadback,
}

impl Variant {
    const ALL: [Self; 4] = [
        Self::CpuSerial,
        Self::CpuRayon,
        Self::GpuResident,
        Self::GpuReadback,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::CpuSerial => "cpu_serial",
            Self::CpuRayon => "cpu_rayon",
            Self::GpuResident => "gpu_resident",
            Self::GpuReadback => "gpu_readback",
        }
    }
}

/// Median and inter-quartile range of a sample set, in microseconds.
fn stats(mut xs: Vec<f64>) -> (f64, f64, f64, f64) {
    xs.sort_by(|a, b| a.partial_cmp(b).expect("no NaN timings"));
    let n = xs.len();
    let q = |f: f64| xs[((n as f64 - 1.0) * f).round() as usize];
    (q(0.5), q(0.25), q(0.75), xs[n - 1])
}

/// Time `f` repeatedly until the budget is met, returning per-iteration µs.
fn measure(mut f: impl FnMut()) -> Vec<f64> {
    // Warm-up, discarded: first touch pays page faults and pipeline creation.
    for _ in 0..3 {
        f();
    }
    let mut samples = Vec::new();
    let start = Instant::now();
    while samples.len() < MAX_REPS && (samples.len() < MIN_REPS || start.elapsed() < TARGET) {
        let t = Instant::now();
        f();
        samples.push(t.elapsed().as_secs_f64() * 1e6);
    }
    samples
}

struct Cell {
    csr: TensorCsr<f32>,
    x: Vec<f32>,
    fd: usize,
}

fn main() {
    let backend = match WgpuBackend::builder().label("crossover").build() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("no GPU adapter: {e}");
            return;
        }
    };
    eprintln!("{}", backend.capabilities().summary());

    println!(
        "iso_time\tdirection\tdistribution\tdegree\trows\tcols\tnnz\tfd\tseed\tvariant\tmedian_us\tq1_us\tq3_us\tmax_us\treps"
    );

    // Forward then reversed: a variant that only wins when measured first is
    // measuring drift, not the kernel.
    for direction in ["fwd", "rev"] {
        for (dist_name, shape) in [
            ("uniform", DegreeShape::Uniform),
            ("irregular", DegreeShape::Irregular),
        ] {
            for degree in DEGREES {
                for rows in ROWS {
                    let spec = SparseSpec {
                        rows,
                        cols: rows,
                        avg_nnz_per_row: degree,
                        shape,
                        seed: SEED,
                    };
                    let csr = spec.build();
                    for fd in FEATURE_DIMS {
                        let cell = Cell {
                            x: spec.dense_block(fd),
                            csr: csr.clone(),
                            fd,
                        };
                        run_cell(&backend, &cell, direction, dist_name, degree, spec.seed);
                    }
                }
            }
        }
    }
}

fn run_cell(
    backend: &WgpuBackend,
    cell: &Cell,
    direction: &str,
    dist: &str,
    degree: usize,
    seed: u64,
) {
    let csr = &cell.csr;
    let fd = cell.fd;
    let res = backend.resources();
    let matrix = res.upload_csr_from("a", csr).unwrap();
    let shape = matrix.shape();
    let input = res
        .upload_tensor("x", shape.input_shape(fd), &cell.x)
        .unwrap();
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let out = arena.acquire(shape.output_shape(fd)).unwrap();
    let gpu_op = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&out)
        .prepare()
        .unwrap();

    let mut cpu_y = vec![0.0f32; csr.num_rows * fd];

    let mut order = Variant::ALL;
    if direction == "rev" {
        order.reverse();
    }

    for v in order {
        let samples = match v {
            Variant::CpuSerial => measure(|| {
                csr.spmm_into_serial(&cell.x, fd, &mut cpu_y);
                std::hint::black_box(&cpu_y);
            }),
            Variant::CpuRayon => measure(|| {
                csr.spmm_into_parallel(&cell.x, fd, &mut cpu_y);
                std::hint::black_box(&cpu_y);
            }),
            Variant::GpuResident => measure(|| {
                let mut s = arena.session(None);
                gpu_op.encode(&mut s).unwrap();
                s.submit_and_wait().unwrap();
            }),
            Variant::GpuReadback => measure(|| {
                let mut s = arena.session(None);
                gpu_op.encode(&mut s).unwrap();
                s.submit_and_wait().unwrap();
                std::hint::black_box(backend.read_tensor(&out).unwrap());
            }),
        };
        let reps = samples.len();
        let (med, q1, q3, max) = stats(samples);
        // A coarse monotonic stamp; the external load sampler uses wall clock,
        // and cells are seconds apart, so second resolution is enough to join.
        println!(
            "{}\t{direction}\t{dist}\t{degree}\t{}\t{}\t{}\t{fd}\t{seed:#x}\t{}\t{med:.2}\t{q1:.2}\t{q3:.2}\t{max:.2}\t{reps}",
            now_iso(),
            csr.num_rows,
            csr.num_cols,
            csr.nnz(),
            v.label(),
        );
    }
    drop(out);
    let _ = arena.reclaim_all();
    let _ = TensorShape::new(1, 1);
}

/// Seconds since the process started, plus a wall-clock stamp supplied by the
/// caller through the environment.
///
/// `SystemTime` formatting without a date crate is awkward, and adding one for a
/// benchmark harness would be a dependency for nothing — so the harness prints
/// elapsed seconds and the launcher records the absolute start time.
fn now_iso() -> String {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    let s = START.get_or_init(Instant::now);
    format!("+{:.1}s", s.elapsed().as_secs_f64())
}
