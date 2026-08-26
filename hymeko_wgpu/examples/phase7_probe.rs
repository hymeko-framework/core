//! Phase 7 diagnostic probe: kernel timestamps (Part A) and readback stage
//! decomposition (Part B), with paired alternating ordering (Part C).
//!
//! # Ordering discipline
//!
//! Phase 5 established that measuring variants in back-to-back blocks leaves a
//! ~5 % order bias even on an idle host. Every comparison here is therefore
//! **paired and alternating** — A/B, B/A, A/B, B/A — with individual paired
//! samples retained, so a variant that only wins when measured first is visible
//! rather than averaged in.
//!
//! # Rejection rule, fixed before any result was read
//!
//! * the first `WARMUP` repetitions of every cell are discarded outright;
//! * a timestamp pair whose end is not greater than its begin is discarded as
//!   unusable (wrap or out-of-order) and counted;
//! * nothing else is rejected. There is no outlier filter, because choosing one
//!   after seeing the data is how a desired answer gets manufactured.
//!
//! Output is TSV to stdout, one row per measurement, for offline analysis.
//!
//! Run:
//! ```text
//! cargo run -p hymeko_wgpu --release --example phase7_probe -- --part a
//! cargo run -p hymeko_wgpu --release --example phase7_probe -- --part b
//! ```

use std::time::Instant;

use hymeko::tensor::compute::TensorShape;
use hymeko_hnn::workloads::{DegreeShape, SparseSpec};
use hymeko_wgpu::profiling::{
    GpuTimer, ReadbackPlan, ReadbackTiming, StagingBuffer, TimestampSupport, encode_spmm_profiled,
    timed_readback,
};
use hymeko_wgpu::testing::Fixture;
use hymeko_wgpu::{TensorArenaConfig, WgpuBackend};

const SEED: u64 = 0x005E_EDC0_FFEE_u64;
const WARMUP: usize = 3;
const PAIRS: usize = 12;

fn backend() -> Option<WgpuBackend> {
    // Timestamp features are requested here and intersected with adapter
    // support inside the builder; an adapter that refuses them yields a backend
    // that reports unsupported rather than one that fails to build.
    match WgpuBackend::builder()
        .label("phase7")
        .features(wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS)
        .build()
    {
        Ok(b) => Some(b),
        Err(e) => {
            eprintln!("no GPU adapter: {e}");
            None
        }
    }
}

fn main() {
    let mut part = "a".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--part"
            && let Some(v) = args.next()
        {
            part = v;
        }
    }

    let Some(backend) = backend() else { return };
    let caps = backend.capabilities();
    eprintln!("{}", caps.summary());
    let support = TimestampSupport::from_features(backend.device().features());
    eprintln!(
        "timestamp support: {support:?}  period_ns={}  inside_encoders={}",
        caps.timestamp_period_ns, caps.timestamp_inside_encoders
    );

    match part.as_str() {
        "a" => part_a(&backend, support),
        "b" => part_b(&backend, support),
        "d" => part_d(&backend, support),
        "e" => part_e(&backend, support),
        "f" => part_f(&backend, support),
        other => eprintln!("unknown --part {other}; expected a, b, d, e or f"),
    }
}

// ---------------------------------------------------------------------------
// Part A — K1 (host wall) vs K2 (GPU timestamp around the compute pass)
// ---------------------------------------------------------------------------

