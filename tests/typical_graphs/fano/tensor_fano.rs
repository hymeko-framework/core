#[cfg(test)]

mod tensor_fano {
    use crate::test_helpers::{
        load_and_lower, log_test_footer, log_test_header, print_dense_matrix,
    };
    use crate::typical_graphs::fano::constants::*;
    use hymeko::common::ids::{EdgeId, NodeId};
    use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg};
    use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
    use hymeko::tensor::util::print_dense_block;
    use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
    use hymeko_hre::expansion::star_expansion_coo;
    use log::info;
    use std::collections::{BTreeSet, HashMap, HashSet};
    use std::time::Instant;

    const AGG_CFG: AggCfg = AggCfg {
        weight: WeightAgg::Sum,
        sign: SignAgg::PreferNonNeutral,
        clamp01: false,
    };

    #[inline(always)]
    fn node_u(n: NodeId) -> usize {
        // Works whether NodeId wraps u32 or usize
        n.0
    }

    #[test]
    fn fano_invariants_hold() {
        log_test_header(
            "fano_invariants_hold",
            "Validates classical Fano incidence invariants against the tensor view.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(FANO_GRAPH_PATH).unwrap();
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &AGG_CFG, &ex);

        let n = hg.num_nodes(); // usize nálad
        let m = hg.num_edges(); // usize nálad

        assert_eq!(n, FANO_GRAPH_TOTAL_NODES, "Fano: node count mismatch");
        assert_eq!(m, FANO_EDGE_COUNT, "Fano: edge count mismatch");

        // Collect the set of nodes that actually appear in edge incidences.
        let mut incident: BTreeSet<usize> = BTreeSet::new();

        // Degree counts only for incident nodes.
        let mut deg = vec![0usize; hg.num_nodes()];

        for e in 0..hg.num_edges() {
            let (a, b) = hg.edge_span(EdgeId::new(e));
            assert_eq!(
                b - a,
                FANO_EDGE_DEGREE,
                "Each Fano edge should have degree 3"
            );

            for p in a..b {
                let u = node_u(hg.flat_edge_nodes[p]);
                incident.insert(u);
                deg[u] += 1;
            }
        }

        // The Fano plane has exactly 7 incident point nodes.
        assert_eq!(
            incident.len(),
            FANO_INCIDENT_NODE_COUNT,
            "Fano: 7 incident point nodes"
        );

        // Each incident point has degree 3.
        for &u in &incident {
            assert_eq!(
                deg[u], FANO_NODE_DEGREE,
                "Each incident node should have degree 3"
            );
        }

        // Any two distinct edges intersect in exactly 1 incident node.
        for e1 in 0..hg.num_edges() {
            for e2 in (e1 + 1)..hg.num_edges() {
                let (a1, b1) = hg.edge_span(EdgeId::new(e1));
                let (a2, b2) = hg.edge_span(EdgeId::new(e2));

                let s1 = &hg.flat_edge_nodes[a1..b1];
                let s2 = &hg.flat_edge_nodes[a2..b2];

                let mut inter = 0usize;
                for &x in s1 {
                    if s2.contains(&x) {
                        inter += 1;
                    }
                }
                assert_eq!(
                    inter, 1,
                    "Edges {e1} and {e2} should intersect in exactly 1 node"
                );
            }
        }
        info!(
            "Fano invariants: {} nodes, {} edges, {} incidences",
            hg.num_nodes(),
            hg.num_edges(),
            hg.flat_edge_nodes.len()
        );
        log_test_footer(
            "fano_invariants_hold",
            Some(start.elapsed()),
            "All structural invariants (degrees, intersections) satisfied.",
        );
    }

    #[test]
    fn star_tensor_respects_bretto_directions() {
        log_test_header(
            "star_tensor_respects_bretto_directions",
            "Checks star tensor directions vs. Bretto sign policy.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(FANO_GRAPH_PATH).unwrap();
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &AGG_CFG, &ex);

        let coo = star_expansion_coo(&hg);

        let num_nodes = hg.num_nodes();
        let edge_base = num_nodes;

        // For each incidence (edge e, node u, sign s),
        // check that tensor has the expected directed entry/entries.
        // '+' : node -> edge, '-' : edge -> node, '0' : both
        for e in 0..hg.num_edges() {
            let (a, b) = hg.edge_span(EdgeId::new(e));
            let e_v = edge_base + e;

            for p in a..b {
                let u = hg.flat_edge_nodes[p].0;
                let s = hg.flat_edge_sign[p];

                let mut has_ne = false; // node -> edge
                let mut has_en = false; // edge -> node

                // Scan COO entries for this slice (small in Fano, fine for test).
                for idx in 0..coo.len() {
                    let entry = coo.entry(idx);
                    if entry.k != e {
                        continue;
                    }
                    let i = entry.i;
                    let j = entry.j;

                    if i == u && j == e_v {
                        has_ne = true;
                    }
                    if i == e_v && j == u {
                        has_en = true;
                    }
                }

                match s {
                    1 => assert!(has_ne && !has_en, "Expected '+' to be node->edge only"),
                    -1 => assert!(has_en && !has_ne, "Expected '-' to be edge->node only"),
                    _ => assert!(
                        has_ne && has_en,
                        "Expected neutral to include both directions"
                    ),
                }
            }
        }
        log_test_footer(
            "star_tensor_respects_bretto_directions",
            Some(start.elapsed()),
            "Each incidence generated the expected directed entries in the star tensor.",
        );
    }

    #[test]
    fn fano_star_tensor_nnz_matches_sign_policy() {
        log_test_header(
            "fano_star_tensor_nnz_matches_sign_policy",
            "Counts non-zeros in the star tensor to match neutral/directed policy.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(FANO_GRAPH_PATH).unwrap();
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &AGG_CFG, &ex);

        // Itt két opció van:
        // A) ha free function: star_expansion_coo(&hg)
        // B) ha method: hg.tensor_star_coo()
        //
        // Én free function-t javaslok, mert tisztább rétegzés.
        let coo = star_expansion_coo(&hg);

        // nnz = directed incidences:
        // neutral (0) -> 2 bejegyzés, plus/minus -> 1 bejegyzés
        let neutral = hg.flat_edge_sign.iter().filter(|&&s| s == 0).count();
        let directed = hg.flat_edge_sign.len() - neutral;
        let expected_nnz = neutral * 2 + directed;

        assert_eq!(coo.len(), expected_nnz);

        // dim: V* = V ∪ E
        assert_eq!(coo.num_slices, hg.num_edges());
        assert_eq!(coo.dim_i, hg.num_nodes() + hg.num_edges());
        assert_eq!(coo.dim_j, hg.num_nodes() + hg.num_edges());
        info!(
            "Star tensor NNZ check: {} directed, {} neutral -> {} entries",
            directed,
            neutral,
            coo.len()
        );
        log_test_footer(
            "fano_star_tensor_nnz_matches_sign_policy",
            Some(start.elapsed()),
            "Star tensor dimensions and nnz matched the sign policy counts.",
        );
    }

    #[test]
    fn fano_star_tensor_has_correct_matrix_entries() {
        log_test_header(
            "fano_star_tensor_has_correct_matrix_entries",
            "Confirms per-slice entries align with incidence signs and values.",
        );
        let start = Instant::now();

        let (_store, compiled) = load_and_lower(FANO_GRAPH_PATH).unwrap();
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &AGG_CFG, &ex);

        let coo = star_expansion_coo(&hg);

        let num_nodes = hg.num_nodes();
        let num_edges = hg.num_edges();
        let dim = num_nodes + num_edges;
        let edge_base = num_nodes;

        // Build per-slice adjacency maps: (i,j) -> summed value.
        // This also makes the test robust to accidental duplicate pushes.
        let mut slice_maps: Vec<HashMap<(usize, usize), f32>> =
            (0..num_edges).map(|_| HashMap::new()).collect();

        // 1) Range checks + build maps
        for e in coo.iter() {
            let k = e.k;
            let i = e.i;
            let j = e.j;
            let v = e.v;

            assert!(k < num_edges, "slice index k out of range");
            assert!(i < dim, "row index i out of range");
            assert!(j < dim, "col index j out of range");

            *slice_maps[k].entry((i, j)).or_insert(0.0) += v;
        }

        // 2) For each incidence in HyperGraphView, verify expected matrix entry/entries exist
        // and have value 1.0 (sum = 1.0).
        for e in 0..num_edges {
            let (a, b) = hg.edge_span(EdgeId::new(e));
            let e_v = edge_base + e;

            // Each edge in the Fano file should be incident to 3 nodes.
            assert_eq!(b - a, 3);

            for p in a..b {
                let u = hg.flat_edge_nodes[p].0; // node vertex index in V*
                let s = hg.flat_edge_sign[p]; // +1 / -1 / 0

                let m = &slice_maps[e];

                let ne = m.get(&(u, e_v)).copied().unwrap_or(0.0); // node -> edge
                let en = m.get(&(e_v, u)).copied().unwrap_or(0.0); // edge -> node

                match s {
                    1 => {
                        assert!(
                            (ne - 1.0).abs() <= FANO_TOLERANCE,
                            "Expected '+' incidence to create (node,edge)=1.0"
                        );
                        assert!(
                            en.abs() <= FANO_TOLERANCE,
                            "Expected '+' incidence NOT to create (edge,node)"
                        );
                    }
                    -1 => {
                        assert!(
                            (en - 1.0).abs() <= FANO_TOLERANCE,
                            "Expected '-' incidence to create (edge,node)=1.0"
                        );
                        assert!(
                            ne.abs() <= FANO_TOLERANCE,
                            "Expected '-' incidence NOT to create (node,edge)"
                        );
                    }
                    _ => {
                        assert!(
                            (ne - 1.0).abs() <= FANO_TOLERANCE,
                            "Expected neutral incidence to create (node,edge)=1.0"
                        );
                        assert!(
                            (en - 1.0).abs() <= FANO_TOLERANCE,
                            "Expected neutral incidence to create (edge,node)=1.0"
                        );
                    }
                }
            }

            // 3) No extra entries: everything present in the slice must correspond
            // to an incidence in this edge (in one of the allowed directions).
            //
            // Build a set of allowed pairs for this edge slice.
            let mut allowed = HashSet::<(usize, usize)>::new();
            for p in a..b {
                let u = hg.flat_edge_nodes[p].0;
                let s = hg.flat_edge_sign[p];
                match s {
                    1 => {
                        allowed.insert((u, e_v));
                    }
                    -1 => {
                        allowed.insert((e_v, u));
                    }
                    _ => {
                        allowed.insert((u, e_v));
                        allowed.insert((e_v, u));
                    }
                }
            }

            for (&(i, j), &val) in slice_maps[e].iter() {
                assert!(
                    allowed.contains(&(i, j)),
                    "Unexpected matrix entry in slice {e}: ({i},{j}) = {val}"
                );
                // Optional: assert it's exactly 1.0 (or 2.0 if duplicates happen).
                assert!(
                    (val - 1.0).abs() <= FANO_TOLERANCE,
                    "Unexpected value in slice {e}: ({i},{j}) = {val}"
                );
            }
        }
        log_test_footer(
            "fano_star_tensor_has_correct_matrix_entries",
            Some(start.elapsed()),
            "All slice entries matched the expected sign-constrained adjacency.",
        );
    }

    #[test]
    fn debug_fano_star_dense_view() {
        log_test_header(
            "debug_fano_star_dense_view",
            "Materializes one dense slice for human inspection.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(FANO_GRAPH_PATH).unwrap();
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &AGG_CFG, &ex);

        let coo = star_expansion_coo(&hg);
        assert!(
            coo.len() > 0,
            "Star tensor should contain entries for Fano graph"
        );

        // Print slice 0 as a sanity check (14x14 would still be OK, but let's show all).
        print_dense_block(&coo, 0, 0, 0, coo.dim_i, coo.dim_j);
        log_test_footer(
            "debug_fano_star_dense_view",
            Some(start.elapsed()),
            "Slice 0 dense block dumped for debugging.",
        );
    }

    #[test]
    fn projected_star_matches_view_incidence() {
        log_test_header(
            "projected_star_matches_view_incidence",
            "Projects the star tensor and matches it against raw incidence lists.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(FANO_GRAPH_PATH).unwrap();
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &AGG_CFG, &ex);

        let coo = star_expansion_coo(&hg);
        let proj = hymeko_hnn::tensor::tensor::project_sum_over_slices(&coo);

        let num_nodes = hg.num_nodes();
        let num_edges = hg.num_edges();
        let dim = num_nodes + num_edges;
        let edge_base = num_nodes;

        // Build expected projection directly from HyperGraphView incidence lists.
        let mut expected = vec![vec![0.0f32; dim]; dim];

        for e in 0..num_edges {
            let (a, b) = hg.edge_span(EdgeId::new(e));
            let e_v = edge_base + e;

            for p in a..b {
                let u = hg.flat_edge_nodes[p].0;
                let s = hg.flat_edge_sign[p];
                let w: f32 = 1.0;

                match s {
                    1 => {
                        // '+' : node -> edge
                        expected[u][e_v] += w;
                    }
                    -1 => {
                        // '-' : edge -> node
                        expected[e_v][u] += w;
                    }
                    _ => {
                        // neutral: both
                        expected[u][e_v] += w;
                        expected[e_v][u] += w;
                    }
                }
            }
        }

        // Compare.
        for i in 0..dim {
            for j in 0..dim {
                let a = proj[i][j];
                let b = expected[i][j];
                assert!(
                    (a - b).abs() <= FANO_TOLERANCE,
                    "Mismatch at ({i},{j}): got {a}, expected {b}"
                );
            }
        }
        print_dense_matrix(&proj, "Projected star matrix (sum over slices)");
        log_test_footer(
            "projected_star_matches_view_incidence",
            Some(start.elapsed()),
            "Projected star matrix matched directly accumulated incidence weights.",
        );
    }
}
