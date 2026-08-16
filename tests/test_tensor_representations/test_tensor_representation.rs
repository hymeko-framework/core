#[cfg(test)]

mod test_tensor_representation {
    use crate::test_helpers::{
        load_and_lower, log_test_footer, log_test_header, print_dense_matrix,
    };
    use crate::test_tensor_representations::constants::*;
    use hymeko::common::ids::{DeclId, EdgeId, NodeId};
    use hymeko::tensor::common::signed_incidence;
    use hymeko::tensor::tensor_val::{EdgeWScalar, EdgeWeight, ScalarWeightExtractor};
    use hymeko_hnn::tensor::common_traversal::inc_to_real;
    use hymeko_hnn::tensor::message_passing::{
        CliqueStepCfg, build_explicit_a, clique_diag, implicit_clique_step, print_dense_real,
        scatter_nodes_from_edges,
    };
    use hymeko_hnn::tensor::tensor::{compute_bipartite_degrees, dense_view_slice};
    use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
    use hymeko_hre::expansion::{
        clique_expansion_coo, star_expansion_coo, star_expansion_coo_normalized,
    };
    use log::info;
    use std::marker::PhantomData;
    use std::time::Instant;

    const STAR_EDGE_BASE: usize = STAR_NODE_COUNT;

    #[test]
    fn test_tensor_representation_creation() {
        log_test_header(
            "test_tensor_representation_creation",
            "Validates the raw star projection against expected weights and sparsity.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(LINEAR_EDGE_VALUES_PATH).unwrap();

        let aggcfg = DEFAULT_AGG_CFG;

        let ex = ScalarWeightExtractor::default();

        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &aggcfg, &ex);

        let coo = star_expansion_coo(&hg);
        let proj = hymeko_hnn::tensor::tensor::project_sum_over_slices(&coo);

        let num_nodes = hg.num_nodes(); // nálad: root + 5 => 6
        let num_edges = hg.num_edges(); // 4
        let dim = num_nodes + num_edges; // 10
        let edge_base = num_nodes; // 6

        // --- helpers ---
        let eps = EPS_F32_DEFAULT;
        let at = |r: usize, c: usize| -> f32 {
            // Ha nálad más az indexelés, itt cseréld:
            proj[r][c]
            // vagy: proj[(r, c)]
            // vagy: *proj.get(r, c).unwrap()
        };

        let assert_close = |r: usize, c: usize, expected: f32| {
            let v = at(r, c);
            assert!(
                (v - expected).abs() <= eps,
                "mismatch at ({},{}) expected {} got {}",
                r,
                c,
                expected,
                v
            );
        };

        // --- shape checks ---
        assert_eq!(
            dim, STAR_EXPECTED_DIM,
            "expected dim=10 (root + 5 nodes + 4 edges)"
        );
        assert_eq!(edge_base, STAR_EDGE_BASE, "expected edge_base=num_nodes=6");

        // --- expected nonzeros (from your printed matrix) ---
        // e1 is edge_base+0 = 6
        assert_close(2, edge_base + 0, 2.50); // node1 -> e1
        assert_close(5, edge_base + 0, 8.80); // node4 -> e1
        assert_close(edge_base + 0, 1, 1.50); // e1 -> node0

        // e2 is 7
        assert_close(edge_base + 1, 3, 0.75); // e2 -> node2
        assert_close(edge_base + 1, 4, 7.50); // e2 -> node3

        // e3 is 8
        assert_close(edge_base + 2, 1, 0.75); // e3 -> node0
        assert_close(edge_base + 2, 3, 1.95); // e3 -> node2
        assert_close(edge_base + 2, 5, 7.50); // e3 -> node4

        // e4 is 9
        assert_close(5, edge_base + 3, 7.50); // node4 -> e4
        assert_close(edge_base + 3, 1, 0.75); // e4 -> node0
        assert_close(edge_base + 3, 2, 1.95); // e4 -> node1

        // --- optional: everything else is (near) zero ---
        // (Ha ez túl szigorú lesz későbbi bővítéseknél, ezt a blokkot vedd ki.)
        let mut expected = vec![vec![0u8; dim]; dim];
        let mark = |r: usize, c: usize, expected: &mut Vec<Vec<u8>>| {
            expected[r][c] = 1;
        };

        // mark all expected nonzeros
        mark(2, edge_base + 0, &mut expected);
        mark(5, edge_base + 0, &mut expected);
        mark(edge_base + 0, 1, &mut expected);

        mark(edge_base + 1, 3, &mut expected);
        mark(edge_base + 1, 4, &mut expected);

        mark(edge_base + 2, 1, &mut expected);
        mark(edge_base + 2, 3, &mut expected);
        mark(edge_base + 2, 5, &mut expected);

        mark(5, edge_base + 3, &mut expected);
        mark(edge_base + 3, 1, &mut expected);
        mark(edge_base + 3, 2, &mut expected);

        for r in 0..dim {
            for c in 0..dim {
                if expected[r][c] == 0 {
                    let v = at(r, c);
                    assert!(v.abs() <= eps, "expected ~0 at ({},{}) but got {}", r, c, v);
                }
            }
        }

        print_dense_matrix(&proj, "Projected star matrix (sum over slices)");
        info!(
            "Star projection validated for {} nodes and {} edges",
            num_nodes, num_edges
        );
        log_test_footer(
            "test_tensor_representation_creation",
            Some(start.elapsed()),
            "All expected star entries matched and remaining cells stayed near zero.",
        );
    }

