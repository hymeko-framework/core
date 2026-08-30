//! Sustained GPU-resident chain — throughput stability over a 60 s window.
//!
//! # What this answers that a Criterion cell cannot
//!
//! Criterion measures short bursts. A laptop GPU under a burst runs at boost
//! clocks and low temperature; the same kernel over a minute may lose clock to
//! thermal or power limits. Reporting only the burst number would overstate what
//! a real forward-pass loop sustains.
//!
//! So this runs the large/fd128 resident chain continuously and reports
//! throughput **per time window**, never collapsed into one median unless the
//! windows agree.
//!
//! # Protocol
//!
//! 1. 10 s warm-up, discarded — the GPU starts cold and at boost clock.
//! 2. 60 s measured, bucketed into 5 s windows.
//! 3. First 10 s and last 10 s reported separately, because their difference is
//!    the throttling signal.
//!
//! Clocks, temperature and power are sampled externally by the launcher
//! (`nvidia-smi`), not from inside this process — reading them here would add a
//! dependency and would sample at the wrong moments anyway.
//!
//! Run:
//! ```text
//! cargo run -p hymeko_wgpu --release --example sustained_chain
//! ```

use std::time::{Duration, Instant};

use hymeko_wgpu::testing::SparseSpec;
use hymeko_wgpu::testing::{Fixture, encode_chain};
use hymeko_wgpu::{ActivationKind, TensorArenaConfig, WgpuBackend};

const LAYERS: usize = 8;
const FEATURE_DIM: usize = 128;
const WARMUP: Duration = Duration::from_secs(10);
const MEASURE: Duration = Duration::from_secs(60);
const WINDOW: Duration = Duration::from_secs(5);

fn main() {
    let backend = match WgpuBackend::builder().label("sustained").build() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("no GPU adapter: {e}");
            return;
        }
    };
    eprintln!("{}", backend.capabilities().summary());

    let spec = SparseSpec::large();
    let csr = spec.build();
    let x = spec.dense_block(FEATURE_DIM);
    let fx = Fixture::build(&backend, &csr, &x, FEATURE_DIM);

    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let a = arena.acquire(fx.out_shape).unwrap();
    let b = arena.acquire(fx.out_shape).unwrap();

    // MACs per chain: every layer is one SpMM over the whole matrix.
    let macs_per_chain = (csr.nnz() * FEATURE_DIM * LAYERS) as f64;

    eprintln!(
        "workload: {} rows x {} cols, nnz {}, fd {}, {} layers, {:.1} MMAC/chain",
        csr.num_rows,
        csr.num_cols,
        csr.nnz(),
        FEATURE_DIM,
        LAYERS,
        macs_per_chain / 1e6
    );

    let one_chain = |arena: &hymeko_wgpu::TensorArena<'_>| {
        let mut session = arena.session(None);
        encode_chain(
            &backend,
            &fx,
            &a,
            &b,
            LAYERS,
            ActivationKind::Relu,
            &mut session,
        );
        session.submit_and_wait().unwrap();
    };

    // --- warm-up ----------------------------------------------------------
    eprintln!("warm-up {} s ...", WARMUP.as_secs());
    let t = Instant::now();
    let mut warm_chains = 0u64;
    while t.elapsed() < WARMUP {
        one_chain(&arena);
        warm_chains += 1;
    }
    eprintln!("warm-up done: {warm_chains} chains");

    // --- measured ---------------------------------------------------------
    println!("window_start_s\twindow_end_s\tchains\tchains_per_s\tgmac_per_s\tmean_ms_per_chain");
    let start = Instant::now();
    let mut window_start = Duration::ZERO;
    let mut window_chains = 0u64;
    let mut rows: Vec<(f64, f64, u64)> = Vec::new();

    while start.elapsed() < MEASURE {
        one_chain(&arena);
        window_chains += 1;
        let now = start.elapsed();
        if now - window_start >= WINDOW {
            let secs = (now - window_start).as_secs_f64();
            emit(
                window_start.as_secs_f64(),
                now.as_secs_f64(),
                window_chains,
                secs,
                macs_per_chain,
            );
            rows.push((window_start.as_secs_f64(), secs, window_chains));
            window_start = now;
            window_chains = 0;
        }
    }
    if window_chains > 0 {
        let now = start.elapsed();
        let secs = (now - window_start).as_secs_f64();
        emit(
            window_start.as_secs_f64(),
            now.as_secs_f64(),
            window_chains,
            secs,
            macs_per_chain,
        );
        rows.push((window_start.as_secs_f64(), secs, window_chains));
    }

    // --- first 10 s vs last 10 s -----------------------------------------
    let rate = |sel: &dyn Fn(f64) -> bool| {
        let (c, s): (u64, f64) = rows
            .iter()
            .filter(|(st, _, _)| sel(*st))
            .fold((0, 0.0), |(c, s), (_, secs, ch)| (c + ch, s + secs));
        if s > 0.0 { c as f64 / s } else { 0.0 }
    };
    let first = rate(&|st| st < 10.0);
    let last = rate(&|st| st >= MEASURE.as_secs_f64() - 10.0);
    eprintln!("first10s: {first:.2} chains/s   last10s: {last:.2} chains/s");
    if first > 0.0 {
        let delta = (last - first) / first * 100.0;
        eprintln!("delta last-vs-first: {delta:+.1}%");
        eprintln!(
            "interpretation: {}",
            if delta.abs() < 3.0 {
                "stable (within the 3% noise floor; do NOT read a trend)"
            } else if delta < 0.0 {
                "THROTTLING or drift - do not collapse to a single median without naming the cause"
            } else {
                "improving - likely still warming caches/clocks; treat first window as warm-up residue"
            }
        );
    }
}

fn emit(t0: f64, t1: f64, chains: u64, secs: f64, macs_per_chain: f64) {
    let cps = chains as f64 / secs;
    println!(
        "{t0:.1}\t{t1:.1}\t{chains}\t{cps:.3}\t{:.2}\t{:.3}",
        cps * macs_per_chain / 1e9,
        1000.0 / cps
    );
}
