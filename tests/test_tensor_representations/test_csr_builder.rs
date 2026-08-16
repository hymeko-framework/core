#[cfg(test)]
mod test_csr_builder {
    use crate::test_helpers::load_and_lower;
    use crate::test_helpers::{log_test_footer, log_test_header};
    use crate::test_tensor_representations::constants::{
        DEFAULT_AGG_CFG, MINIMAL_TENSOR_VALUES_PATH,
    };
    use hymeko::tensor::representations::tensor_csr::TensorCsrBuilder;
    use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
    use hymeko_hnn::tensor::representations::tensor_csr_representations::star_expansion_csr;
    use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
    use log::info;
    use std::time::Instant;

    struct CsrBuilderCase {
        dim_i: usize,
        dim_j: usize,
        rows: &'static [usize],
        cols: &'static [usize],
        vals: &'static [f64],
        expected_row_ptr: &'static [usize],
        expected_col_ind: &'static [usize],
        expected_vals: &'static [f64],
    }

    const COALESCED_DUP_CASE: CsrBuilderCase = CsrBuilderCase {
        dim_i: 3,
        dim_j: 4,
        rows: &[0, 3, 3, 7],
        cols: &[2, 0, 2, 3, 1, 1, 3],
        vals: &[1.0, 2.0, 1.5, 0.5, 1.0, 1.0, 0.5],
        expected_row_ptr: &[0, 2, 2, 4],
        expected_col_ind: &[0, 2, 1, 3],
        expected_vals: &[2.0, 2.5, 2.0, 1.0],
    };

    const SIMPLE_CHAIN_CASE: CsrBuilderCase = CsrBuilderCase {
        dim_i: 2,
        dim_j: 2,
        rows: &[0, 1, 2],
        cols: &[0, 1],
        vals: &[5.0, 7.0],
        expected_row_ptr: &[0, 1, 2],
        expected_col_ind: &[0, 1],
        expected_vals: &[5.0, 7.0],
    };

    const EMPTY_INPUT_CASE: CsrBuilderCase = CsrBuilderCase {
        dim_i: 4,
        dim_j: 5,
        rows: &[0, 0, 0, 0, 0],
        cols: &[],
        vals: &[],
        expected_row_ptr: &[0, 0, 0, 0, 0],
        expected_col_ind: &[],
        expected_vals: &[],
    };

    const GAPPED_ROWS_CASE: CsrBuilderCase = CsrBuilderCase {
        dim_i: 5,
        dim_j: 6,
        // row0: 2 entries, row1: empty, row2: 3 entries, row3: empty, row4: 1 entry
        rows: &[0, 2, 2, 5, 5, 6],
        cols: &[5, 1, 4, 4, 0, 2],
        vals: &[1.0, 3.0, 2.0, 1.0, 7.0, 9.0],
        expected_row_ptr: &[0, 2, 2, 4, 4, 5],
        expected_col_ind: &[1, 5, 0, 4, 2],
        expected_vals: &[3.0, 1.0, 7.0, 3.0, 9.0],
    };

    const DUP_CANCEL_CASE: CsrBuilderCase = CsrBuilderCase {
        dim_i: 2,
        dim_j: 3,
        // row0 has duplicates on col1 that cancel to zero; row1 has stable unique entries
        rows: &[0, 3, 5],
        cols: &[1, 1, 2, 0, 2],
        vals: &[2.5, -2.5, 4.0, 1.0, 3.0],
        expected_row_ptr: &[0, 2, 4],
        expected_col_ind: &[1, 2, 0, 2],
        expected_vals: &[0.0, 4.0, 1.0, 3.0],
    };

    const TEST_CASES: &[&CsrBuilderCase] = &[
        &COALESCED_DUP_CASE,
        &SIMPLE_CHAIN_CASE,
        &EMPTY_INPUT_CASE,
        &GAPPED_ROWS_CASE,
        &DUP_CANCEL_CASE,
    ];