    #[test]
    fn test_star_projection_linear_edge_values() {
        log_test_header(
            "test_star_projection_linear_edge_values",
            "Recomputes the star projection and compares every non-zero entry.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(LINEAR_EDGE_VALUES_PATH).unwrap();

        let aggcfg = DEFAULT_AGG_CFG;

        let ex = ScalarWeightExtractor::default();

        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &aggcfg, &ex);
        let coo = star_expansion_coo(&hg);
        let proj = hymeko_hnn::tensor::tensor::project_sum_over_slices(&coo);

        let num_nodes = hg.num_nodes();
        let num_edges = hg.num_edges();
        let dim = num_nodes + num_edges;
        let edge_base = num_nodes;

        // Root + 5 node + 4 edge = 10
        assert_eq!(dim, STAR_EXPECTED_DIM);
        assert_eq!(edge_base, STAR_EDGE_BASE);

        let eps = EPS_F32_DEFAULT;

        // ---- adapt this accessor if your matrix type differs ----
        let at = |r: usize, c: usize| -> f32 {
            // Choose the one that matches your Dense matrix type:
            proj[r][c]
            // proj[(r, c)]
            // *proj.get(r, c).unwrap()
        };

        let assert_close = |r: usize, c: usize, expected: f32| {
            let v = at(r, c);
            assert!(
                (v - expected).abs() <= eps,
                "mismatch at ({},{}) expected {} got {}",
                r,
                c,
                expected,
                v
            );
        };

        // Mapping (based on your printed output):
        // 0=root
        // 1..5 = node0..node4
        // 6..9 = e1..e4

        // e1 (index 6): (- node0[1.5], +node1[2.5], +node4[8.8])
        assert_close(2, edge_base + 0, 2.50); // node1 -> e1
        assert_close(5, edge_base + 0, 8.80); // node4 -> e1
        assert_close(edge_base + 0, 1, 1.50); // e1 -> node0

        // e2 (index 7): (- node2[0.75], -node3[7.5])
        assert_close(edge_base + 1, 3, 0.75); // e2 -> node2
        assert_close(edge_base + 1, 4, 7.50); // e2 -> node3

        // e3 (index 8): (- node0[0.75], -node2[1.95], -node4[7.5])
        assert_close(edge_base + 2, 1, 0.75); // e3 -> node0
        assert_close(edge_base + 2, 3, 1.95); // e3 -> node2
        assert_close(edge_base + 2, 5, 7.50); // e3 -> node4

        // e4 (index 9): (- node0[0.75], -node1[1.95], +node4[7.5])
        assert_close(5, edge_base + 3, 7.50); // node4 -> e4
        assert_close(edge_base + 3, 1, 0.75); // e4 -> node0
        assert_close(edge_base + 3, 2, 1.95); // e4 -> node1

        // Optional: assert everything else is ~0 (strict, but great right now)
        let mut expected = vec![vec![false; dim]; dim];
        let mark = |r: usize, c: usize, expected: &mut Vec<Vec<bool>>| expected[r][c] = true;

        mark(2, edge_base + 0, &mut expected);
        mark(5, edge_base + 0, &mut expected);
        mark(edge_base + 0, 1, &mut expected);

        mark(edge_base + 1, 3, &mut expected);
        mark(edge_base + 1, 4, &mut expected);

        mark(edge_base + 2, 1, &mut expected);
        mark(edge_base + 2, 3, &mut expected);
        mark(edge_base + 2, 5, &mut expected);

        mark(5, edge_base + 3, &mut expected);
        mark(edge_base + 3, 1, &mut expected);
        mark(edge_base + 3, 2, &mut expected);

        for r in 0..dim {
            for c in 0..dim {
                if !expected[r][c] {
                    let v = at(r, c);
                    assert!(v.abs() <= eps, "expected ~0 at ({},{}) but got {}", r, c, v);
                }
            }
        }

        print_dense_matrix(&proj, "Projected star matrix (sum over slices)");
        info!(
            "Star projection check completed for dim {} with {} edges",
            dim, num_edges
        );
        log_test_footer(
            "test_star_projection_linear_edge_values",
            Some(start.elapsed()),
            "Star projection matched expected weights for all arcs.",
        );
    }

