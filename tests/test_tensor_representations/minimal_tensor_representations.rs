#[cfg(test)]
mod minimal_tensor_representations {
    use crate::test_helpers::{load_and_lower, log_test_footer, log_test_header};
    use crate::test_tensor_representations::constants::*;
    use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
    use hymeko::tensor::util::print_dense_block;
    use hymeko_hnn::tensor::tensor::{compute_bipartite_degrees, project_sum_over_slices};
    use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
    use hymeko_hre::expansion::{
        clique_expansion_coo, star_expansion_coo, star_expansion_coo_normalized,
    };
    use log::info;
    use std::time::Instant;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn tiny_star_2nodes_1edge_directions_and_values() {
        log_test_header(
            "tiny_star_2nodes_1edge_directions_and_values",
            "Validates raw star incidences for the two-node tiny graph.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(MINIMAL_TENSOR_VALUES_PATH).unwrap();

        let aggcfg = DEFAULT_AGG_CFG;
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &aggcfg, &ex);

        let coo = star_expansion_coo(&hg);
        let a = project_sum_over_slices(&coo);

        let n = hg.num_nodes();
        let m = hg.num_edges();
        assert_eq!(m, 1, "expected exactly 1 edge in this tiny graph");

        let e_idx = n + 0;
        let eps = EPS_F32_STRICT;

        // Keressük meg a két várt nemnullát úgy, hogy ne kelljen tudni a node indexeket.
        // Várt: A[e_idx, v_minus] = 0.85 és A[v_plus, e_idx] = 0.9
        let mut found_minus: Option<(usize, f32)> = None; // (v, val)
        let mut found_plus: Option<(usize, f32)> = None; // (v, val)

        for v in 0..n {
            let em = a[e_idx][v]; // edge -> node
            let pe = a[v][e_idx]; // node -> edge

            if em.abs() > eps {
                found_minus = Some((v, em));
            }
            if pe.abs() > eps {
                found_plus = Some((v, pe));
            }
        }

        let (v_minus, val_minus) =
            found_minus.expect("expected one edge->node nonzero (minus incidence)");
        let (v_plus, val_plus) =
            found_plus.expect("expected one node->edge nonzero (plus incidence)");

        assert!(
            approx_eq(val_minus, 0.85, EPS_F32_DEFAULT),
            "edge->node value should be 0.85, got {}",
            val_minus
        );
        assert!(
            approx_eq(val_plus, 0.9, EPS_F32_DEFAULT),
            "node->edge value should be 0.9, got {}",
            val_plus
        );

        // Biztosítsuk, hogy nincs véletlen extra nemnulla a node-edge blokkon kívül
        // (szigorú, de tiny példán jó)
        for i in 0..(n + m) {
            for j in 0..(n + m) {
                let v = a[i][j];
                if v.abs() <= eps {
                    continue;
                }

                let ok = (i == e_idx && j == v_minus) || (i == v_plus && j == e_idx);

                assert!(ok, "unexpected nonzero at ({},{}) = {}", i, j, v);
            }
        }
        let dim = hg.num_nodes() + hg.num_edges();
        print_dense_block::<f32>(&coo, 0, 0, 0, dim, dim);
        info!(
            "Tiny star located minus node {} -> 0.85 and plus node {} -> 0.9",
            v_minus, v_plus
        );
        log_test_footer(
            "tiny_star_2nodes_1edge_directions_and_values",
            Some(start.elapsed()),
            "Star incidences produced the expected directional weights (0.85/0.9).",
        );
    }