    #[test]
    fn test_tensor_csr_builder_finalize_coalesced() {
        log_test_header(
            "test_tensor_csr_builder_finalize_coalesced",
            "Coalesces COO data into CSR form across multiple fixtures.",
        );
        let start = Instant::now();
        for (idx, case) in TEST_CASES.iter().enumerate() {
            run_case(case, idx);
        }
        log_test_footer(
            "test_tensor_csr_builder_finalize_coalesced",
            Some(start.elapsed()),
            "All CSR builder cases produced the expected row/col/value buffers.",
        );
    }

    #[test]
    fn test_ir_to_csr_star_transformation_case() {
        log_test_header(
            "test_ir_to_csr_star_transformation_case",
            "Build CSR directly from lowered IR and validate structural invariants.",
        );
        let start = Instant::now();

        let (_store, compiled) = load_and_lower(MINIMAL_TENSOR_VALUES_PATH).unwrap();
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(
            &compiled.ir,
            &DEFAULT_AGG_CFG,
            &ex,
        );
        let csr = star_expansion_csr(&hg);

        let expected_dim = hg.num_nodes() + hg.num_edges();
        assert_eq!(csr.num_rows, expected_dim, "IR->CSR row dimension mismatch");
        assert_eq!(csr.num_cols, expected_dim, "IR->CSR col dimension mismatch");
        assert_eq!(
            csr.row_ptr.len(),
            expected_dim + 1,
            "row_ptr length must be dim + 1"
        );
        assert_eq!(
            csr.row_ptr.first().copied(),
            Some(0),
            "row_ptr must start at 0"
        );
        assert_eq!(
            csr.row_ptr.last().copied(),
            Some(csr.val.len()),
            "row_ptr end must equal nnz"
        );
        assert!(
            !csr.val.is_empty(),
            "IR->CSR transformation produced empty star expansion"
        );

        info!(
            "IR->CSR star case validated (dim {}x{}, nnz={})",
            csr.num_rows,
            csr.num_cols,
            csr.val.len()
        );
        log_test_footer(
            "test_ir_to_csr_star_transformation_case",
            Some(start.elapsed()),
            "Lowered IR transformed into a structurally consistent CSR star expansion.",
        );
    }

    fn run_case(case: &CsrBuilderCase, idx: usize) {
        let builder = TensorCsrBuilder::<f64> {
            dim_i: case.dim_i,
            dim_j: case.dim_j,
            uncoalesced_row_ptr: case.rows.to_vec(),
            cols: case.cols.to_vec(),
            vals: case.vals.to_vec(),
        };
        let csr = builder.finalize_coalesced();

        assert_eq!(
            csr.num_rows, case.dim_i,
            "Expected {} rows, got {}",
            case.dim_i, csr.num_rows
        );
        assert_eq!(
            csr.num_cols, case.dim_j,
            "Expected {} columns, got {}",
            case.dim_j, csr.num_cols
        );
        assert_eq!(
            csr.row_ptr, case.expected_row_ptr,
            "Unexpected row_ptr: {:?}",
            csr.row_ptr
        );
        assert_eq!(
            csr.col_ind, case.expected_col_ind,
            "Unexpected col_ind: {:?}",
            csr.col_ind
        );
        assert_eq!(csr.val, case.expected_vals, "Unexpected val: {:?}", csr.val);

        // Structural safety checks shared by all fixtures.
        assert_eq!(csr.row_ptr.len(), case.dim_i + 1, "row_ptr length mismatch");
        assert_eq!(
            csr.row_ptr.first().copied(),
            Some(0),
            "row_ptr must start at 0"
        );
        assert_eq!(
            csr.row_ptr.last().copied(),
            Some(csr.val.len()),
            "row_ptr end must equal nnz"
        );

        info!(
            "CSR case {} validated (dim {}x{}, nnz={})",
            idx,
            csr.num_rows,
            csr.num_cols,
            csr.val.len()
        );
    }
}
