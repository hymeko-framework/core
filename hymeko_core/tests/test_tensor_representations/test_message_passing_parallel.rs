//! Regression tests for the parallelised hypergraph message-passing kernels.
//!
//! The 2026-08-24 optimisation rewrote `scatter_nodes_from_edges` and
//! `clique_diag` from an **edge-major** traversal (read-modify-write into a
//! shared `y[v]`) to a **node-major** one over the node-first incidence CSR,
//! which is what makes them parallelisable without atomics.
//!
//! The claim justifying that rewrite is stronger than "within tolerance": both
//! incidence CSRs hold the same multiset, the node-first one ordered by
//! `(node, edge)`, so each node still accumulates its contributions in
//! increasing edge order — the exact sequence the old edge-major loop
//! delivered. The result must therefore be **bitwise** identical.
//!
//! `reference_scatter_edge_major` and `reference_clique_diag_edge_major` below
//! are verbatim reimplementations of the pre-rewrite algorithm. Keeping a copy
//! in the test tree is intentional: it is the only thing that can fail if a
//! future change silently reassociates the sums, and a test that merely
//! compared the new code to itself would not. These are reference oracles, not
//! duplicated production logic.

#[cfg(test)]
mod test_message_passing_parallel {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::tensor::tensor_val::EdgeWScalar;
    use hymeko_hnn::tensor::common_traversal::inc_scalar_signed;
    use hymeko_hnn::tensor::message_passing::{
        CliqueStepCfg, clique_diag, clique_diag_into, gather_edges_from_nodes,
        gather_edges_from_nodes_parallel, gather_edges_from_nodes_serial, implicit_clique_step,
        remove_self_effect, scatter_nodes_from_edges, scatter_nodes_from_edges_parallel,
        scatter_nodes_from_edges_serial,
    };
    use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
    use hymeko_hnn::workloads::{DegreeShape, HyperSpec, SignPattern};
    use log::info;
    use std::time::Instant;

    type View = HyperGraphView<f32, EdgeWScalar<f32>, f32>;

    const POISON: f32 = -999.5;

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

    /// The scales the kernels are exercised at. `small` sits below the parallel
    /// dispatch threshold and `medium` above it, so the dispatcher is observed
    /// taking both branches rather than assumed to.
    fn specs() -> Vec<(&'static str, HyperSpec)> {
        vec![
            ("small", HyperSpec::small()),
            ("medium", HyperSpec::medium()),
            (
                "irregular",
                HyperSpec {
                    shape: DegreeShape::Irregular,
                    ..HyperSpec::medium()
                },
            ),
            (
                "all_plus",
                HyperSpec {
                    signs: SignPattern::AllPlus,
                    ..HyperSpec::small()
                },
            ),
        ]
    }

    // -----------------------------------------------------------------
    // reference oracles: the pre-rewrite, edge-major algorithms
    // -----------------------------------------------------------------

    fn reference_scatter_edge_major(hg: &View, x_edges: &[f32], use_abs: bool) -> Vec<f32> {
        let mut y = vec![0.0f32; hg.num_nodes()];
        for e in 0..hg.num_edges() {
            let s = hg.edge_offsets[e];
            let eend = hg.edge_offsets[e + 1];
            let xe = x_edges[e];
            for p in s..eend {
                let v = hg.flat_edge_nodes[p].0;
                let b = inc_scalar_signed(hg, p, e, use_abs);
                y[v] += b * xe;
            }
        }
        y
    }

    fn reference_clique_diag_edge_major(hg: &View, use_abs: bool) -> Vec<f32> {
        let mut diag = vec![0.0f32; hg.num_nodes()];
        for e in 0..hg.num_edges() {
            let s = hg.edge_offsets[e];
            let eend = hg.edge_offsets[e + 1];
            for p in s..eend {
                let v = hg.flat_edge_nodes[p].0;
                let b = inc_scalar_signed(hg, p, e, use_abs);
                diag[v] += b * b;
            }
        }
        diag
    }