fn part_a(backend: &WgpuBackend, support: TimestampSupport) {
    if !support.passes() {
        eprintln!("TIMESTAMP_QUERY not granted: kernel-only time is UNSUPPORTED on this adapter.");
        eprintln!("Reporting no K2. K1 alone cannot be compared with a CUDA kernel interval.");
        return;
    }
    let Some(timer) = GpuTimer::new(backend, 4) else {
        eprintln!("GpuTimer unavailable despite reported support; aborting Part A.");
        return;
    };

    println!("part\trows\tdeg\tfd\tnnz\tdistribution\trep\tk1_host_us\tk2_gpu_us\tdiscarded");

    for (rows, deg, fd) in [
        (200_000usize, 8usize, 8usize),
        (200_000, 8, 32),
        (200_000, 8, 128),
        (200_000, 32, 32),
    ] {
        let spec = SparseSpec {
            rows,
            cols: rows,
            avg_nnz_per_row: deg,
            shape: DegreeShape::Uniform,
            seed: SEED,
        };
        let csr = spec.build();
        let x = spec.dense_block(fd);
        let fx = Fixture::build(backend, &csr, &x, fd);
        let arena = backend.tensor_arena(TensorArenaConfig::default());
        let out = arena.acquire(fx.out_shape).expect("acquire");
        let op = backend
            .operations()
            .spmm()
            .matrix(&fx.matrix)
            .input(&fx.input)
            .output(&out)
            .prepare()
            .expect("prepare");

        let mut discarded = 0usize;
        for rep in 0..(WARMUP + PAIRS * 2) {
            let mut session = arena.session(None);
            let writes = timer.pass_writes(0, 1).expect("pass writes");
            let t0 = Instant::now();
            encode_spmm_profiled(&op, &mut session, writes).expect("encode");
            {
                let enc = session.encoder();
                timer.resolve(enc, 2).expect("resolve");
            }
            session.submit_and_wait().expect("submit");
            let k1 = t0.elapsed().as_secs_f64() * 1e6;

            let ticks = timer.read(backend, 2).expect("read timestamps");
            let k2 = timer
                .elapsed(&ticks, 0, 1)
                .and_then(hymeko_wgpu::profiling::ElapsedTicks::micros);
            if k2.is_none() {
                discarded += 1;
            }
            if rep < WARMUP {
                continue;
            }
            println!(
                "A\t{rows}\t{deg}\t{fd}\t{}\tuniform\t{}\t{k1:.2}\t{}\t{discarded}",
                csr.nnz(),
                rep - WARMUP,
                k2.map_or_else(|| "NA".to_string(), |v| format!("{v:.2}")),
            );
        }
        drop(out);
        let _ = arena.reclaim_all();
    }
}

// ---------------------------------------------------------------------------
// Part B — readback stage decomposition, paired alternating
// ---------------------------------------------------------------------------

fn part_b(backend: &WgpuBackend, support: TimestampSupport) {
    if !support.encoders() {
        eprintln!(
            "TIMESTAMP_QUERY_INSIDE_ENCODERS not granted: R2 (GPU copy) is UNSUPPORTED; \
             the remaining stages are still measured directly."
        );
    }
    let timer = GpuTimer::new(backend, 4);

    println!(
        "part\tmib\tbytes\tvariant\tchunk_mib\trep\tphase\tr1_alloc_us\tr2_gpu_copy_us\tr3_submit_wait_us\tr4_map_us\tr5_memcpy_us\tr6_total_us\tgbps\tbytes_ok"
    );

    // Dense through the previously observed 41-51 MiB transition, then out to
    // 128 MiB. Sizes are element counts chosen so bytes land on these targets.
    let targets_mib: [f64; 14] = [
        32.0, 36.0, 40.0, 42.0, 44.0, 46.0, 48.0, 50.0, 52.0, 56.0, 64.0, 80.0, 96.0, 128.0,
    ];

    for &mib in &targets_mib {
        // f32 elements; deliberately NOT a chunk multiple for the odd sizes so
        // the non-multiple path is exercised.
        let elements = ((mib * 1024.0 * 1024.0) / 4.0) as usize;
        let shape = TensorShape::new(elements, 1);
        let bytes = (elements * 4) as u64;

        let res = backend.resources();
        // A known pattern so returned bytes can be validated, not just timed.
        let pattern: Vec<f32> = (0..elements).map(|i| (i % 1024) as f32).collect();
        let src = res.upload_tensor("src", shape, &pattern).expect("upload");

        // One correctly-sized reusable staging buffer for the reuse variant.
        let mut reused = StagingBuffer::new(backend, bytes);

        for rep in 0..(WARMUP + PAIRS * 2) {
            let phase = if rep < WARMUP { "warmup" } else { "steady" };
            // Paired alternating: fresh-first on even reps, reuse-first on odd.
            let fresh_first = rep % 2 == 0;
            let order: [bool; 2] = if fresh_first {
                [true, false]
            } else {
                [false, true]
            };
            for fresh in order {
                let (data, t) = if fresh {
                    timed_readback(
                        backend,
                        src.buffer(),
                        bytes,
                        ReadbackPlan {
                            timer: timer.as_ref(),
                            ..ReadbackPlan::default()
                        },
                    )
                    .expect("readback")
                } else {
                    timed_readback(
                        backend,
                        src.buffer(),
                        bytes,
                        ReadbackPlan {
                            staging: Some(&mut reused),
                            timer: timer.as_ref(),
                            ..ReadbackPlan::default()
                        },
                    )
                    .expect("readback")
                };
                let ok = validate(&data, elements);
                emit(
                    mib,
                    bytes,
                    if fresh { "fresh" } else { "reused" },
                    rep,
                    phase,
                    &t,
                    ok,
                );
            }

            // Chunked variants, only in steady state to keep the matrix bounded.
            if rep >= WARMUP {
                for chunk_mib in [8u64, 16] {
                    let chunk = chunk_mib * 1024 * 1024;
                    let (data, t) = timed_readback(
                        backend,
                        src.buffer(),
                        bytes,
                        ReadbackPlan {
                            staging: Some(&mut reused),
                            chunk_bytes: Some(chunk),
                            timer: timer.as_ref(),
                            ..ReadbackPlan::default()
                        },
                    )
                    .expect("chunked readback");
                    let ok = validate(&data, elements);
                    emit(mib, bytes, "chunked", rep, phase, &t, ok);
                }
            }
        }
    }

    // Edge cases: zero-length and a deliberately non-chunk-multiple payload.
    let res = backend.resources();
    let empty = res
        .zeroed_tensor("empty", TensorShape::new(0, 8))
        .expect("zero tensor");
    let (v, t) = timed_readback(
        backend,
        empty.buffer(),
        0,
        ReadbackPlan {
            timer: timer.as_ref(),
            ..ReadbackPlan::default()
        },
    )
    .expect("zero readback");
    eprintln!(
        "edge: zero-length -> {} elements, total {:.2} us (must be 0 elements)",
        v.len(),
        t.total_us
    );

    let odd_elements = 3_145_733usize; // ~12 MiB + 20 bytes, not a chunk multiple
    let odd_bytes = (odd_elements * 4) as u64;
    let odd_pattern: Vec<f32> = (0..odd_elements).map(|i| (i % 1024) as f32).collect();
    let odd = res
        .upload_tensor("odd", TensorShape::new(odd_elements, 1), &odd_pattern)
        .expect("upload odd");
    let (data, t) = timed_readback(
        backend,
        odd.buffer(),
        odd_bytes,
        ReadbackPlan {
            chunk_bytes: Some(8 * 1024 * 1024),
            timer: timer.as_ref(),
            ..ReadbackPlan::default()
        },
    )
    .expect("odd chunked readback");
    eprintln!(
        "edge: non-multiple chunked -> {} elements, bytes_ok={}, total {:.2} us",
        data.len(),
        validate(&data, odd_elements),
        t.total_us
    );
}

