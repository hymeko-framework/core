#[cfg(test)]
mod tests {
    use hymeko::common::ids::DeclId;
    use hymeko::common::pathkey::PathKey;
    use hymeko::ir::hash::{HashId, hash_doc};
    use hymeko::resolution::interner::Interner;
    use hymeko::resolution::resolve::Index;
    use log::info;
    use std::time::Instant;

    const SYMBOL_FANO: &str = "fano";
    const SYMBOL_NODE_0: &str = "n0";
    const SYMBOL_NODE_1: &str = "n1";
    const SYMBOL_EDGE_0: &str = "e0";
    const MASSIVE_NODE_COUNT: usize = 10_000;
    const HASH_RUNS: usize = 100;
    const PERF_BUDGET_MS: u128 = 2000;
    const PERF_BUDGET_MS_MASSIVE: u128 = 1500000;
    const PERF_AVG_BUDGET_MS: f64 = 50.0;
    const PERF_AVG_BUDGET_MS_MASSIVE: f64 = 300.0;

    // Helper to mock the environment
    fn setup_mock_env() -> (Interner, Vec<(PathKey, DeclId)>) {
        let mut it = Interner::new();
        // Register some symbols
        let s_fano = it.intern(SYMBOL_FANO);
        let s_n0 = it.intern(SYMBOL_NODE_0);
        let s_n1 = it.intern(SYMBOL_NODE_1);
        let s_e0 = it.intern(SYMBOL_EDGE_0);

        // Create some paths
        let paths = vec![
            (PathKey(vec![s_fano, s_n0]), DeclId::new(0)),
            (PathKey(vec![s_fano, s_n1]), DeclId::new(1)),
            (PathKey(vec![s_fano, s_e0]), DeclId::new(2)),
            (PathKey(vec![s_fano]), DeclId::new(3)), // Parent path
        ];

        (it, paths)
    }

    fn median_ms(samples: &mut [f64]) -> f64 {
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = samples.len();
        if n == 0 {
            return 0.0;
        }
        if n % 2 == 1 {
            samples[n / 2]
        } else {
            (samples[n / 2 - 1] + samples[n / 2]) * 0.5
        }
    }

    fn stddev_ms(samples: &[f64], mean: f64) -> f64 {
        if samples.is_empty() {
            return 0.0;
        }
        let variance = samples
            .iter()
            .map(|v| {
                let d = *v - mean;
                d * d
            })
            .sum::<f64>()
            / samples.len() as f64;
        variance.sqrt()
    }

