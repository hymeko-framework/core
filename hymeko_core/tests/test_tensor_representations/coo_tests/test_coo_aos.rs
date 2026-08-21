#[cfg(test)]
mod test_tensor_coo_aos {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::tensor::representations::tensor_coo::TensorCoo;
    use log::info;
    use std::time::Instant;

    // ========================================================
    // Helpers
    // ========================================================

    /// Build a small known tensor: Fano-like 3-slice, 4×4 with 6 entries.
    fn make_known_coo() -> TensorCoo<f32> {
        let mut t = TensorCoo::with_meta(3, 4, 4);
        t.push(0, 0, 1, 1.0);
        t.push(0, 1, 0, 1.0);
        t.push(1, 2, 3, 2.5);
        t.push(1, 3, 2, 2.5);
        t.push(2, 0, 3, 0.5);
        t.push(2, 3, 0, 0.5);
        t
    }

    /// The same entries as `make_known_coo`, expressed as tuples for verification.
    const KNOWN_ENTRIES: [(usize, usize, usize, f32); 6] = [
        (0, 0, 1, 1.0),
        (0, 1, 0, 1.0),
        (1, 2, 3, 2.5),
        (1, 3, 2, 2.5),
        (2, 0, 3, 0.5),
        (2, 3, 0, 0.5),
    ];

    // ========================================================
    // Correctness: Basic API
    // ========================================================

    #[test]
    fn test_coo_empty() {
        log_test_header(
            "test_coo_empty",
            "Empty tensor has len 0 and is_empty true.",
        );
        let start = Instant::now();

        let t: TensorCoo<f32> = TensorCoo::with_meta(5, 10, 10);
        assert_eq!(t.len(), 0);
        assert!(t.is_empty());
        assert_eq!(t.num_slices, 5);
        assert_eq!(t.dim_i, 10);
        assert_eq!(t.dim_j, 10);

        log_test_footer("test_coo_empty", Some(start.elapsed()), "Empty tensor OK.");
    }

    #[test]
    fn test_coo_push_and_len() {
        log_test_header(
            "test_coo_push_and_len",
            "Push entries and verify len grows.",
        );
        let start = Instant::now();

        let mut t = TensorCoo::with_meta(2, 3, 3);
        assert_eq!(t.len(), 0);

        t.push(0, 1, 2, 3.14);
        assert_eq!(t.len(), 1);
        assert!(!t.is_empty());

        t.push(1, 0, 0, 2.71);
        assert_eq!(t.len(), 2);

        log_test_footer(
            "test_coo_push_and_len",
            Some(start.elapsed()),
            "Push/len OK.",
        );
    }

    #[test]
    fn test_coo_entry_accessor() {
        log_test_header(
            "test_coo_entry_accessor",
            "Indexed entry access returns correct values.",
        );
        let start = Instant::now();

        let t = make_known_coo();
        assert_eq!(t.len(), KNOWN_ENTRIES.len());

        for (idx, &(ek, ei, ej, ev)) in KNOWN_ENTRIES.iter().enumerate() {
            let e = t.entry(idx);
            assert_eq!(e.k, ek, "entry({}).k mismatch", idx);
            assert_eq!(e.i, ei, "entry({}).i mismatch", idx);
            assert_eq!(e.j, ej, "entry({}).j mismatch", idx);
            assert!(
                (e.v - ev).abs() < 1e-9,
                "entry({}).v mismatch: {} vs {}",
                idx,
                e.v,
                ev
            );
        }

        log_test_footer(
            "test_coo_entry_accessor",
            Some(start.elapsed()),
            "Entry accessor OK.",
        );
    }

    #[test]
    fn test_coo_iter() {
        log_test_header(
            "test_coo_iter",
            "Iterator visits all entries in push order.",
        );
        let start = Instant::now();

        let t = make_known_coo();
        let collected: Vec<(usize, usize, usize, f32)> =
            t.iter().map(|e| (e.k, e.i, e.j, e.v)).collect();

        assert_eq!(collected.len(), KNOWN_ENTRIES.len());
        for (got, &expected) in collected.iter().zip(KNOWN_ENTRIES.iter()) {
            assert_eq!(*got, expected);
        }

        log_test_footer("test_coo_iter", Some(start.elapsed()), "Iterator OK.");
    }