    #[test]
    fn test_clique_message_passing_matches_clique_view() {
        log_test_header(
            "test_clique_message_passing_matches_clique_view",
            "Cross-checks implicit clique traversal against the dense clique expansion.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(LINEAR_EDGE_VALUES_PATH).unwrap();

        let aggcfg = DEFAULT_AGG_CFG;
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &aggcfg, &ex);

        // 1) "Dense" reference: clique_view -> matrix
        let clique_coo = clique_expansion_coo(&hg);
        let a = hymeko_hnn::tensor::tensor::project_sum_over_slices(&clique_coo);

        let n = hg.num_nodes();

        // ---- adapt accessor to your matrix type ----
        let mat = |r: usize, c: usize| -> f32 {
            a[r][c]
            // a[(r, c)]
            // *a.get(r, c).unwrap()
        };

        // 2) Choose an input vector x (one-hot is great for debugging)
        // root=0, node0=1, ... as before
        let mut x = vec![0.0f32; n];
        x[2] = 1.0; // excite node1 (index 2), tweak as you like

        // y_dense = A * x
        let mut y_dense = vec![0.0f32; n];
        for i in 0..n {
            let mut acc = 0.0f32;
            for j in 0..n {
                acc += mat(i, j) * x[j];
            }
            y_dense[i] = acc;
        }

        // 3) y_imp: implicit clique step that matches clique_expansion_coo() semantics
        // clique_expansion_coo does, for each hyperedge:
        //   for each pair (u,v), a<b:
        //      if su==+ : add edge u->v with weight wu
        //      if su==- : add edge v->u with weight wu
        //      if su==0 : add both u->v and v->u with weight wu
        // where wu = flat_edge_w[p] * edge_weight[e]
        let mut y_imp = vec![0.0f32; n];

        for eid in 0..hg.num_edges() {
            let s = hg.edge_offsets[eid];
            let eend = hg.edge_offsets[eid + 1];

            // collect nodes in this hyperedge
            let mut nodes: Vec<(usize, i8, f32)> = Vec::with_capacity(eend - s);
            for p in s..eend {
                let u = hg.flat_edge_nodes[p].0;
                let su = hg.flat_edge_sign[p];
                let wu = inc_to_real(&hg, p, eid);
                nodes.push((u, su, wu));
            }

            // apply the same pair logic as clique_expansion_coo
            for a_i in 0..nodes.len() {
                for b_i in (a_i + 1)..nodes.len() {
                    let (u, su, wu) = nodes[a_i];
                    let (v, sv, wv) = nodes[b_i];
                    let w = wu * wv;

                    match (su, sv) {
                        (1, -1) => {
                            // matrix entry (u,v) = w
                            y_imp[u] += w * x[v];
                        }
                        (-1, 1) => {
                            // matrix entry (v,u) = w
                            y_imp[v] += w * x[u];
                        }
                        _ => {
                            // undirected pair: add both directions
                            y_imp[u] += w * x[v];
                            y_imp[v] += w * x[u];
                        }
                    }
                }
            }
        }
        let proj = hymeko_hnn::tensor::tensor::project_sum_over_slices(&clique_coo);
        print_dense_matrix(&proj, "Clique projection (sum over slices)");
        for (i, (dense, imp)) in y_dense.iter().zip(&y_imp).enumerate() {
            let diff = (dense - imp).abs();
            assert!(
                diff <= EPS_F32_DEFAULT,
                "implicit clique mismatch at i={} dense={} imp={} |diff|={}",
                i,
                dense,
                imp,
                diff
            );
        }
        info!(
            "Implicit clique verified for {} nodes across {} edges",
            n,
            hg.num_edges()
        );
        log_test_footer(
            "test_clique_message_passing_matches_clique_view",
            Some(start.elapsed()),
            "Implicit clique step produced the same activations as the dense reference.",
        );
    }