fn validate(data: &[f32], elements: usize) -> bool {
    if data.len() != elements {
        return false;
    }
    // Spot-check the pattern at the ends and the middle; a full scan on 128 MiB
    // every repetition would dominate the measurement it is validating.
    let idx = [0usize, elements / 2, elements - 1];
    idx.iter()
        .all(|&i| (data[i] - (i % 1024) as f32).abs() < f32::EPSILON)
}

#[allow(clippy::too_many_arguments)]
fn emit(
    mib: f64,
    bytes: u64,
    variant: &str,
    rep: usize,
    phase: &str,
    t: &ReadbackTiming,
    ok: bool,
) {
    println!(
        "B\t{mib:.0}\t{bytes}\t{variant}\t{}\t{rep}\t{phase}\t{:.2}\t{}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.3}\t{ok}",
        t.chunk_bytes
            .map_or_else(|| "NA".to_string(), |c| (c / (1024 * 1024)).to_string()),
        t.alloc_us,
        t.gpu_copy_us
            .map_or_else(|| "NA".to_string(), |v| format!("{v:.2}")),
        t.submit_wait_us,
        t.map_us,
        t.memcpy_us,
        t.total_us,
        t.total_gbps(),
    );
}

// ---------------------------------------------------------------------------
// Part D — why is R5 slow? Factorize the host copy over source × destination
// ---------------------------------------------------------------------------
//
// Part B measured R5 (mapped range -> Vec) at a flat ~4 GB/s, far below what
// this host's DRAM can do. Two candidate causes were indistinguishable in that
// measurement, because R5 included allocating the destination:
//
//   (a) the SOURCE is slow  — a MAP_READ mapping is uncached / write-combined,
//                             so reads from it do not run at cached-DRAM rate;
//   (b) the DESTINATION is slow — a fresh 128 MiB Vec must be faulted in page
//                             by page on first touch.
//
// The 2x2 below separates them. Same byte counts, same process, same timer
// boundary. If cached->fresh is also ~4 GB/s, the cost is (b). If cached->fresh
// is fast and mapped->warm is still slow, the cost is (a). Both slow means both.

