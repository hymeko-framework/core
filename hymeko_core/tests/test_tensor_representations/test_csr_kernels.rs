//! Regression tests for the CSR SpMV / SpMM kernels.
//!
//! APPROVED-CORE-EDIT: csr-spmv-spmm-into (2026-08-24) added `spmv_into` /
//! `spmm_into` with explicit serial and Rayon variants. The approval carried
//! three constraints; this module is where two of them are enforced:
//!
//!   * **no change to floating-point accumulation order** — asserted as
//!     *bitwise* equality between the serial and parallel paths, not as a
//!     tolerance. A tolerance assertion would pass even if the parallel path
//!     reassociated the sums, which is exactly the defect worth catching.
//!   * **fully-overwritten output** — the `*_into` forms must leave the caller's
//!     buffer holding the result regardless of what was in it, so every kernel
//!     is also driven with a poisoned (non-zero sentinel) buffer.
//!
//! Correctness against a *dense* reference is done at small sizes, where the
//! O(n·m) reference is affordable and definitive. At medium/large sizes the
//! dense reference is replaced by serial-vs-parallel equality, which is what
//! the parallelisation can actually break.

#[cfg(test)]
mod test_csr_kernels {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use crate::test_tensor_representations::constants::*;
    use hymeko::tensor::representations::tensor_csr::TensorCsr;
    use hymeko_hnn::workloads::{DegreeShape, SparseSpec};
    use log::info;
    use std::time::Instant;

    const POISON: f32 = -999.5;

    // -----------------------------------------------------------------
    // helpers
    // -----------------------------------------------------------------

    /// Expand a CSR matrix to dense row-major form. Test-only; O(rows·cols).
    fn to_dense(csr: &TensorCsr<f32>) -> Vec<Vec<f32>> {
        let mut dense = vec![vec![0.0f32; csr.num_cols]; csr.num_rows];
        for i in 0..csr.num_rows {
            for k in csr.row_ptr[i]..csr.row_ptr[i + 1] {
                // `+=` not `=`: a duplicate column in a row means an
                // uncoalesced matrix, and the kernels sum such entries.
                dense[i][csr.col_ind[k]] += csr.val[k];
            }
        }
        dense
    }

    /// Dense `y = A x`, accumulating in column order — the same order the CSR
    /// kernel visits a row's entries when the row is sorted by column.
    fn dense_matvec(dense: &[Vec<f32>], x: &[f32]) -> Vec<f32> {
        dense
            .iter()
            .map(|row| row.iter().zip(x.iter()).map(|(&a, &b)| a * b).sum())
            .collect()
    }

    /// Dense `Y = A X`, X row-major `cols × fd`.
    fn dense_matmul(dense: &[Vec<f32>], x: &[f32], fd: usize) -> Vec<f32> {
        let rows = dense.len();
        let mut y = vec![0.0f32; rows * fd];
        for (i, row) in dense.iter().enumerate() {
            for (j, &a) in row.iter().enumerate() {
                if a == 0.0 {
                    continue;
                }
                for f in 0..fd {
                    y[i * fd + f] += a * x[j * fd + f];
                }
            }
        }
        y
    }

    fn assert_close(a: &[f32], b: &[f32], eps: f32, msg: &str) {
        assert_eq!(a.len(), b.len(), "len mismatch: {msg}");
        for i in 0..a.len() {
            let d = (a[i] - b[i]).abs();
            assert!(d <= eps, "{msg} at i={i}: {} vs {} (diff {d})", a[i], b[i]);
        }
    }

    /// Bitwise equality. Deliberately stricter than `assert_close`: the whole
    /// claim behind the parallel path is that it does not reassociate.
    fn assert_bitwise(a: &[f32], b: &[f32], msg: &str) {
        assert_eq!(a.len(), b.len(), "len mismatch: {msg}");
        for i in 0..a.len() {
            assert_eq!(
                a[i].to_bits(),
                b[i].to_bits(),
                "{msg} at i={i}: {} vs {} are not bitwise equal",
                a[i],
                b[i]
            );
        }
    }