    fn reference_gather(hg: &View, x_nodes: &[f32], use_abs: bool) -> Vec<f32> {
        let mut out = vec![0.0f32; hg.num_edges()];
        for e in 0..hg.num_edges() {
            let s = hg.edge_offsets[e];
            let eend = hg.edge_offsets[e + 1];
            let mut acc = 0.0f32;
            for p in s..eend {
                let v = hg.flat_edge_nodes[p].0;
                acc += inc_scalar_signed(hg, p, e, use_abs) * x_nodes[v];
            }
            out[e] = acc;
        }
        out
    }

    fn edge_vector(m: usize) -> Vec<f32> {
        (0..m).map(|e| 0.5 + (e % 23) as f32 * 0.125).collect()
    }

    // -----------------------------------------------------------------
    // gather
    // -----------------------------------------------------------------

    #[test]
    fn test_gather_serial_parallel_and_reference_agree() {
        log_test_header(
            "test_gather_serial_parallel_and_reference_agree",
            "gather: serial, parallel, dispatcher and reference must be bit-identical.",
        );
        let start = Instant::now();

        for (label, spec) in specs() {
            let hg = spec.build();
            let x = spec.node_vector();
            let expected = reference_gather(&hg, &x, true);

            let mut serial = vec![POISON; hg.num_edges()];
            let mut parallel = vec![POISON; hg.num_edges()];
            let mut dispatched = vec![POISON; hg.num_edges()];
            gather_edges_from_nodes_serial(&hg, &x, &mut serial, true);
            gather_edges_from_nodes_parallel(&hg, &x, &mut parallel, true);
            gather_edges_from_nodes(&hg, &x, &mut dispatched, true);

            assert_bitwise(
                &serial,
                &expected,
                &format!("gather serial vs reference [{label}]"),
            );
            assert_bitwise(
                &parallel,
                &expected,
                &format!("gather parallel vs reference [{label}]"),
            );
            assert_bitwise(
                &dispatched,
                &expected,
                &format!("gather dispatch vs reference [{label}]"),
            );

            info!(
                "[{label}] gather agrees across 4 paths ({} edges)",
                hg.num_edges()
            );
        }

        log_test_footer(
            "test_gather_serial_parallel_and_reference_agree",
            Some(start.elapsed()),
            "Gather is bit-identical across serial, parallel, dispatched and reference paths.",
        );
    }

    // -----------------------------------------------------------------
    // scatter — the node-first rewrite
    // -----------------------------------------------------------------

    #[test]
    fn test_scatter_node_major_matches_edge_major_bitwise() {
        log_test_header(
            "test_scatter_node_major_matches_edge_major_bitwise",
            "The node-first scatter rewrite must reproduce the edge-major result bit-for-bit.",
        );
        let start = Instant::now();

        for (label, spec) in specs() {
            let hg = spec.build();
            let x_edges = edge_vector(hg.num_edges());
            let expected = reference_scatter_edge_major(&hg, &x_edges, true);

            let mut serial = vec![POISON; hg.num_nodes()];
            let mut parallel = vec![POISON; hg.num_nodes()];
            let mut dispatched = vec![POISON; hg.num_nodes()];
            scatter_nodes_from_edges_serial(&hg, &x_edges, &mut serial, true);
            scatter_nodes_from_edges_parallel(&hg, &x_edges, &mut parallel, true);
            scatter_nodes_from_edges(&hg, &x_edges, &mut dispatched, true);

            assert_bitwise(
                &serial,
                &expected,
                &format!("scatter serial vs edge-major [{label}]"),
            );
            assert_bitwise(
                &parallel,
                &expected,
                &format!("scatter parallel vs edge-major [{label}]"),
            );
            assert_bitwise(
                &dispatched,
                &expected,
                &format!("scatter dispatch vs edge-major [{label}]"),
            );

            info!(
                "[{label}] scatter node-first == edge-major, bitwise ({} nodes)",
                hg.num_nodes()
            );
        }

        log_test_footer(
            "test_scatter_node_major_matches_edge_major_bitwise",
            Some(start.elapsed()),
            "The node-first scatter reassociates nothing: bitwise equal to the original edge-major loop.",
        );
    }