    #[test]
    fn test_coo_ordering_preserved() {
        log_test_header(
            "test_coo_ordering_preserved",
            "Push order is preserved exactly — no sorting, no reordering.",
        );
        let start = Instant::now();

        let mut t = TensorCoo::with_meta(1, 100, 100);
        // Push in deliberately non-sorted order
        t.push(0, 99, 0, 1.0);
        t.push(0, 0, 99, 2.0);
        t.push(0, 50, 50, 3.0);

        assert_eq!(t.entry(0).i, 99);
        assert_eq!(t.entry(1).i, 0);
        assert_eq!(t.entry(2).i, 50);

        log_test_footer(
            "test_coo_ordering_preserved",
            Some(start.elapsed()),
            "Order preserved OK.",
        );
    }

    #[test]
    fn test_coo_reserve_does_not_change_len() {
        log_test_header(
            "test_coo_reserve_does_not_change_len",
            "Reserve allocates but does not change len.",
        );
        let start = Instant::now();

        let mut t: TensorCoo<f64> = TensorCoo::with_meta(1, 10, 10);
        t.reserve(10_000);
        assert_eq!(t.len(), 0);
        assert!(t.is_empty());

        t.push(0, 0, 0, 1.0);
        assert_eq!(t.len(), 1);

        log_test_footer(
            "test_coo_reserve_does_not_change_len",
            Some(start.elapsed()),
            "Reserve OK.",
        );
    }

    // ========================================================
    // Correctness: into_soa() roundtrip
    // ========================================================

    #[test]
    fn test_coo_into_soa_roundtrip() {
        log_test_header(
            "test_coo_into_soa_roundtrip",
            "into_soa() produces matching separate arrays with correct metadata.",
        );
        let start = Instant::now();

        let t = make_known_coo();
        let soa = t.into_soa();

        assert_eq!(soa.num_slices, 3);
        assert_eq!(soa.dim_i, 4);
        assert_eq!(soa.dim_j, 4);
        assert_eq!(soa.k.len(), KNOWN_ENTRIES.len());
        assert_eq!(soa.i.len(), KNOWN_ENTRIES.len());
        assert_eq!(soa.j.len(), KNOWN_ENTRIES.len());
        assert_eq!(soa.v.len(), KNOWN_ENTRIES.len());

        for (idx, &(ek, ei, ej, ev)) in KNOWN_ENTRIES.iter().enumerate() {
            assert_eq!(soa.k[idx], ek, "soa.k[{}] mismatch", idx);
            assert_eq!(soa.i[idx], ei, "soa.i[{}] mismatch", idx);
            assert_eq!(soa.j[idx], ej, "soa.j[{}] mismatch", idx);
            assert!((soa.v[idx] - ev).abs() < 1e-9, "soa.v[{}] mismatch", idx);
        }

        log_test_footer(
            "test_coo_into_soa_roundtrip",
            Some(start.elapsed()),
            "SOA roundtrip OK.",
        );
    }

    #[test]
    fn test_coo_into_soa_empty() {
        log_test_header(
            "test_coo_into_soa_empty",
            "into_soa() on empty tensor produces empty arrays.",
        );
        let start = Instant::now();

        let t: TensorCoo<f32> = TensorCoo::with_meta(5, 10, 10);
        let soa = t.into_soa();

        assert_eq!(soa.k.len(), 0);
        assert_eq!(soa.v.len(), 0);
        assert_eq!(soa.num_slices, 5);
        assert_eq!(soa.dim_i, 10);

        log_test_footer(
            "test_coo_into_soa_empty",
            Some(start.elapsed()),
            "Empty SOA OK.",
        );
    }

    // ========================================================
    // Correctness: Dense view still works (integration)
    // ========================================================