    /// Hand-built 4x5 matrix with an empty row (row 2) and an unsorted,
    /// duplicate-column row (row 3) — shapes a generator will not produce.
    fn handmade() -> TensorCsr<f32> {
        TensorCsr {
            num_rows: 4,
            num_cols: 5,
            //        r0      r1        r2(empty) r3
            row_ptr: vec![0, 2, 5, 5, 8],
            col_ind: vec![0, 3, 1, 2, 4, 4, 0, 4],
            val: vec![1.5, -2.0, 0.25, 3.0, 1.0, 2.0, -1.0, 0.5],
        }
    }

    // -----------------------------------------------------------------
    // spmv
    // -----------------------------------------------------------------

    #[test]
    fn test_spmv_matches_dense_reference() {
        log_test_header(
            "test_spmv_matches_dense_reference",
            "Validates spmv/spmv_into against an independent dense matvec.",
        );
        let start = Instant::now();

        let csr = handmade();
        let x = vec![1.0f32, 2.0, -0.5, 4.0, 0.75];
        let expected = dense_matvec(&to_dense(&csr), &x);

        let got = csr.spmv(&x);
        assert_close(&got, &expected, EPS_F32_ULTRA, "spmv vs dense");

        let mut into = vec![POISON; csr.num_rows];
        csr.spmv_into(&x, &mut into);
        assert_close(&into, &expected, EPS_F32_ULTRA, "spmv_into vs dense");

        log_test_footer(
            "test_spmv_matches_dense_reference",
            Some(start.elapsed()),
            "spmv and spmv_into reproduced the dense reference on a matrix with an empty and a duplicate-column row.",
        );
    }

    #[test]
    fn test_spmv_serial_matches_parallel_bitwise() {
        log_test_header(
            "test_spmv_serial_matches_parallel_bitwise",
            "Asserts the Rayon SpMV path reassociates nothing, at three scales.",
        );
        let start = Instant::now();

        for (label, spec) in [
            ("small", SparseSpec::small()),
            ("medium", SparseSpec::medium()),
            (
                "irregular",
                SparseSpec {
                    shape: DegreeShape::Irregular,
                    ..SparseSpec::medium()
                },
            ),
        ] {
            let csr = spec.build();
            let x = spec.dense_vector();

            let mut serial = vec![POISON; csr.num_rows];
            let mut parallel = vec![POISON; csr.num_rows];
            csr.spmv_into_serial(&x, &mut serial);
            csr.spmv_into_parallel(&x, &mut parallel);

            assert_bitwise(
                &serial,
                &parallel,
                &format!("spmv serial vs parallel [{label}]"),
            );

            // the allocating wrapper and the dispatcher must agree too
            assert_bitwise(&csr.spmv(&x), &serial, &format!("spmv wrapper [{label}]"));
            let mut dispatched = vec![POISON; csr.num_rows];
            csr.spmv_into(&x, &mut dispatched);
            assert_bitwise(
                &dispatched,
                &serial,
                &format!("spmv_into dispatch [{label}]"),
            );

            info!(
                "[{label}] spmv bitwise-identical across all four entry points ({} rows, {} nnz)",
                csr.num_rows,
                csr.nnz()
            );
        }

        log_test_footer(
            "test_spmv_serial_matches_parallel_bitwise",
            Some(start.elapsed()),
            "Serial, parallel, dispatched and allocating SpMV agree bit-for-bit.",
        );
    }

    // -----------------------------------------------------------------
    // spmm
    // -----------------------------------------------------------------

    #[test]
    fn test_spmm_matches_dense_reference_across_feature_dims() {
        log_test_header(
            "test_spmm_matches_dense_reference_across_feature_dims",
            "Validates spmm/spmm_into against a dense matmul for fd in {1,8,32,128}.",
        );
        let start = Instant::now();

        let csr = handmade();
        let dense = to_dense(&csr);

        for fd in [1usize, 8, 32, 128] {
            let x: Vec<f32> = (0..csr.num_cols * fd)
                .map(|t| 0.5 + (t % 13) as f32 * 0.125)
                .collect();
            let expected = dense_matmul(&dense, &x, fd);

            let got = csr.spmm(&x, fd);
            assert_close(
                &got,
                &expected,
                EPS_F32_STRICT,
                &format!("spmm vs dense fd={fd}"),
            );

            let mut into = vec![POISON; csr.num_rows * fd];
            csr.spmm_into(&x, fd, &mut into);
            assert_close(
                &into,
                &expected,
                EPS_F32_STRICT,
                &format!("spmm_into vs dense fd={fd}"),
            );
        }

        log_test_footer(
            "test_spmm_matches_dense_reference_across_feature_dims",
            Some(start.elapsed()),
            "spmm reproduced the dense reference at every benchmarked feature dimension.",
        );
    }