    #[test]
    fn test_scatter_without_abs_preserves_signs() {
        log_test_header(
            "test_scatter_without_abs_preserves_signs",
            "use_abs = false must keep negative incidences negative on both paths.",
        );
        let start = Instant::now();

        let spec = HyperSpec {
            signs: SignPattern::Mixed,
            ..HyperSpec::medium()
        };
        let hg = spec.build();
        let x_edges = edge_vector(hg.num_edges());
        let expected = reference_scatter_edge_major(&hg, &x_edges, false);

        let mut parallel = vec![POISON; hg.num_nodes()];
        scatter_nodes_from_edges_parallel(&hg, &x_edges, &mut parallel, false);
        assert_bitwise(&parallel, &expected, "scatter use_abs=false");

        assert!(
            expected.iter().any(|&v| v < 0.0),
            "the fixture should produce some negative node values, else the test proves nothing"
        );

        log_test_footer(
            "test_scatter_without_abs_preserves_signs",
            Some(start.elapsed()),
            "Signed scatter matched the reference and did produce negative values.",
        );
    }

    // -----------------------------------------------------------------
    // clique diagonal
    // -----------------------------------------------------------------

    #[test]
    fn test_clique_diag_matches_edge_major_bitwise() {
        log_test_header(
            "test_clique_diag_matches_edge_major_bitwise",
            "clique_diag / clique_diag_into vs the edge-major sum of squares.",
        );
        let start = Instant::now();

        for (label, spec) in specs() {
            let hg = spec.build();
            let expected = reference_clique_diag_edge_major(&hg, true);

            let got = clique_diag(&hg, true);
            assert_bitwise(&got, &expected, &format!("clique_diag [{label}]"));

            let mut into = vec![POISON; hg.num_nodes()];
            clique_diag_into(&hg, true, &mut into);
            assert_bitwise(&into, &expected, &format!("clique_diag_into [{label}]"));
        }

        log_test_footer(
            "test_clique_diag_matches_edge_major_bitwise",
            Some(start.elapsed()),
            "clique_diag is bitwise equal to the edge-major reference at every scale.",
        );
    }

    // -----------------------------------------------------------------
    // implicit clique step — fused diagonal removal
    // -----------------------------------------------------------------

    #[test]
    fn test_implicit_clique_step_matches_unfused_pipeline() {
        log_test_header(
            "test_implicit_clique_step_matches_unfused_pipeline",
            "Fusing the diagonal removal into the scatter must not change the result.",
        );
        let start = Instant::now();

        for (label, spec) in specs() {
            let hg = spec.build();
            let x = spec.node_vector();
            let cfg = CliqueStepCfg {
                use_abs: true,
                include_self: false,
            };
            let diag = clique_diag(&hg, cfg.use_abs);

            // fused (production)
            let mut y_fused = vec![POISON; hg.num_nodes()];
            let mut buf = vec![POISON; hg.num_edges()];
            implicit_clique_step(&hg, &x, &mut y_fused, &mut buf, Some(&diag), cfg);

            // unfused: gather, scatter, then a separate remove_self_effect pass
            let mut x_edges = vec![POISON; hg.num_edges()];
            gather_edges_from_nodes(&hg, &x, &mut x_edges, cfg.use_abs);
            let mut y_unfused = vec![POISON; hg.num_nodes()];
            scatter_nodes_from_edges(&hg, &x_edges, &mut y_unfused, cfg.use_abs);
            remove_self_effect(&mut y_unfused, &diag, &x);

            assert_bitwise(
                &y_fused,
                &y_unfused,
                &format!("fused vs unfused clique step [{label}]"),
            );
        }

        log_test_footer(
            "test_implicit_clique_step_matches_unfused_pipeline",
            Some(start.elapsed()),
            "The fused diagonal removal is bitwise equal to the separate elementwise pass.",
        );
    }