    #[test]
    fn test_implicit_clique_step_matches_explicit_bwb_t() {
        log_test_header(
            "test_implicit_clique_step_matches_explicit_bwb_t",
            "Confirms implicit clique propagation equals the explicit BWB^T product.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(LINEAR_EDGE_VALUES_PATH).unwrap();

        let aggcfg = DEFAULT_AGG_CFG;
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &aggcfg, &ex);

        // cfg must match your implicit_clique_step semantics
        let cfg = CliqueStepCfg {
            use_abs: true,      // recommended: stable
            include_self: true, // make comparison easier first
        };

        let n = hg.num_nodes();
        let m = hg.num_edges();
        let eps = EPS_F32_DEFAULT;

        // ---- Build explicit A = B W B^T (no diag removal here) ----
        // We'll treat incidence as:
        // b(v,e) = flat_edge_w[p] * edge_weight[e] * sign_factor
        // and then abs() if cfg.use_abs.
        let mut b: Vec<Vec<(usize, f32)>> = vec![Vec::new(); m]; // per edge: (v, bve)

        for e in 0..m {
            let (s, eend) = hg.edge_span(EdgeId::new(e));

            for p in s..eend {
                let v = hg.flat_edge_nodes[p].0;
                let mut val = inc_to_real(&hg, p, e);

                // same sign convention as in implicit_clique_step
                val *= signed_incidence::<f32>(hg.flat_edge_sign[p]);

                if cfg.use_abs {
                    val = val.abs();
                }
                b[e].push((v, val));
            }
        }

        // Explicit A in dense form (only for testing; ok for small n)
        let mut a = vec![vec![0.0f32; n]; n];
        for e in 0..m {
            let nodes = &b[e];
            for &(u, bu) in nodes {
                for &(v, bv) in nodes {
                    a[u][v] += bu * bv;
                }
            }
        }

        // ---- Pick a deterministic x ----
        let mut x = vec![0.0f32; n];
        // simple deterministic pattern (works better than one-hot)
        for i in 0..n {
            x[i] = (i as f32) * 0.1 + 1.0;
        }

        // y_dense = A * x
        let mut y_dense = vec![0.0f32; n];
        for i in 0..n {
            let mut acc = 0.0f32;
            for j in 0..n {
                acc += a[i][j] * x[j];
            }
            y_dense[i] = acc;
        }

        // --- NEW: Allocate buffers and pass precomputed optional diagonal ---
        let mut y_imp = vec![0.0f32; n];
        let mut buffer_edges = vec![0.0f32; m];
        let diag_opt = if !cfg.include_self {
            let _diag = clique_diag(&hg, cfg.use_abs);
            None
        } else {
            None
        };

        // y_imp from your implementation
        implicit_clique_step(&hg, &x, &mut y_imp, &mut buffer_edges, diag_opt, cfg);

        // Compare
        for i in 0..n {
            let d = (y_dense[i] - y_imp[i]).abs();
            assert!(
                d <= eps,
                "implicit clique mismatch at i={} dense={} imp={} |diff|={}",
                i,
                y_dense[i],
                y_imp[i],
                d
            );
        }
        let a_coo = build_explicit_a(&hg, cfg);
        let a_dense = dense_view_slice(&a_coo, 0);
        print_dense_real(
            &a_dense,
            "Explicit A = B B^T (matches implicit_clique_step)",
        );
        info!(
            "Implicit clique verified for {} nodes and {} edges using include_self={}",
            n, m, cfg.include_self
        );
        log_test_footer(
            "test_implicit_clique_step_matches_explicit_bwb_t",
            Some(start.elapsed()),
            "Implicit clique outputs matched the dense multiplication within EPS.",
        );
    }