    #[test]
    fn test_spmm_serial_matches_parallel_bitwise() {
        log_test_header(
            "test_spmm_serial_matches_parallel_bitwise",
            "Asserts the Rayon SpMM path reassociates nothing, across feature dims.",
        );
        let start = Instant::now();

        let spec = SparseSpec {
            shape: DegreeShape::Irregular,
            ..SparseSpec::medium()
        };
        let csr = spec.build();

        for fd in [1usize, 8, 32] {
            let x = spec.dense_block(fd);
            let mut serial = vec![POISON; csr.num_rows * fd];
            let mut parallel = vec![POISON; csr.num_rows * fd];

            csr.spmm_into_serial(&x, fd, &mut serial);
            csr.spmm_into_parallel(&x, fd, &mut parallel);
            assert_bitwise(
                &serial,
                &parallel,
                &format!("spmm serial vs parallel fd={fd}"),
            );

            assert_bitwise(&csr.spmm(&x, fd), &serial, &format!("spmm wrapper fd={fd}"));
        }

        log_test_footer(
            "test_spmm_serial_matches_parallel_bitwise",
            Some(start.elapsed()),
            "Serial, parallel and allocating SpMM agree bit-for-bit on an irregular matrix.",
        );
    }

    #[test]
    fn test_spmm_fd1_agrees_with_spmv() {
        log_test_header(
            "test_spmm_fd1_agrees_with_spmv",
            "Cross-checks the two kernels against each other at feature_dim = 1.",
        );
        let start = Instant::now();

        let spec = SparseSpec::small();
        let csr = spec.build();
        let x = spec.dense_vector();

        let via_spmv = csr.spmv(&x);
        let via_spmm = csr.spmm(&x, 1);
        assert_bitwise(&via_spmv, &via_spmm, "spmv vs spmm(fd=1)");

        log_test_footer(
            "test_spmm_fd1_agrees_with_spmv",
            Some(start.elapsed()),
            "SpMM at feature_dim=1 is bit-identical to SpMV.",
        );
    }

    // -----------------------------------------------------------------
    // contracts and degenerate shapes
    // -----------------------------------------------------------------

    #[test]
    fn test_into_kernels_fully_overwrite_a_poisoned_buffer() {
        log_test_header(
            "test_into_kernels_fully_overwrite_a_poisoned_buffer",
            "Guards the 'fully overwritten, never accumulated' postcondition.",
        );
        let start = Instant::now();

        let csr = handmade();
        let x = vec![1.0f32, 2.0, -0.5, 4.0, 0.75];

        // Run twice into the SAME buffer. If any kernel accumulated instead of
        // overwriting, the second run would double the result.
        let mut y = vec![POISON; csr.num_rows];
        csr.spmv_into(&x, &mut y);
        let first = y.clone();
        csr.spmv_into(&x, &mut y);
        assert_bitwise(&first, &y, "spmv_into is not idempotent - it accumulates");

        for fd in [1usize, 4] {
            let xb: Vec<f32> = (0..csr.num_cols * fd)
                .map(|t| 1.0 + t as f32 * 0.5)
                .collect();
            let mut yb = vec![POISON; csr.num_rows * fd];
            csr.spmm_into(&xb, fd, &mut yb);
            let first = yb.clone();
            csr.spmm_into(&xb, fd, &mut yb);
            assert_bitwise(&first, &yb, "spmm_into is not idempotent - it accumulates");

            // and the same for the explicitly parallel variant
            let mut par = vec![POISON; csr.num_rows * fd];
            csr.spmm_into_parallel(&xb, fd, &mut par);
            assert_bitwise(&first, &par, "spmm_into_parallel disagrees after poisoning");
        }

        log_test_footer(
            "test_into_kernels_fully_overwrite_a_poisoned_buffer",
            Some(start.elapsed()),
            "Every *_into kernel overwrote its output; running twice changed nothing.",
        );
    }

