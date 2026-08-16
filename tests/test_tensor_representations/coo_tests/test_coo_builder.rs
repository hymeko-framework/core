#[cfg(test)]
mod test_coo_builder {
    use crate::test_helpers::{load_and_lower, log_test_footer, log_test_header};
    use crate::test_tensor_representations::constants::{
        DEFAULT_AGG_CFG, MINIMAL_TENSOR_VALUES_PATH,
    };
    use hymeko::tensor::representations::tensor_coo::TensorCoo;
    use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
    use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
    use hymeko_hre::expansion::{clique_expansion_coo, star_expansion_coo};
    use log::info;
    use std::time::Instant;

    struct CooBuilderCase {
        num_slices: usize,
        dim_i: usize,
        dim_j: usize,
        entries: &'static [(usize, usize, usize, f32)],
    }

    const BASIC_CASE: CooBuilderCase = CooBuilderCase {
        num_slices: 2,
        dim_i: 4,
        dim_j: 4,
        entries: &[(0, 0, 1, 1.0), (0, 2, 3, 2.5), (1, 3, 0, 0.75)],
    };

    const DUPLICATE_COORD_CASE: CooBuilderCase = CooBuilderCase {
        num_slices: 1,
        dim_i: 3,
        dim_j: 3,
        // COO should preserve duplicate coordinates (coalescing is done downstream).
        entries: &[(0, 1, 2, 1.0), (0, 1, 2, -0.5), (0, 1, 2, 0.25)],
    };

    const EMPTY_CASE: CooBuilderCase = CooBuilderCase {
        num_slices: 3,
        dim_i: 5,
        dim_j: 6,
        entries: &[],
    };

    const TEST_CASES: &[&CooBuilderCase] = &[&BASIC_CASE, &DUPLICATE_COORD_CASE, &EMPTY_CASE];

    #[test]
    fn test_tensor_coo_builder_cases() {
        log_test_header(
            "test_tensor_coo_builder_cases",
            "Build TensorCoo from fixture tuples and validate metadata/order invariants.",
        );
        let start = Instant::now();

        for (idx, case) in TEST_CASES.iter().enumerate() {
            run_case(case, idx);
        }

        log_test_footer(
            "test_tensor_coo_builder_cases",
            Some(start.elapsed()),
            "All COO builder-style fixtures matched expected metadata and entries.",
        );
    }

    #[test]
    fn test_ir_to_coo_star_transformation_case() {
        log_test_header(
            "test_ir_to_coo_star_transformation_case",
            "Build COO directly from lowered IR and validate structural invariants.",
        );
        let start = Instant::now();

        let (_store, compiled) = load_and_lower(MINIMAL_TENSOR_VALUES_PATH).unwrap();
        info!(
            "Loaded module with IR stats: decls={}, nodes={}, edges={}, arcs={}",
            compiled.ir.decl_nodes.len(),
            compiled.ir.nodes.len(),
            compiled.ir.edges.len(),
            compiled.ir.arcs.len()
        );
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(
            &compiled.ir,
            &DEFAULT_AGG_CFG,
            &ex,
        );
        info!(
            "HyperGraphView built: nodes={}, edges={}, flat_edge_nodes={}, flat_node_edges={}",
            hg.num_nodes(),
            hg.num_edges(),
            hg.flat_edge_nodes.len(),
            hg.flat_node_edges.len()
        );
        let coo = star_expansion_coo(&hg);

        let expected_dim = hg.num_nodes() + hg.num_edges();
        assert_eq!(
            coo.num_slices,
            hg.num_edges(),
            "IR->COO num_slices must equal edge count"
        );
        assert_eq!(coo.dim_i, expected_dim, "IR->COO dim_i mismatch");
        assert_eq!(coo.dim_j, expected_dim, "IR->COO dim_j mismatch");
        assert!(
            !coo.is_empty(),
            "IR->COO transformation produced no entries"
        );

        let mut per_slice = vec![0usize; coo.num_slices];
        for e in coo.iter() {
            assert!(e.k < coo.num_slices, "slice index out of range: {}", e.k);
            assert!(e.i < coo.dim_i, "row index out of range: {}", e.i);
            assert!(e.j < coo.dim_j, "col index out of range: {}", e.j);
            per_slice[e.k] += 1;
        }

        let populated = per_slice.iter().filter(|&&n| n > 0).count();
        info!(
            "Star COO slice distribution: populated_slices={}, per_slice_nnz={:?}",
            populated, per_slice
        );
        info!(
            "IR->COO star case validated (slices={}, dim {}x{}, nnz={})",
            coo.num_slices,
            coo.dim_i,
            coo.dim_j,
            coo.len()
        );
        log_test_footer(
            "test_ir_to_coo_star_transformation_case",
            Some(start.elapsed()),
            "Lowered IR transformed into a structurally consistent COO star expansion.",
        );
    }