    #[test]
    fn test_coo_dense_view_slice_correctness() {
        log_test_header(
            "test_coo_dense_view_slice_correctness",
            "dense_view_slice produces the expected matrix for each slice.",
        );
        let start = Instant::now();

        let t = make_known_coo();

        // Slice 0: entries (0,1,1.0) and (1,0,1.0)
        let m0 = hymeko_hnn::tensor::tensor::dense_view_slice(&t, 0);
        assert_eq!(m0.len(), 4);
        assert_eq!(m0[0].len(), 4);
        assert!(
            (m0[0][1] - 1.0).abs() < 1e-9,
            "m0[0][1] should be 1.0, got {}",
            m0[0][1]
        );
        assert!(
            (m0[1][0] - 1.0).abs() < 1e-9,
            "m0[1][0] should be 1.0, got {}",
            m0[1][0]
        );
        // Everything else in slice 0 should be zero
        assert!((m0[0][0]).abs() < 1e-9);
        assert!((m0[2][3]).abs() < 1e-9);
        info!("Slice 0 validated: 2 non-zeros at (0,1) and (1,0).");

        // Slice 1: entries (2,3,2.5) and (3,2,2.5)
        let m1 = hymeko_hnn::tensor::tensor::dense_view_slice(&t, 1);
        assert!((m1[2][3] - 2.5).abs() < 1e-9);
        assert!((m1[3][2] - 2.5).abs() < 1e-9);
        assert!((m1[0][0]).abs() < 1e-9);
        info!("Slice 1 validated: 2 non-zeros at (2,3) and (3,2).");

        // Slice 2: entries (0,3,0.5) and (3,0,0.5)
        let m2 = hymeko_hnn::tensor::tensor::dense_view_slice(&t, 2);
        assert!((m2[0][3] - 0.5).abs() < 1e-9);
        assert!((m2[3][0] - 0.5).abs() < 1e-9);
        info!("Slice 2 validated: 2 non-zeros at (0,3) and (3,0).");

        log_test_footer(
            "test_coo_dense_view_slice_correctness",
            Some(start.elapsed()),
            "All 3 slices produce correct dense matrices.",
        );
    }

    #[test]
    fn test_coo_project_sum_over_slices() {
        log_test_header(
            "test_coo_project_sum_over_slices",
            "Summing all slices produces the correct aggregate matrix.",
        );
        let start = Instant::now();

        let t = make_known_coo();
        let m = hymeko_hnn::tensor::tensor::project_sum_over_slices(&t);

        // (0,1): 1.0 from slice 0
        assert!((m[0][1] - 1.0).abs() < 1e-9);
        // (1,0): 1.0 from slice 0
        assert!((m[1][0] - 1.0).abs() < 1e-9);
        // (2,3): 2.5 from slice 1
        assert!((m[2][3] - 2.5).abs() < 1e-9);
        // (0,3): 0.5 from slice 2
        assert!((m[0][3] - 0.5).abs() < 1e-9);
        // (3,0): 0.5 from slice 2
        assert!((m[3][0] - 0.5).abs() < 1e-9);
        // (0,0): should be zero — no entries there
        assert!((m[0][0]).abs() < 1e-9);

        log_test_footer(
            "test_coo_project_sum_over_slices",
            Some(start.elapsed()),
            "Aggregate projection matches expected values.",
        );
    }

    // ========================================================
    // Correctness: Duplicate coalescing through dense view
    // ========================================================

    #[test]
    fn test_coo_duplicate_entries_coalesce_in_dense() {
        log_test_header(
            "test_coo_duplicate_entries_coalesce_in_dense",
            "Multiple pushes to the same (k,i,j) should sum in dense view.",
        );
        let start = Instant::now();

        let mut t = TensorCoo::with_meta(1, 3, 3);
        t.push(0, 1, 2, 1.0);
        t.push(0, 1, 2, 0.5); // duplicate coordinate
        t.push(0, 1, 2, 0.25); // triple

        let m = hymeko_hnn::tensor::tensor::dense_view_slice(&t, 0);
        let expected = 1.75f32;
        let eps = 1e-6f32;
        assert!(
            (m[1][2] - expected).abs() < eps,
            "Expected 1.0+0.5+0.25=1.75, got {}",
            m[1][2]
        );

        // The COO itself should have 3 entries (not coalesced)
        assert_eq!(t.len(), 3);

        log_test_footer(
            "test_coo_duplicate_entries_coalesce_in_dense",
            Some(start.elapsed()),
            "Duplicate entries sum correctly in dense view.",
        );
    }

    // ========================================================
    // Correctness: f64 precision
    // ========================================================