    #[test]
    fn test_empty_matrix() {
        log_test_header("test_empty_matrix", "Zero rows, zero cols, zero non-zeros.");
        let start = Instant::now();

        let csr: TensorCsr<f32> = TensorCsr {
            num_rows: 0,
            num_cols: 0,
            row_ptr: vec![0],
            col_ind: Vec::new(),
            val: Vec::new(),
        };

        assert!(
            csr.spmv(&[]).is_empty(),
            "spmv on an empty matrix must be empty"
        );
        assert!(
            csr.spmm(&[], 4).is_empty(),
            "spmm on an empty matrix must be empty"
        );

        let mut y: Vec<f32> = Vec::new();
        csr.spmv_into_serial(&[], &mut y);
        csr.spmv_into_parallel(&[], &mut y);
        csr.spmm_into_serial(&[], 4, &mut y);
        csr.spmm_into_parallel(&[], 4, &mut y);

        log_test_footer(
            "test_empty_matrix",
            Some(start.elapsed()),
            "All four kernels handled the empty matrix without panicking.",
        );
    }

    #[test]
    fn test_singleton_and_all_empty_rows() {
        log_test_header(
            "test_singleton_and_all_empty_rows",
            "1x1 with one entry, and a matrix whose every row is empty.",
        );
        let start = Instant::now();

        let single = TensorCsr {
            num_rows: 1,
            num_cols: 1,
            row_ptr: vec![0, 1],
            col_ind: vec![0],
            val: vec![2.5f32],
        };
        assert_close(&single.spmv(&[4.0]), &[10.0], EPS_F32_ULTRA, "1x1 spmv");
        assert_close(
            &single.spmm(&[4.0, 1.0], 2),
            &[10.0, 2.5],
            EPS_F32_ULTRA,
            "1x1 spmm fd=2",
        );

        // structurally empty: rows exist, none has a non-zero
        let hollow: TensorCsr<f32> = TensorCsr {
            num_rows: 5,
            num_cols: 3,
            row_ptr: vec![0; 6],
            col_ind: Vec::new(),
            val: Vec::new(),
        };
        let x = vec![1.0f32, 2.0, 3.0];
        assert_close(
            &hollow.spmv(&x),
            &vec![0.0; 5],
            EPS_F32_ULTRA,
            "hollow spmv",
        );

        let mut poisoned = vec![POISON; 5];
        hollow.spmv_into_parallel(&x, &mut poisoned);
        assert_close(
            &poisoned,
            &vec![0.0; 5],
            EPS_F32_ULTRA,
            "hollow spmv_into_parallel must zero",
        );

        log_test_footer(
            "test_singleton_and_all_empty_rows",
            Some(start.elapsed()),
            "Singleton and structurally-empty matrices produced the expected zeros.",
        );
    }

    #[test]
    fn test_feature_dim_zero_is_a_noop() {
        log_test_header(
            "test_feature_dim_zero_is_a_noop",
            "feature_dim = 0 must not panic on the Rayon chunking path.",
        );
        let start = Instant::now();

        let csr = handmade();
        let out = csr.spmm(&[], 0);
        assert!(
            out.is_empty(),
            "spmm with feature_dim 0 must produce nothing"
        );

        let mut y: Vec<f32> = Vec::new();
        csr.spmm_into_parallel(&[], 0, &mut y);

        log_test_footer(
            "test_feature_dim_zero_is_a_noop",
            Some(start.elapsed()),
            "feature_dim = 0 returned empty without dividing or chunking by zero.",
        );
    }

    #[test]
    #[should_panic(expected = "Dimension mismatch")]
    fn test_spmv_rejects_wrong_x_length() {
        let csr = handmade();
        let _ = csr.spmv(&[1.0, 2.0]); // needs 5
    }

    #[test]
    #[should_panic(expected = "Dimension mismatch")]
    fn test_spmv_into_rejects_wrong_y_length() {
        let csr = handmade();
        let x = vec![1.0f32; 5];
        let mut y = vec![0.0f32; 2]; // needs 4
        csr.spmv_into(&x, &mut y);
    }
}