fn part_d(backend: &WgpuBackend, support: TimestampSupport) {
    let timer = GpuTimer::new(backend, 4);
    let _ = support;

    println!("part\tmib\tbytes\tsource\tdest\trep\tcopy_us\tgbps");

    for &mib in &[32.0f64, 64.0, 128.0] {
        let elements = ((mib * 1024.0 * 1024.0) / 4.0) as usize;
        let bytes = (elements * 4) as u64;
        let shape = TensorShape::new(elements, 1);
        let gbps = |us: f64| bytes as f64 / (us * 1e-6) / 1e9;

        let pattern: Vec<f32> = (0..elements).map(|i| (i % 1024) as f32).collect();
        let res = backend.resources();
        let src = res.upload_tensor("src", shape, &pattern).expect("upload");
        let mut staging = StagingBuffer::new(backend, bytes);
        let mut warm: Vec<f32> = Vec::with_capacity(elements);
        // Force the warm destination fully resident before it is used, so it is
        // genuinely warm rather than merely reserved.
        warm.extend_from_slice(&pattern);

        for rep in 0..(WARMUP + PAIRS) {
            // --- cached source -> fresh destination -------------------------
            let t0 = Instant::now();
            let fresh_copy = pattern.to_vec();
            let cached_fresh = t0.elapsed().as_secs_f64() * 1e6;
            std::hint::black_box(&fresh_copy);
            drop(fresh_copy);

            // --- cached source -> warm destination --------------------------
            let t0 = Instant::now();
            warm.clear();
            warm.extend_from_slice(&pattern);
            let cached_warm = t0.elapsed().as_secs_f64() * 1e6;
            std::hint::black_box(&warm);

            // --- mapped source -> fresh destination -------------------------
            let (data, tf) = timed_readback(
                backend,
                src.buffer(),
                bytes,
                ReadbackPlan {
                    staging: Some(&mut staging),
                    timer: timer.as_ref(),
                    ..ReadbackPlan::default()
                },
            )
            .expect("mapped->fresh");
            assert!(validate(&data, elements), "mapped->fresh bytes wrong");
            drop(data);

            // --- mapped source -> warm destination --------------------------
            let (empty, tw) = timed_readback(
                backend,
                src.buffer(),
                bytes,
                ReadbackPlan {
                    staging: Some(&mut staging),
                    timer: timer.as_ref(),
                    dest: Some(&mut warm),
                    ..ReadbackPlan::default()
                },
            )
            .expect("mapped->warm");
            assert!(empty.is_empty(), "dest-reuse path must return an empty Vec");
            assert!(validate(&warm, elements), "mapped->warm bytes wrong");

            if rep < WARMUP {
                continue;
            }
            let r = rep - WARMUP;
            for (source, dest, us) in [
                ("cached", "fresh", cached_fresh),
                ("cached", "warm", cached_warm),
                ("mapped", "fresh", tf.memcpy_us),
                ("mapped", "warm", tw.memcpy_us),
            ] {
                println!(
                    "D\t{mib:.0}\t{bytes}\t{source}\t{dest}\t{r}\t{us:.2}\t{:.3}",
                    gbps(us)
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Part E — does the R2 discontinuity follow the TRANSFER size or the STAGING
//          BUFFER's allocated capacity?
// ---------------------------------------------------------------------------
//
// Part B established that chunking a large transfer into small copies does not
// recover the fast rate: 16 x 8 MiB behaves exactly like 1 x 128 MiB. That
// rules out per-copy size as the governing quantity and leaves two candidates:
// the total bytes moved, or the capacity the staging buffer was allocated with.
//
// This part holds the transfer fixed at 32 MiB — the size that is FAST on
// Turing and SLOW on Blackwell — and varies only the staging capacity. If the
// rate tracks capacity, the driver's memory placement is decided at allocation
// time and no copy-side change can affect it.

fn part_e(backend: &WgpuBackend, support: TimestampSupport) {
    let Some(timer) = GpuTimer::new(backend, 4) else {
        eprintln!("no GpuTimer; Part E needs R2");
        return;
    };
    if !support.encoders() {
        eprintln!("TIMESTAMP_QUERY_INSIDE_ENCODERS not granted: Part E cannot measure R2.");
        return;
    }

    println!("part\txfer_mib\tcap_mib\tbytes\trep\tr2_gpu_copy_us\tgbps");

    for &xfer_mib in &[32u64, 64] {
        let elements = ((xfer_mib * 1024 * 1024) / 4) as usize;
        let bytes = xfer_mib * 1024 * 1024;
        let pattern: Vec<f32> = (0..elements).map(|i| (i % 1024) as f32).collect();
        let res = backend.resources();
        let src = res
            .upload_tensor("src", TensorShape::new(elements, 1), &pattern)
            .expect("upload");

        for &cap_mib in &[32u64, 64, 128, 256] {
            if cap_mib < xfer_mib {
                continue;
            }
            let mut staging = StagingBuffer::new(backend, cap_mib * 1024 * 1024);
            let mut warm: Vec<f32> = vec![0.0; elements];

            for rep in 0..(WARMUP + PAIRS) {
                let (_, t) = timed_readback(
                    backend,
                    src.buffer(),
                    bytes,
                    ReadbackPlan {
                        staging: Some(&mut staging),
                        timer: Some(&timer),
                        dest: Some(&mut warm),
                        ..ReadbackPlan::default()
                    },
                )
                .expect("readback");
                assert!(validate(&warm, elements), "part E bytes wrong");
                if rep < WARMUP {
                    continue;
                }
                let Some(r2) = t.gpu_copy_us else { continue };
                println!(
                    "E\t{xfer_mib}\t{cap_mib}\t{bytes}\t{}\t{r2:.2}\t{:.3}",
                    rep - WARMUP,
                    bytes as f64 / (r2 * 1e-6) / 1e9
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Part F — does the size cliff survive a preallocated destination?
// ---------------------------------------------------------------------------
//
// Part B swept sizes while allocating a fresh destination Vec every repetition.
// Part E then measured a 64 MiB transfer 4x faster than Part B had, differing
// only in that the destination was preallocated — which should not be able to
// change a GPU-timeline interval at all, unless the two contend for something.
//
// The suspected contended resource is host memory bandwidth: on glibc,
// DEFAULT_MMAP_THRESHOLD_MAX is 32 MiB, so every allocation above that is
// mmap'd and munmap'd, and the kernel must hand back freshly zeroed pages each
// time. That zeroing costs bandwidth the device-to-host DMA also needs.
//
// This part re-runs Part B's sweep with the destination preallocated and
// everything else identical. If the cliff is an artifact of destination churn
// it disappears; if it is a driver placement effect it survives.

fn part_f(backend: &WgpuBackend, support: TimestampSupport) {
    let timer = GpuTimer::new(backend, 4);
    if !support.encoders() {
        eprintln!("TIMESTAMP_QUERY_INSIDE_ENCODERS not granted: Part F cannot measure R2.");
    }

    println!("part\tmib\tbytes\tdest\trep\tr2_gpu_copy_us\tr5_memcpy_us\tr6_total_us\tr2_gbps");

    for &mib in &[
        32.0f64, 36.0, 40.0, 44.0, 48.0, 56.0, 64.0, 80.0, 96.0, 128.0,
    ] {
        let elements = ((mib * 1024.0 * 1024.0) / 4.0) as usize;
        let bytes = (elements * 4) as u64;
        let pattern: Vec<f32> = (0..elements).map(|i| (i % 1024) as f32).collect();
        let res = backend.resources();
        let src = res
            .upload_tensor("src", TensorShape::new(elements, 1), &pattern)
            .expect("upload");
        let mut staging = StagingBuffer::new(backend, bytes);
        let mut warm: Vec<f32> = vec![0.0; elements];

        for rep in 0..(WARMUP + PAIRS) {
            let (_, t) = timed_readback(
                backend,
                src.buffer(),
                bytes,
                ReadbackPlan {
                    staging: Some(&mut staging),
                    timer: timer.as_ref(),
                    dest: Some(&mut warm),
                    ..ReadbackPlan::default()
                },
            )
            .expect("readback");
            assert!(validate(&warm, elements), "part F bytes wrong");
            if rep < WARMUP {
                continue;
            }
            let r2 = t.gpu_copy_us.unwrap_or(f64::NAN);
            println!(
                "F\t{mib:.0}\t{bytes}\twarm\t{}\t{r2:.2}\t{:.2}\t{:.2}\t{:.3}",
                rep - WARMUP,
                t.memcpy_us,
                t.total_us,
                bytes as f64 / (r2 * 1e-6) / 1e9
            );
        }
    }
}