    #[test]
    fn test_bipartite_degrees_linear_edge_values() {
        log_test_header(
            "test_bipartite_degrees_linear_edge_values",
            "Ensures node/edge degree sums line up for the linear-edge fixture.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(LINEAR_EDGE_VALUES_PATH).unwrap();

        let aggcfg = DEFAULT_AGG_CFG;
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &aggcfg, &ex);

        let (deg_v, deg_e) = compute_bipartite_degrees(&hg, true); // abs degree

        // index mapping:
        // 0=root
        // 1..5 = node0..node4
        // edges: 0..3 = e1..e4

        let eps = EPS_F32_DEFAULT;

        // node degrees (sum of abs incidence weights per node)

        assert!((deg_v[0] - 0.0).abs() <= eps); // root
        assert!((deg_v[1] - 3.0).abs() <= eps); // node0: 1.5 + 0.75 + 0.75
        assert!((deg_v[2] - 4.45).abs() <= eps); // node1: 2.5 + 1.95
        assert!((deg_v[3] - 2.7).abs() <= eps); // node2: 0.75 + 1.95
        assert!((deg_v[4] - 7.5).abs() <= eps); // node3: 7.5
        assert!((deg_v[5] - 23.8).abs() <= eps); // node4: 8.8 + 7.5 + 7.5

        // edge degrees (sum of abs incidence weights per edge)
        assert!((deg_e[0] - 12.8).abs() <= eps); // e1: 1.5+2.5+8.8
        assert!((deg_e[1] - 8.25).abs() <= eps); // e2: 0.75+7.5
        assert!((deg_e[2] - 10.2).abs() <= eps); // e3: 0.75+1.95+7.5
        assert!((deg_e[3] - 10.2).abs() <= eps); // e4: 0.75+1.95+7.5
        let sum_nodes: f32 = deg_v.iter().sum();
        let sum_edges: f32 = deg_e.iter().sum();
        info!(
            "Bipartite degrees OK: node mass = {:.2}, edge mass = {:.2}",
            sum_nodes, sum_edges
        );
        log_test_footer(
            "test_bipartite_degrees_linear_edge_values",
            Some(start.elapsed()),
            "Bipartite degrees matched expected per-node and per-edge totals.",
        );
    }

