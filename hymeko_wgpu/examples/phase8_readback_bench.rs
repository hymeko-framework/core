//! Phase 8 benchmark: readback reuse measured **through the production API**.
//!
//! Phase 7's numbers are not reused here. Every value this emits is measured
//! again through the code that actually ships.
//!
//! # Variants
//!
//! | id | staging | destination | how it is driven |
//! |----|---------|-------------|------------------|
//! | A | fresh | fresh | `read_tensor` — the legacy convenience API |
//! | B | fresh | reused | profiling probe (decomposition only) |
//! | C | reused | fresh | `read_tensor_into` + `take_result()` each rep |
//! | D | reused | reused | `read_tensor_into` — the reusable API |
//! | E | — | — | resident control: submit and wait, no readback |
//!
//! A, C, D and E run entirely through public API. B is not a shippable
//! configuration — nobody would deliberately reallocate staging while keeping a
//! destination — and exists only to attribute the D-over-A gap between the two
//! reuse axes. It goes through the profiling probe, which shares the same
//! engine step primitives.
//!
//! # Ordering
//!
//! Paired and alternating: the variant order is rotated by repetition index so
//! no variant is systematically measured first. Phase 5 established a ~5 %
//! order bias in this harness; Phase 7 confirmed it is not detectable under
//! this design, and the same discipline is kept here.
//!
//! Run:
//! ```text
//! cargo run -p hymeko_wgpu --release --example phase8_readback_bench
//! ```

use std::time::Instant;

use hymeko_tensor::compute::TensorShape;
use hymeko_wgpu::profiling::{ReadbackPlan, timed_readback};
use hymeko_wgpu::{StagingStorage, WgpuBackend};

const WARMUP: usize = 3;
const REPS: usize = 12;

/// Element counts covering the small control, the Phase 7 transition band, and
/// the sizes where the destination cost dominates. Values are element counts;
/// bytes are 4x these.
fn sizes_mib() -> Vec<f64> {
    vec![
        0.031_25, // 32 KiB small control
        32.0, 41.0, 43.0, 45.0, 47.0, 49.0, 51.0, // dense through the band
        64.0, 96.0, 97.0, 102.0, 128.0,
    ]
}

fn main() {
    // `--only X` restricts the matrix to one variant. The variants allocate and
    // free large blocks; running them interleaved lets one variant's allocation
    // churn perturb another, which is the very effect Phase 7 identified. This
    // switch measures a variant in isolation so that contamination can be
    // tested rather than assumed.
    let mut only: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--only"
            && let Some(v) = args.next()
        {
            only = Some(v.to_uppercase());
        }
    }
    let backend = match WgpuBackend::builder().label("phase8").build() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("no GPU adapter: {e}");
            return;
        }
    };
    let caps = backend.capabilities();
    eprintln!("{}", caps.summary());
    eprintln!("max_buffer_size = {} MiB", caps.max_buffer_size / (1 << 20));

    println!("variant\tmib\tbytes\trep\ttotal_us\tgbps\tstaging_allocs\tdest_cap");

    for mib in sizes_mib() {
        let bytes_f = mib * 1024.0 * 1024.0;
        let elements = (bytes_f / 4.0) as usize;
        if elements == 0 {
            continue;
        }
        let bytes = (elements * 4) as u64;
        if bytes > caps.max_buffer_size {
            eprintln!("skip {mib} MiB: beyond max_buffer_size");
            continue;
        }

        let pattern: Vec<f32> = (0..elements).map(|i| (i % 1024) as f32).collect();
        let tensor =
            match backend
                .resources()
                .upload_tensor("src", TensorShape::new(elements, 1), &pattern)
            {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("skip {mib} MiB: upload failed: {e}");
                    continue;
                }
            };

        // Persistent state per variant, so reuse is genuine across repetitions.
        let mut ws_c = backend.create_readback_workspace();
        let mut ws_d = backend.create_readback_workspace();
        let mut dest_b: Vec<f32> = Vec::new();

        let gbps = |us: f64| bytes as f64 / (us * 1e-6) / 1e9;

        for rep in 0..(WARMUP + REPS) {
            // Rotate the starting variant so none is always measured first.
            let order = ["A", "B", "C", "D", "E"];
            let start = rep % order.len();

            for k in 0..order.len() {
                let variant = order[(start + k) % order.len()];
                if only.as_ref().is_some_and(|o| o != variant) {
                    continue;
                }
                let (us, allocs, dcap) = match variant {
                    "A" => {
                        let t0 = Instant::now();
                        let v = backend.read_tensor(&tensor).expect("legacy read");
                        let us = t0.elapsed().as_secs_f64() * 1e6;
                        assert_eq!(v.len(), elements, "A returned the wrong length");
                        (us, 1u64, v.capacity())
                    }
                    "B" => {
                        // Fresh staging, reused destination. Decomposition only.
                        let mut fresh = StagingStorage::new();
                        let t0 = Instant::now();
                        let (_, _) = timed_readback(
                            &backend,
                            tensor.buffer(),
                            bytes,
                            ReadbackPlan {
                                staging: Some(&mut fresh),
                                dest: Some(&mut dest_b),
                                ..ReadbackPlan::default()
                            },
                        )
                        .expect("B readback");
                        let us = t0.elapsed().as_secs_f64() * 1e6;
                        assert_eq!(dest_b.len(), elements, "B returned the wrong length");
                        (us, fresh.allocations(), dest_b.capacity())
                    }
                    "C" => {
                        // Reused staging, surrendered destination each rep.
                        let t0 = Instant::now();
                        let n = backend
                            .read_tensor_into(&tensor, &mut ws_c)
                            .expect("C read")
                            .len();
                        let owned = ws_c.take_result();
                        let us = t0.elapsed().as_secs_f64() * 1e6;
                        assert_eq!(n, elements, "C returned the wrong length");
                        std::hint::black_box(&owned);
                        (us, ws_c.staging_allocations(), owned.capacity())
                    }
                    "D" => {
                        let t0 = Instant::now();
                        let got = backend
                            .read_tensor_into(&tensor, &mut ws_d)
                            .expect("D read");
                        let n = got.len();
                        let us = t0.elapsed().as_secs_f64() * 1e6;
                        assert_eq!(n, elements, "D returned the wrong length");
                        (us, ws_d.staging_allocations(), ws_d.destination_capacity())
                    }
                    // Resident control: the same submit-and-wait boundary with
                    // no readback at all, so a readback change that leaked into
                    // the dispatch path would show up here.
                    _ => {
                        let t0 = Instant::now();
                        let encoder = backend.encoder(Some("phase8.resident"));
                        let index = backend.submit(encoder);
                        backend.wait(index).expect("resident wait");
                        (t0.elapsed().as_secs_f64() * 1e6, 0, 0)
                    }
                };

                if rep >= WARMUP {
                    println!(
                        "{variant}\t{mib:.5}\t{bytes}\t{}\t{us:.2}\t{:.4}\t{allocs}\t{dcap}",
                        rep - WARMUP,
                        gbps(us)
                    );
                }
            }
        }

        // Structural evidence, printed to stderr so it cannot be confused with
        // a measurement row: after warm-up neither reusable variant allocated
        // more than once.
        eprintln!(
            "{mib:>9.5} MiB  C_staging_allocs={} D_staging_allocs={} D_dest_cap={}",
            ws_c.staging_allocations(),
            ws_d.staging_allocations(),
            ws_d.destination_capacity()
        );
    }
}