    #[test]
    fn test_coo_f64_precision() {
        log_test_header(
            "test_coo_f64_precision",
            "f64 entries maintain full precision through push/entry/soa.",
        );
        let start = Instant::now();

        let mut t: TensorCoo<f64> = TensorCoo::with_meta(1, 2, 2);
        let precise_val: f64 = std::f64::consts::PI * 1e15; // large, needs full f64
        t.push(0, 0, 1, precise_val);

        assert_eq!(t.entry(0).v, precise_val, "Entry accessor lost precision");

        let soa = t.into_soa();
        assert_eq!(soa.v[0], precise_val, "into_soa lost precision");

        log_test_footer(
            "test_coo_f64_precision",
            Some(start.elapsed()),
            "f64 precision OK.",
        );
    }

    // ========================================================
    // Stress: Large construction
    // ========================================================

    #[test]
    fn test_coo_large_construction() {
        log_test_header(
            "test_coo_large_construction",
            "Push 1M entries, verify len, spot-check first/last/middle.",
        );
        let start = Instant::now();

        let n = 1_000_000usize;
        let mut t = TensorCoo::with_meta(100, 1000, 1000);
        t.reserve(n);

        for idx in 0..n {
            t.push(idx % 100, idx % 1000, (idx * 7) % 1000, idx as f32 * 0.001);
        }

        assert_eq!(t.len(), n);

        // Spot-check first
        let e0 = t.entry(0);
        assert_eq!(e0.k, 0);
        assert_eq!(e0.i, 0);
        assert_eq!(e0.j, 0);
        assert!((e0.v - 0.0).abs() < 1e-9);

        // Spot-check last
        let last = t.entry(n - 1);
        assert_eq!(last.k, (n - 1) % 100);
        assert_eq!(last.i, (n - 1) % 1000);
        assert_eq!(last.j, ((n - 1) * 7) % 1000);

        // Spot-check middle
        let mid = n / 2;
        let em = t.entry(mid);
        assert_eq!(em.k, mid % 100);
        assert_eq!(em.i, mid % 1000);

        let elapsed = start.elapsed();
        info!(
            "Constructed and verified 1M entries in {:.3}ms",
            elapsed.as_secs_f64() * 1000.0
        );

        log_test_footer(
            "test_coo_large_construction",
            Some(elapsed),
            &format!(
                "1M entries: push + verify in {:.1}ms",
                elapsed.as_secs_f64() * 1000.0
            ),
        );
    }

    #[test]
    fn test_coo_large_into_soa() {
        log_test_header(
            "test_coo_large_into_soa",
            "into_soa on 1M entries: verify length and spot-check consistency.",
        );
        let start = Instant::now();

        let n = 1_000_000usize;
        let mut t = TensorCoo::with_meta(100, 1000, 1000);
        t.reserve(n);
        for idx in 0..n {
            t.push(idx % 100, idx % 1000, (idx * 7) % 1000, idx as f32 * 0.001);
        }

        let soa_start = Instant::now();
        let soa = t.into_soa();
        let soa_elapsed = soa_start.elapsed();

        assert_eq!(soa.k.len(), n);
        assert_eq!(soa.i.len(), n);
        assert_eq!(soa.j.len(), n);
        assert_eq!(soa.v.len(), n);

        // Spot-check
        assert_eq!(soa.k[0], 0);
        assert_eq!(soa.i[n - 1], (n - 1) % 1000);
        assert_eq!(soa.j[n / 2], ((n / 2) * 7) % 1000);

        info!(
            "into_soa for 1M entries took {:.3}ms",
            soa_elapsed.as_secs_f64() * 1000.0
        );

        log_test_footer(
            "test_coo_large_into_soa",
            Some(start.elapsed()),
            &format!(
                "SOA transpose: {:.1}ms for 1M entries",
                soa_elapsed.as_secs_f64() * 1000.0
            ),
        );
    }

    // ========================================================
    // Performance: Construction throughput (timed assertion)
    // ========================================================