    #[test]
    fn test_star_projection_linear_edge_values_normalized() {
        log_test_header(
            "test_star_projection_linear_edge_values_normalized",
            "Verifies the normalized star projection keeps the expected sparsity pattern.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(LINEAR_EDGE_VALUES_PATH).unwrap();

        let aggcfg = DEFAULT_AGG_CFG;
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &aggcfg, &ex);

        // normalized star
        let coo = star_expansion_coo_normalized(&hg, true, STAR_NORMALIZATION_EPS);
        let proj = hymeko_hnn::tensor::tensor::project_sum_over_slices(&coo);

        let num_nodes = hg.num_nodes();
        let num_edges = hg.num_edges();
        let dim = num_nodes + num_edges;
        let edge_base = num_nodes;

        assert_eq!(dim, STAR_EXPECTED_DIM);
        assert_eq!(edge_base, STAR_EDGE_BASE);

        let eps = EPS_F32_DEFAULT;

        let at = |r: usize, c: usize| -> f32 {
            proj[r][c]
            // proj[(r, c)]
            // *proj.get(r, c).unwrap()
        };

        let assert_close = |r: usize, c: usize, expected: f32| {
            let v = at(r, c);
            assert!(
                (v - expected).abs() <= eps,
                "mismatch at ({},{}) expected {} got {}",
                r,
                c,
                expected,
                v
            );
        };

        // Same sparsity pattern as raw test, different values:

        // e1 (edge vertex = 6)
        assert_close(2, edge_base + 0, 0.33124934); // node1 -> e1
        assert_close(5, edge_base + 0, 0.50418417); // node4 -> e1
        assert_close(edge_base + 0, 1, 0.24206146); // e1 -> node0

        // e2 (7)
        assert_close(edge_base + 1, 3, 0.15891043); // e2 -> node2
        assert_close(edge_base + 1, 4, 0.95346259); // e2 -> node3

        // e3 (8)
        assert_close(edge_base + 2, 1, 0.13558154); // e3 -> node0
        assert_close(edge_base + 2, 3, 0.37158027); // e3 -> node2
        assert_close(edge_base + 2, 5, 0.48136299); // e3 -> node4

        // e4 (9)
        assert_close(5, edge_base + 3, 0.48136299); // node4 -> e4
        assert_close(edge_base + 3, 1, 0.13558154); // e4 -> node0
        assert_close(edge_base + 3, 2, 0.28943731); // e4 -> node1
        info!(
            "Normalized star projection covered dim {} with {} edges",
            dim, num_edges
        );
        log_test_footer(
            "test_star_projection_linear_edge_values_normalized",
            Some(start.elapsed()),
            "Normalized star matrix entries matched the golden weights.",
        );
    }

    #[test]
    fn test_star_projection_normalized_scale_invariant() {
        log_test_header(
            "test_star_projection_normalized_scale_invariant",
            "Checks that scaling incidences leaves the normalized projection unchanged.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(LINEAR_EDGE_VALUES_PATH).unwrap();

        let aggcfg = DEFAULT_AGG_CFG;
        let ex = ScalarWeightExtractor::default();
        let hg1 = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &aggcfg, &ex);
        let mut hg2 =
            HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &aggcfg, &ex);

        // scale all incidence weights by c
        for w in &mut hg2.flat_edge_w {
            *w *= INCIDENCE_SCALE_FACTOR;
        }

        let coo1 = star_expansion_coo_normalized(&hg1, true, STAR_NORMALIZATION_EPS);
        let proj1 = hymeko_hnn::tensor::tensor::project_sum_over_slices(&coo1);

        let coo2 = star_expansion_coo_normalized(&hg2, true, STAR_NORMALIZATION_EPS);
        let proj2 = hymeko_hnn::tensor::tensor::project_sum_over_slices(&coo2);

        let dim = hg1.num_nodes() + hg1.num_edges();
        assert_eq!(dim, hg2.num_nodes() + hg2.num_edges());

        let eps = EPS_F32_DEFAULT;