    #[test]
    fn test_implicit_clique_step_include_self_skips_diagonal() {
        log_test_header(
            "test_implicit_clique_step_include_self_skips_diagonal",
            "include_self = true must equal a plain scatter and need no diagonal.",
        );
        let start = Instant::now();

        let spec = HyperSpec::small();
        let hg = spec.build();
        let x = spec.node_vector();
        let cfg = CliqueStepCfg {
            use_abs: true,
            include_self: true,
        };

        let mut y = vec![POISON; hg.num_nodes()];
        let mut buf = vec![POISON; hg.num_edges()];
        implicit_clique_step(&hg, &x, &mut y, &mut buf, None, cfg);

        let mut x_edges = vec![POISON; hg.num_edges()];
        gather_edges_from_nodes(&hg, &x, &mut x_edges, cfg.use_abs);
        let mut expected = vec![POISON; hg.num_nodes()];
        scatter_nodes_from_edges(&hg, &x_edges, &mut expected, cfg.use_abs);

        assert_bitwise(&y, &expected, "include_self=true vs plain scatter");

        log_test_footer(
            "test_implicit_clique_step_include_self_skips_diagonal",
            Some(start.elapsed()),
            "include_self=true took the no-diagonal branch and matched a plain scatter.",
        );
    }

    #[test]
    #[should_panic(expected = "Precomputed diagonal required")]
    fn test_implicit_clique_step_requires_diagonal() {
        let spec = HyperSpec::small();
        let hg = spec.build();
        let x = spec.node_vector();
        let mut y = vec![0.0f32; hg.num_nodes()];
        let mut buf = vec![0.0f32; hg.num_edges()];
        implicit_clique_step(
            &hg,
            &x,
            &mut y,
            &mut buf,
            None,
            CliqueStepCfg {
                use_abs: true,
                include_self: false,
            },
        );
    }

    // -----------------------------------------------------------------
    // degenerate shapes
    // -----------------------------------------------------------------

    #[test]
    fn test_empty_and_singleton_hypergraphs() {
        log_test_header(
            "test_empty_and_singleton_hypergraphs",
            "No nodes / no edges / one node one edge must not panic or divide by zero.",
        );
        let start = Instant::now();

        // no edges at all: every kernel must produce zeros, not garbage
        let no_edges = HyperSpec {
            nodes: 8,
            edges: 0,
            ..HyperSpec::small()
        };
        let hg = no_edges.build();
        assert_eq!(hg.num_nodes(), 8);
        assert_eq!(hg.num_edges(), 0);

        let mut y = vec![POISON; 8];
        scatter_nodes_from_edges_parallel(&hg, &[], &mut y, true);
        assert_bitwise(
            &y,
            &vec![0.0f32; 8],
            "scatter with no edges must zero the output",
        );
        assert_bitwise(
            &clique_diag(&hg, true),
            &vec![0.0f32; 8],
            "diag with no edges",
        );

        // no nodes at all
        let no_nodes = HyperSpec {
            nodes: 0,
            edges: 0,
            ..HyperSpec::small()
        };
        let hg0 = no_nodes.build();
        let mut empty: Vec<f32> = Vec::new();
        scatter_nodes_from_edges(&hg0, &[], &mut empty, true);
        gather_edges_from_nodes(&hg0, &[], &mut empty, true);
        assert!(clique_diag(&hg0, true).is_empty());

        // singleton: one node, one edge containing it
        let single = HyperSpec {
            nodes: 1,
            edges: 1,
            avg_cardinality: 1,
            shape: DegreeShape::Uniform,
            signs: SignPattern::AllPlus,
            seed: 7,
        };
        let hg1 = single.build();
        assert_eq!(hg1.num_nodes(), 1);
        assert_eq!(hg1.num_edges(), 1);
        let x1 = vec![2.0f32];
        let mut xe = vec![POISON; 1];
        gather_edges_from_nodes(&hg1, &x1, &mut xe, true);
        let mut y1 = vec![POISON; 1];
        scatter_nodes_from_edges(&hg1, &xe, &mut y1, true);
        assert!(
            y1[0].is_finite(),
            "singleton produced a non-finite value: {}",
            y1[0]
        );

        log_test_footer(
            "test_empty_and_singleton_hypergraphs",
            Some(start.elapsed()),
            "Empty, edgeless and singleton hypergraphs behaved correctly on both paths.",
        );
    }
}