    #[test]
    fn test_coo_construction_throughput() {
        log_test_header(
            "test_coo_construction_throughput",
            "Push 5M entries with reserve. Should complete well under 1 second.",
        );

        let n = 5_000_000usize;
        let mut t = TensorCoo::with_meta(100, 10_000, 10_000);
        t.reserve(n);

        let start = Instant::now();
        for idx in 0..n {
            t.push(idx % 100, idx % 10_000, (idx * 13) % 10_000, 1.0f32);
        }
        let elapsed = start.elapsed();

        assert_eq!(t.len(), n);
        let throughput_m = n as f64 / elapsed.as_secs_f64() / 1_000_000.0;
        info!(
            "Push throughput: {:.1}M entries/sec ({:.1}ms for {}M entries)",
            throughput_m,
            elapsed.as_secs_f64() * 1000.0,
            n / 1_000_000,
        );

        // Sanity: 5M pushes should complete in under 500ms on any reasonable hardware.
        // This is a generous bound — we expect ~50-100ms.
        assert!(
            elapsed.as_secs_f64() < 0.5,
            "Push throughput too slow: {:.1}ms for 5M entries",
            elapsed.as_secs_f64() * 1000.0,
        );

        log_test_footer(
            "test_coo_construction_throughput",
            Some(elapsed),
            &format!("{:.0}M entries/sec", throughput_m),
        );
    }

    #[test]
    fn test_coo_iteration_throughput() {
        log_test_header(
            "test_coo_iteration_throughput",
            "Iterate 5M entries and accumulate. Should complete well under 500ms.",
        );

        let n = 5_000_000usize;
        let mut t = TensorCoo::with_meta(100, 10_000, 10_000);
        t.reserve(n);
        for idx in 0..n {
            t.push(idx % 100, idx % 10_000, (idx * 13) % 10_000, 1.0f32);
        }

        // Iterate and accumulate to prevent dead code elimination
        let start = Instant::now();
        let mut sum_k: usize = 0;
        let mut sum_v: f32 = 0.0;
        for e in t.iter() {
            sum_k += e.k;
            sum_v += e.v;
        }
        let elapsed = start.elapsed();

        // Prevent optimizer from removing the loop
        assert!(sum_k > 0);
        assert!(sum_v > 0.0);

        let throughput_m = n as f64 / elapsed.as_secs_f64() / 1_000_000.0;
        info!(
            "Iteration throughput: {:.1}M entries/sec ({:.1}ms for {}M entries)",
            throughput_m,
            elapsed.as_secs_f64() * 1000.0,
            n / 1_000_000,
        );

        assert!(
            elapsed.as_secs_f64() < 0.5,
            "Iteration too slow: {:.1}ms for 5M entries",
            elapsed.as_secs_f64() * 1000.0,
        );

        log_test_footer(
            "test_coo_iteration_throughput",
            Some(elapsed),
            &format!(
                "{:.0}M entries/sec (sum_k={}, sum_v={:.0})",
                throughput_m, sum_k, sum_v
            ),
        );
    }

    #[test]
    fn test_coo_construction_without_reserve() {
        log_test_header(
            "test_coo_construction_without_reserve",
            "Push 1M entries WITHOUT reserve — measures amortized realloc cost.",
        );

        let n = 1_000_000usize;
        let mut t = TensorCoo::with_meta(100, 10_000, 10_000);
        // Deliberately NO reserve()

        let start = Instant::now();
        for idx in 0..n {
            t.push(idx % 100, idx % 10_000, (idx * 13) % 10_000, 1.0f32);
        }
        let elapsed = start.elapsed();

        assert_eq!(t.len(), n);
        let throughput_m = n as f64 / elapsed.as_secs_f64() / 1_000_000.0;
        info!(
            "Push without reserve: {:.1}M entries/sec ({:.1}ms for 1M entries)",
            throughput_m,
            elapsed.as_secs_f64() * 1000.0,
        );

        // Even without reserve, 1M pushes to a single Vec should complete
        // in well under 200ms. The old 4-Vec version would take longer
        // because each realloc cycle hit 4 independent vectors.
        assert!(
            elapsed.as_secs_f64() < 0.2,
            "Push without reserve too slow: {:.1}ms for 1M entries",
            elapsed.as_secs_f64() * 1000.0,
        );

        log_test_footer(
            "test_coo_construction_without_reserve",
            Some(elapsed),
            &format!("{:.0}M entries/sec (no reserve)", throughput_m),
        );
    }
}