        let at1 = |r: usize, c: usize| -> f32 { proj1[r][c] };
        let at2 = |r: usize, c: usize| -> f32 { proj2[r][c] };
        let mut max_diff = 0.0f32;

        for r in 0..dim {
            for c in 0..dim {
                let d = (at1(r, c) - at2(r, c)).abs();
                max_diff = max_diff.max(d);
                assert!(
                    d <= eps,
                    "scale invariance failed at ({},{}) diff={}",
                    r,
                    c,
                    d
                );
            }
        }
        info!("Normalized star scale invariance max diff {:.3e}", max_diff);
        log_test_footer(
            "test_star_projection_normalized_scale_invariant",
            Some(start.elapsed()),
            "Normalized star projection remained stable under global scaling.",
        );
    }

    #[test]
    fn test_regression_degree_initialization_starts_at_zero() {
        log_test_header(
            "test_regression_degree_initialization_starts_at_zero",
            "Guards against accidental non-zero initialization in degree buffers.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(LINEAR_EDGE_VALUES_PATH).unwrap();

        let aggcfg = DEFAULT_AGG_CFG;
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f64, EdgeWScalar<f64>, f64>::from_ir(&compiled.ir, &aggcfg, &ex);

        let (deg_v, deg_e) = compute_bipartite_degrees(&hg, true);

        // 1. Explicit Zero-Degree Check
        // Node 0 is known to be isolated or have 0.0 degree in this specific file.
        // If the 1.0 initialization bug returns, this equals 1.0 and fails immediately.
        let eps = EPS_F64_DEFAULT;
        assert!(
            (deg_v[0] - 0.0).abs() <= eps,
            "Regression caught: Node 0 degree is {}, expected exactly 0.0. Check array initialization.",
            deg_v[0]
        );

        // 2. Structural Invariant Check
        // The total mass of node degrees must perfectly match the total mass of edge degrees.
        // Initializing with 1.0 silently breaks this if the number of nodes != number of edges.
        let sum_v: f64 = deg_v.iter().sum();
        let sum_e: f64 = deg_e.iter().sum();

        assert!(
            (sum_v - sum_e).abs() <= eps,
            "Regression caught: Bipartite degree mass mismatch. Node sum: {}, Edge sum: {}. They must be identical.",
            sum_v,
            sum_e
        );
        info!(
            "Degree-init regression check sums: nodes={:.2}, edges={:.2}",
            sum_v, sum_e
        );
        log_test_footer(
            "test_regression_degree_initialization_starts_at_zero",
            Some(start.elapsed()),
            "Degree buffers started at zero and preserved node=edge mass.",
        );
    }

    #[test]
    fn test_compute_bipartite_degrees_manual_simple() {
        log_test_header(
            "test_compute_bipartite_degrees_manual_simple",
            "Uses a hand-built graph to confirm degree calculations stay consistent.",
        );
        let start = Instant::now();
        // We manually construct a graph with 3 nodes and 2 edges.
        // Node 0: Isolated (Degree should be 0.0)
        // Node 1: Connected to Edge 0 (Weight 2.0) and Edge 1 (Weight 3.5)
        // Node 2: Connected to Edge 0 (Weight 4.0)

        // Edge 0 spans Node 1, Node 2
        // Edge 1 spans Node 1

        let hg = HyperGraphView::<f64, EdgeWScalar<f64>, f64> {
            node_decl: vec![DeclId::new(0), DeclId::new(1), DeclId::new(2)],
            edge_decl: vec![DeclId::new(3), DeclId::new(4)],

            // --- Node to Edge CSR (Not used by compute_degrees, but needed for struct) ---
            node_offsets: vec![0, 0, 2, 3], // Node 0 has 0, Node 1 has 2, Node 2 has 1
            flat_node_edges: vec![EdgeId::new(0), EdgeId::new(1), EdgeId::new(0)],
            flat_node_sign: vec![1, 1, 1],
            flat_node_w: vec![2.0, 3.5, 4.0],

            // --- Edge to Node CSR (The one compute_bipartite_degrees actually uses) ---
            edge_offsets: vec![0, 2, 3], // Edge 0 has 2 nodes, Edge 1 has 1 node
            flat_edge_nodes: vec![
                NodeId::new(1),
                NodeId::new(2), // Edge 0's nodes
                NodeId::new(1), // Edge 1's nodes
            ],
            flat_edge_sign: vec![1, 1, 1],

            // These are the incidence weights (.degree_mass() is called on these)
            flat_edge_w: vec![2.0, 4.0, 3.5],

            // Global edge weights (must implement ::one() per your from_ir logic)
            edge_weight: vec![EdgeWScalar::one(), EdgeWScalar::one()],
            _phantom: PhantomData,
        };

        let (deg_v, deg_e) = compute_bipartite_degrees(&hg, true);

        // 1. Verify Node Degrees
        assert_eq!(deg_v[0], 0.0, "Isolated node must have exactly 0.0 degree.");
        assert_eq!(
            deg_v[1],
            2.0 + 3.5,
            "Node 1 degree should be the sum of its incidence weights."
        );
        assert_eq!(
            deg_v[2], 4.0,
            "Node 2 degree should exactly match its single incidence."
        );

        // 2. Verify Edge Degrees
        assert_eq!(
            deg_e[0],
            2.0 + 4.0,
            "Edge 0 degree should be sum of Node 1 and Node 2 weights."
        );
        assert_eq!(
            deg_e[1], 3.5,
            "Edge 1 degree should exactly match Node 1's weight."
        );

        // 3. Verify Bipartite Mass Invariant
        let sum_v: f64 = deg_v.iter().sum();
        let sum_e: f64 = deg_e.iter().sum();
        assert_eq!(sum_v, sum_e, "Total node mass must equal total edge mass.");
        info!("Manual degree sums: nodes={:.1}, edges={:.1}", sum_v, sum_e);
        log_test_footer(
            "test_compute_bipartite_degrees_manual_simple",
            Some(start.elapsed()),
            "Manual bipartite degree example preserved node=edge mass.",
        );
    }

    #[test]
    fn test_regression_scatter_no_phantom_allocation() {
        log_test_header(
            "test_regression_scatter_no_phantom_allocation",
            "Ensures scatter writes into caller-provided buffers without phantom allocation.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(LINEAR_EDGE_VALUES_PATH).unwrap();

        let aggcfg = DEFAULT_AGG_CFG;
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &aggcfg, &ex);

        let n = hg.num_nodes();
        let m = hg.num_edges();
        let use_abs = true;

        // Populate input with 1.0s to ensure mathematical accumulation occurs
        let x_edges = vec![1.0f32; m];

        // 1) Fill the out-parameter with a toxic sentinel value (999.0).
        let mut y_nodes_out = vec![SCATTER_SENTINEL; n];

        // Execute the sterile scatter operation
        scatter_nodes_from_edges(&hg, &x_edges, &mut y_nodes_out, use_abs);

        let mut sum = 0.0f32;
        for i in 0..n {
            // 2) Verify the caller's buffer was actually targeted and cleared
            assert!(
                (y_nodes_out[i] - SCATTER_SENTINEL).abs() > 0.1,
                "Regression caught: scatter_nodes_from_edges ignored the caller's buffer. Sentinel values remain."
            );
            sum += y_nodes_out[i];
        }

        // 3) Verify the mathematical accumulation was written to this specific buffer
        assert!(
            sum > 0.001,
            "Regression caught: scatter_nodes_from_edges zeroed the caller's buffer but failed to write the accumulated results into it (Sum is 0.0)."
        );
        info!(
            "Scatter regression ran with {} nodes, {} edges; summed mass {:.3}",
            n, m, sum
        );
        log_test_footer(
            "test_regression_scatter_no_phantom_allocation",
            Some(start.elapsed()),
            "Scatter path mutated the caller buffer and produced positive mass.",
        );
    }
}