    #[test]
    fn tiny_star_2nodes_1edge_normalized_matches_formula() {
        log_test_header(
            "tiny_star_2nodes_1edge_normalized_matches_formula",
            "Checks normalized star entries against the manual degree formula.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(MINIMAL_TENSOR_VALUES_PATH).unwrap();

        let aggcfg = DEFAULT_AGG_CFG;
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &aggcfg, &ex);

        let (deg_v, deg_e) = compute_bipartite_degrees(&hg, true);

        let coo = star_expansion_coo_normalized(&hg, true, STAR_NORMALIZATION_EPS);
        let a = project_sum_over_slices(&coo);

        let n = hg.num_nodes();
        let e_idx = n + 0;

        // megtaláljuk ugyanúgy a két nemnullát
        let eps = EPS_F32_STRICT;
        let mut v_minus = None;
        let mut v_plus = None;

        for v in 0..n {
            if a[e_idx][v].abs() > eps {
                v_minus = Some(v);
            }
            if a[v][e_idx].abs() > eps {
                v_plus = Some(v);
            }
        }
        let v_minus = v_minus.unwrap();
        let v_plus = v_plus.unwrap();

        // elvárt normalizált értékek
        let de = deg_e[0].max(STAR_NORMALIZATION_EPS);
        let expected_minus = 0.85 / (deg_v[v_minus] * de).sqrt();
        let expected_plus = 0.9 / (deg_v[v_plus] * de).sqrt();

        assert!((a[e_idx][v_minus] - expected_minus).abs() < EPS_F32_DEFAULT);
        assert!((a[v_plus][e_idx] - expected_plus).abs() < EPS_F32_DEFAULT);
        let dim = hg.num_nodes() + hg.num_edges();
        print_dense_block::<f32>(&coo, 0, 0, 0, dim, dim);
        info!(
            "Normalized star minus node {} plus node {} matched analytic scaling",
            v_minus, v_plus
        );
        log_test_footer(
            "tiny_star_2nodes_1edge_normalized_matches_formula",
            Some(start.elapsed()),
            "Normalized entries equaled the computed (deg_v, deg_e) scaling.",
        );
    }

    #[test]
    fn tiny_clique_2nodes_1edge_expected_direction_and_value() {
        log_test_header(
            "tiny_clique_2nodes_1edge_expected_direction_and_value",
            "Checks clique projection directionality for the 2-node toy graph.",
        );
        let start = Instant::now();
        let (_store, compiled) = load_and_lower(MINIMAL_TENSOR_VALUES_PATH).unwrap();

        let aggcfg = DEFAULT_AGG_CFG;
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &aggcfg, &ex);

        let clique = clique_expansion_coo(&hg);
        assert_eq!(
            clique.num_slices,
            hg.num_edges(),
            "clique COO slices should match num_edges"
        );

        let n = hg.num_nodes();
        assert_eq!(n, 3, "this tiny graph should have exactly 2 nodes");

        // Debug print (run with -- --nocapture)
        print_dense_block(&clique, 0, 0, 0, n, n);

        let a = project_sum_over_slices(&clique);
        let eps = EPS_F32_STRICT;

        // Find the single nonzero off-diagonal (robust to node index order)
        // Expected: exactly one offdiag ~ 1.53, the other offdiag ~ 0, diag ~0
        assert!(
            (a[0][0]).abs() <= eps,
            "diag(0,0) should be 0, got {}",
            a[0][0]
        );
        assert!(
            (a[1][1]).abs() <= eps,
            "diag(1,1) should be 0, got {}",
            a[1][1]
        );

        for i in 0..n {
            assert!(a[0][i].abs() < eps);
            assert!(a[i][0].abs() < eps);
        }

        // count nonzeros off-diagonal (excluding root)
        let mut nz = vec![];
        for i in 1..n {
            for j in 1..n {
                if i == j {
                    continue;
                }
                if a[i][j].abs() > eps {
                    nz.push((i, j, a[i][j]));
                }
            }
        }
        assert_eq!(
            nz.len(),
            1,
            "There should be exactly one directed off-diagonal entry"
        );
        assert!(
            (nz[0].2 - 0.765).abs() < EPS_F32_DEFAULT,
            "Expected mathematically correct single-counted weight 0.765, got {}",
            nz[0].2
        );
        info!(
            "Clique non-zero registered at ({}, {}) with value {:.3}",
            nz[0].0, nz[0].1, nz[0].2
        );
        log_test_footer(
            "tiny_clique_2nodes_1edge_expected_direction_and_value",
            Some(start.elapsed()),
            "Clique projection kept exactly one directed off-diagonal entry (0.765).",
        );
    }
}