    #[test]
    fn test_ir_to_coo_clique_transformation_case() {
        log_test_header(
            "test_ir_to_coo_clique_transformation_case",
            "Build clique COO from lowered IR and validate structural invariants.",
        );
        let start = Instant::now();

        let (_store, compiled) = load_and_lower(MINIMAL_TENSOR_VALUES_PATH).unwrap();
        info!(
            "Loaded module with IR stats: decls={}, nodes={}, edges={}, arcs={}",
            compiled.ir.decl_nodes.len(),
            compiled.ir.nodes.len(),
            compiled.ir.edges.len(),
            compiled.ir.arcs.len()
        );
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(
            &compiled.ir,
            &DEFAULT_AGG_CFG,
            &ex,
        );
        let coo = clique_expansion_coo(&hg);

        assert_eq!(
            coo.num_slices,
            hg.num_edges(),
            "IR->COO clique num_slices must equal edge count"
        );
        assert_eq!(coo.dim_i, hg.num_nodes(), "IR->COO clique dim_i mismatch");
        assert_eq!(coo.dim_j, hg.num_nodes(), "IR->COO clique dim_j mismatch");
        assert!(
            !coo.is_empty(),
            "IR->COO clique transformation produced no entries"
        );

        let mut per_slice = vec![0usize; coo.num_slices];
        for e in coo.iter() {
            assert!(e.k < coo.num_slices, "slice index out of range: {}", e.k);
            assert!(e.i < coo.dim_i, "row index out of range: {}", e.i);
            assert!(e.j < coo.dim_j, "col index out of range: {}", e.j);
            per_slice[e.k] += 1;
        }

        info!(
            "Clique COO slice distribution: populated_slices={}, per_slice_nnz={:?}",
            per_slice.iter().filter(|&&n| n > 0).count(),
            per_slice
        );
        info!(
            "IR->COO clique case validated (slices={}, dim {}x{}, nnz={})",
            coo.num_slices,
            coo.dim_i,
            coo.dim_j,
            coo.len()
        );
        log_test_footer(
            "test_ir_to_coo_clique_transformation_case",
            Some(start.elapsed()),
            "Lowered IR transformed into a structurally consistent COO clique expansion.",
        );
    }

    fn run_case(case: &CooBuilderCase, idx: usize) {
        let mut coo = TensorCoo::<f32>::with_meta(case.num_slices, case.dim_i, case.dim_j);
        coo.reserve(case.entries.len());
        info!(
            "Building COO case {} with pre-reserved capacity {}",
            idx,
            case.entries.len()
        );
        for &(k, i, j, v) in case.entries {
            coo.push(k, i, j, v);
        }

        assert_eq!(coo.num_slices, case.num_slices, "num_slices mismatch");
        assert_eq!(coo.dim_i, case.dim_i, "dim_i mismatch");
        assert_eq!(coo.dim_j, case.dim_j, "dim_j mismatch");
        assert_eq!(coo.len(), case.entries.len(), "nnz mismatch");

        for (entry_idx, &(ek, ei, ej, ev)) in case.entries.iter().enumerate() {
            let got = coo.entry(entry_idx);
            assert_eq!(got.k, ek, "entry {} k mismatch", entry_idx);
            assert_eq!(got.i, ei, "entry {} i mismatch", entry_idx);
            assert_eq!(got.j, ej, "entry {} j mismatch", entry_idx);
            assert!(
                (got.v - ev).abs() < 1e-6,
                "entry {} v mismatch: {} vs {}",
                entry_idx,
                got.v,
                ev
            );
        }

        let mut per_slice = vec![0usize; case.num_slices];
        for e in coo.iter() {
            per_slice[e.k] += 1;
        }
        info!(
            "COO case {} validated (slices={}, dim {}x{}, nnz={}, per_slice_nnz={:?})",
            idx,
            coo.num_slices,
            coo.dim_i,
            coo.dim_j,
            coo.len(),
            per_slice
        );
    }
}