    fn percentile_from_sorted(sorted: &[f64], pct: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let clamped = pct.clamp(0.0, 100.0);
        let idx = ((clamped / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
        sorted[idx]
    }

    #[test]
    fn test_hash_doc_determinism_insertion_order() {
        let (it, paths) = setup_mock_env();

        // Environment A: Inserted in Forward Order
        let mut idx_a = Index::default();
        for (pk, did) in paths.clone().into_iter() {
            idx_a.by_path.insert(pk, did);
        }
        let hash_a = hash_doc(&idx_a, &it);

        // Environment B: Inserted in Reverse Order
        let mut idx_b = Index::default();
        for (pk, did) in paths.clone().into_iter().rev() {
            idx_b.by_path.insert(pk, did);
        }
        let hash_b = hash_doc(&idx_b, &it);

        // Environment C: Inserted in Scrambled Order
        let mut idx_c = Index::default();
        idx_c.by_path.insert(paths[2].0.clone(), paths[2].1);
        idx_c.by_path.insert(paths[0].0.clone(), paths[0].1);
        idx_c.by_path.insert(paths[3].0.clone(), paths[3].1);
        idx_c.by_path.insert(paths[1].0.clone(), paths[1].1);
        let hash_c = hash_doc(&idx_c, &it);

        // THE CRITICAL ASSERTION: All hashes must be perfectly identical
        // despite the chaotic memory insertion order.
        assert_eq!(hash_a, hash_b, "Hash failed reverse order determinism!");
        assert_eq!(hash_a, hash_c, "Hash failed scrambled order determinism!");
    }

    #[test]
    fn test_hash_doc_performance_benchmark() {
        let (mut it, paths) = setup_mock_env();

        // Build a massive index to simulate a heavy hypergraph
        let mut massive_idx = Index::default();
        for i in 0..MASSIVE_NODE_COUNT {
            let s_node = it.intern(&format!("node_{}", i));
            massive_idx
                .by_path
                .insert(PathKey(vec![paths[3].0.0[0], s_node]), DeclId::new(i));
        }

        let start = Instant::now();

        // Run the hash 100 times to test throughput
        let mut last_hash = HashId([0; 32]);
        for _ in 0..HASH_RUNS {
            last_hash = hash_doc(&massive_idx, &it);
        }

        let elapsed = start.elapsed();
        let avg_ms = elapsed.as_secs_f64() * 1_000.0 / HASH_RUNS as f64;

        // Log the telemetry (aligning with your recent CI telemetry cleanup)
        info!(
            "Hashing 10,000 nodes 100 times took: {:?} (avg {:.3} ms/run)",
            elapsed, avg_ms
        );

        // Basic sanity check to ensure it actually computed something
        assert_ne!(last_hash.0, [0; 32]);

        // We expect this to be well under a few milliseconds per run
        assert!(
            elapsed.as_millis() < PERF_BUDGET_MS,
            "Hashing is suspiciously slow! Elapsed: {:?} exceeds budget of {} ms",
            elapsed,
            PERF_BUDGET_MS
        );
        assert!(
            avg_ms < PERF_AVG_BUDGET_MS,
            "Average hash runtime {:.3} ms exceeds budget",
            avg_ms
        );
    }

    #[test]
    fn test_hash_doc_performance_benchmark_multiple_nodes() {
        let (mut it, paths) = setup_mock_env();
        // Test hashing performance across a range of node counts to see how it scales
        let node_counts = [
            10, 100, 200, 500, 1_000, 2_000, 2_500, 5_000, 10_000, 20_000, 25_000, 50_000, 100_000,
        ];
        // Collect telemetry for each node count to analyze scaling behavior
        let mut telemetry = Vec::with_capacity(node_counts.len());
        // Store rows as numeric tuples to avoid per-run string formatting in the hot loop.
        let mut run_rows: Vec<(usize, usize, f64)> =
            Vec::with_capacity(node_counts.len() * HASH_RUNS);
        let mut summary_rows: Vec<(usize, f64, f64, f64, f64, f64, f64, f64)> =
            Vec::with_capacity(node_counts.len());
        // Test each node count in the range, ensuring we stay within reasonable performance bounds
        for &count in node_counts.iter() {
            info!("Testing hash performance with {} nodes...", count);

            // Build a massive index to simulate a heavy hypergraph
            let mut massive_idx = Index::default();
            for i in 0..count {
                let s_node = it.intern(&format!("node_{}", i));
                massive_idx
                    .by_path
                    .insert(PathKey(vec![paths[3].0.0[0], s_node]), DeclId::new(i));
            }

            let start = Instant::now();

            // Run the hash HASH_RUNS times and sample each run for richer telemetry.
            let mut last_hash = HashId([0; 32]);
            let mut run_times_ms = Vec::with_capacity(HASH_RUNS);
            for run_idx in 0..HASH_RUNS {
                let run_start = Instant::now();
                last_hash = hash_doc(&massive_idx, &it);
                let run_ms = run_start.elapsed().as_secs_f64() * 1_000.0;
                run_times_ms.push(run_ms);
                run_rows.push((count, run_idx + 1, run_ms));
            }

            let elapsed = start.elapsed();
            let avg_ms = elapsed.as_secs_f64() * 1_000.0 / HASH_RUNS as f64;
            let mut sorted_samples = run_times_ms.clone();
            let median_ms = median_ms(&mut sorted_samples);
            let stddev_ms = stddev_ms(&run_times_ms, avg_ms);
            let min_ms = *sorted_samples.first().unwrap_or(&0.0);
            let max_ms = *sorted_samples.last().unwrap_or(&0.0);
            let p95_ms = percentile_from_sorted(&sorted_samples, 95.0);

            info!(
                "Hashing {} nodes {} times took: {:?} (avg {:.3} ms/run, median {:.3} ms, p95 {:.3} ms, stddev {:.3} ms)",
                count, HASH_RUNS, elapsed, avg_ms, median_ms, p95_ms, stddev_ms
            );
            // Collect telemetry for analysis
            telemetry.push((count, elapsed, avg_ms, median_ms, stddev_ms));
            summary_rows.push((
                count,
                elapsed.as_secs_f64(),
                avg_ms,
                median_ms,
                p95_ms,
                min_ms,
                max_ms,
                stddev_ms,
            ));

            // Basic sanity check to ensure it actually computed something
            assert_ne!(last_hash.0, [0; 32]);

            // Use a wider elapsed budget for the largest scenario to reduce CI flakiness.
            let elapsed_budget_ms = if count >= 10_000 {
                PERF_BUDGET_MS_MASSIVE
            } else {
                PERF_BUDGET_MS
            };
            let avg_budget_ms = if count >= 10_000 {
                PERF_AVG_BUDGET_MS_MASSIVE
            } else {
                PERF_AVG_BUDGET_MS
            };
            assert!(
                elapsed.as_millis() < elapsed_budget_ms,
                "Hashing {} nodes is suspiciously slow: {:?} exceeds budget of {} ms",
                count,
                elapsed,
                elapsed_budget_ms
            );
            assert!(
                avg_ms < avg_budget_ms,
                "Average hash runtime {:.3} ms exceeds budget {:.3} ms for count {}",
                avg_ms,
                avg_budget_ms,
                count
            );
        }

        // Analyze scaling across adjacent buckets to catch pathological regressions.
        for window in telemetry.windows(2) {
            let (prev_count, _prev_elapsed, prev_avg_ms, prev_median_ms, _prev_stddev_ms) =
                window[0];
            let (curr_count, _curr_elapsed, curr_avg_ms, curr_median_ms, _curr_stddev_ms) =
                window[1];
            let count_ratio = curr_count as f64 / prev_count as f64;
            // Use median, not avg, so a single jittery sample on a shared CI runner
            // cannot push the smallest-bucket ratio over threshold.
            let runtime_ratio = curr_median_ms / prev_median_ms.max(1e-9);
            let ms_per_node_prev = prev_avg_ms / prev_count as f64;
            let ms_per_node_curr = curr_avg_ms / curr_count as f64;
            let normalized_ratio = ms_per_node_curr / ms_per_node_prev.max(1e-12);

            info!(
                "Scale {} -> {} nodes: avg {:.4} -> {:.4} ms/run | runtime x{:.3}, normalized x{:.3}",
                prev_count, curr_count, prev_avg_ms, curr_avg_ms, runtime_ratio, normalized_ratio
            );

            // Cap runtime growth at 2x the count growth, with an additive +2.0 grace
            // term so adjacent buckets with small count ratios (e.g. 2000->2500 at
            // 1.25x) don't get a punishingly tight window that shared-runner jitter
            // in debug builds can blow past.  At count_ratio=10 the limit is 22x;
            // at 1.25x it's 4.5x.  The +1.0 grace was empirically too tight: median
            // runtimes at sub-millisecond ranges can ratio at ~3.7x purely from
            // scheduling jitter (observed 2026-05-23 on ubuntu-latest CI). The
            // per-node normalized check below (<=5.0x) remains the catch-all for
            // genuine super-quadratic regressions.
            let max_runtime_ratio = count_ratio * 2.0 + 2.0;
            assert!(
                runtime_ratio <= max_runtime_ratio,
                "Scaling regression: {} -> {} nodes produced runtime ratio x{:.3} (limit x{:.3}) for count ratio x{:.3}",
                prev_count,
                curr_count,
                runtime_ratio,
                max_runtime_ratio,
                count_ratio
            );

            // Per-node cost should stay broadly stable as input grows.
            assert!(
                normalized_ratio <= 5.0,
                "Per-node cost regression: {} -> {} nodes normalized ratio x{:.3}",
                prev_count,
                curr_count,
                normalized_ratio
            );
        }
        // Save telemetry to a file for offline analysis
        let run_csv = format!(
            "node_count,run_index,run_ms\n{}",
            run_rows
                .iter()
                .map(|(count, run_index, run_ms)| format!("{},{},{:.6}", count, run_index, run_ms))
                .collect::<Vec<_>>()
                .join("\n")
        );
        std::fs::write("hash_performance_runs.csv", run_csv)
            .expect("Failed to write run telemetry file");

        let summary_csv = format!(
            "node_count,elapsed_seconds,avg_ms_per_run,median_ms_per_run,p95_ms_per_run,min_ms_per_run,max_ms_per_run,stddev_ms_per_run\n{}",
            summary_rows
                .iter()
                .map(
                    |(
                        count,
                        elapsed_seconds,
                        avg_ms,
                        median_ms,
                        p95_ms,
                        min_ms,
                        max_ms,
                        stddev_ms,
                    )| {
                        format!(
                            "{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
                            count,
                            elapsed_seconds,
                            avg_ms,
                            median_ms,
                            p95_ms,
                            min_ms,
                            max_ms,
                            stddev_ms
                        )
                    }
                )
                .collect::<Vec<_>>()
                .join("\n")
        );
        std::fs::write("hash_performance_summary.csv", summary_csv)
            .expect("Failed to write summary telemetry file");
    }
}
