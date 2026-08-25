//! Regression tests for `HyperGraphView::from_ir` construction.
//!
//! The 2026-08-24 optimisation replaced the second comparison sort in
//! `build_csr_node_to_edges` (`sort_unstable_by_key(|x| (x.n.0, x.e.0))`,
//! parallel above 8192 elements) with an `O(nnz + num_nodes)` counting sort.
//!
//! That is only valid because counting sort is **stable** and the input is
//! already `(edge, node)`-ordered and deduplicated: scattering into per-node
//! buckets in input order therefore leaves each node's slice in increasing edge
//! order. These tests pin that claim, because the `(node, edge)` ordering is
//! load-bearing well beyond construction — the parallel scatter in
//! `message_passing` relies on it for bitwise-identical output, so an ordering
//! bug here would surface as silent numerical drift there, not as a crash.

#[cfg(test)]
mod test_view_construction {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg};
    use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
    use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
    use hymeko_hnn::workloads::{DegreeShape, HyperSpec, SignPattern};
    use log::info;
    use std::time::Instant;

    type View = HyperGraphView<f32, EdgeWScalar<f32>, f32>;

    const CFG: AggCfg = AggCfg {
        weight: WeightAgg::Sum,
        sign: SignAgg::PreferNonNeutral,
        clamp01: false,
    };

    fn specs() -> Vec<(&'static str, HyperSpec)> {
        vec![
            ("small", HyperSpec::small()),
            // Crosses the old PAR_SORT_THRESHOLD (8192), i.e. the size at which
            // the replaced code switched to a parallel comparison sort.
            (
                "medium",
                HyperSpec {
                    nodes: 4_000,
                    edges: 2_000,
                    ..HyperSpec::small()
                },
            ),
            (
                "irregular",
                HyperSpec {
                    shape: DegreeShape::Irregular,
                    ..HyperSpec::small()
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

    fn build(spec: &HyperSpec) -> View {
        View::from_ir(&spec.ir(), &CFG, &ScalarWeightExtractor)
    }

    #[test]
    fn test_node_first_csr_is_node_edge_ordered() {
        log_test_header(
            "test_node_first_csr_is_node_edge_ordered",
            "The counting sort must yield strictly increasing edge ids within every node slice.",
        );
        let start = Instant::now();

        for (label, spec) in specs() {
            let hg = build(&spec);
            for v in 0..hg.num_nodes() {
                let s = hg.node_offsets[v];
                let e = hg.node_offsets[v + 1];
                let slice = &hg.flat_node_edges[s..e];
                for w in slice.windows(2) {
                    assert!(
                        w[0].0 < w[1].0,
                        "[{label}] node {v}: edge ids not strictly increasing ({} then {})",
                        w[0].0,
                        w[1].0
                    );
                }
            }
            info!(
                "[{label}] node-first CSR ordering verified over {} nodes",
                hg.num_nodes()
            );
        }

        log_test_footer(
            "test_node_first_csr_is_node_edge_ordered",
            Some(start.elapsed()),
            "Every node incidence slice is in strictly increasing edge order.",
        );
    }

    #[test]
    fn test_both_orientations_hold_the_same_multiset() {
        log_test_header(
            "test_both_orientations_hold_the_same_multiset",
            "Edge-first and node-first CSRs must encode identical (edge, node, sign, weight) tuples.",
        );
        let start = Instant::now();

        for (label, spec) in specs() {
            let hg = build(&spec);

            // Edge-first traversal, collected as (e, n, sign, weight bits).
            let mut from_edges: Vec<(usize, usize, i8, u32)> = Vec::new();
            for e in 0..hg.num_edges() {
                for p in hg.edge_offsets[e]..hg.edge_offsets[e + 1] {
                    from_edges.push((
                        e,
                        hg.flat_edge_nodes[p].0,
                        hg.flat_edge_sign[p],
                        hg.flat_edge_w[p].to_bits(),
                    ));
                }
            }

            // Node-first traversal, same tuples.
            let mut from_nodes: Vec<(usize, usize, i8, u32)> = Vec::new();
            for v in 0..hg.num_nodes() {
                for p in hg.node_offsets[v]..hg.node_offsets[v + 1] {
                    from_nodes.push((
                        hg.flat_node_edges[p].0,
                        v,
                        hg.flat_node_sign[p],
                        hg.flat_node_w[p].to_bits(),
                    ));
                }
            }

            assert_eq!(
                from_edges.len(),
                from_nodes.len(),
                "[{label}] incidence counts differ between orientations"
            );

            // Sorting both by (edge, node) must make them identical - weights
            // compared bitwise, since a lossy copy would still pass a tolerance.
            from_nodes.sort_unstable();
            let mut expected = from_edges.clone();
            expected.sort_unstable();
            assert_eq!(
                from_nodes, expected,
                "[{label}] the two orientations encode different incidence data"
            );

            info!(
                "[{label}] {} incidences agree across both orientations",
                from_edges.len()
            );
        }

        log_test_footer(
            "test_both_orientations_hold_the_same_multiset",
            Some(start.elapsed()),
            "Edge-first and node-first CSRs carry bit-identical incidence data.",
        );
    }

    #[test]
    fn test_offsets_are_well_formed() {
        log_test_header(
            "test_offsets_are_well_formed",
            "Both offset arrays must be non-decreasing and terminate at nnz.",
        );
        let start = Instant::now();

        for (label, spec) in specs() {
            let hg = build(&spec);

            assert_eq!(
                hg.edge_offsets.len(),
                hg.num_edges() + 1,
                "[{label}] edge_offsets length"
            );
            assert_eq!(
                hg.node_offsets.len(),
                hg.num_nodes() + 1,
                "[{label}] node_offsets length"
            );
            assert_eq!(
                hg.edge_offsets[0], 0,
                "[{label}] edge_offsets must start at 0"
            );
            assert_eq!(
                hg.node_offsets[0], 0,
                "[{label}] node_offsets must start at 0"
            );

            for w in hg.edge_offsets.windows(2) {
                assert!(w[0] <= w[1], "[{label}] edge_offsets not monotonic");
            }
            for w in hg.node_offsets.windows(2) {
                assert!(w[0] <= w[1], "[{label}] node_offsets not monotonic");
            }

            let nnz = hg.flat_edge_nodes.len();
            assert_eq!(
                *hg.edge_offsets.last().unwrap(),
                nnz,
                "[{label}] edge_offsets tail"
            );
            assert_eq!(
                *hg.node_offsets.last().unwrap(),
                nnz,
                "[{label}] node_offsets tail"
            );
            assert_eq!(hg.flat_node_edges.len(), nnz, "[{label}] node-first nnz");
            assert_eq!(hg.flat_edge_sign.len(), nnz, "[{label}] edge sign length");
            assert_eq!(hg.flat_node_sign.len(), nnz, "[{label}] node sign length");
            assert_eq!(hg.flat_edge_w.len(), nnz, "[{label}] edge weight length");
            assert_eq!(hg.flat_node_w.len(), nnz, "[{label}] node weight length");
        }

        log_test_footer(
            "test_offsets_are_well_formed",
            Some(start.elapsed()),
            "Both CSR offset arrays are monotonic, zero-based and consistent with nnz.",
        );
    }

    #[test]
    fn test_empty_ir_builds_an_empty_view() {
        log_test_header(
            "test_empty_ir_builds_an_empty_view",
            "An IR with no nodes and no edges must not panic the counting sort.",
        );
        let start = Instant::now();

        let empty = HyperSpec {
            nodes: 0,
            edges: 0,
            ..HyperSpec::small()
        };
        let hg = build(&empty);
        assert_eq!(hg.num_nodes(), 0);
        assert_eq!(hg.num_edges(), 0);
        assert!(hg.flat_node_edges.is_empty());
        assert!(hg.flat_edge_nodes.is_empty());

        // Nodes but no edges: buckets exist, all empty.
        let edgeless = HyperSpec {
            nodes: 12,
            edges: 0,
            ..HyperSpec::small()
        };
        let hg2 = build(&edgeless);
        assert_eq!(hg2.num_nodes(), 12);
        assert_eq!(hg2.num_edges(), 0);
        assert!(
            hg2.node_offsets.iter().all(|&o| o == 0),
            "all node buckets must be empty"
        );

        log_test_footer(
            "test_empty_ir_builds_an_empty_view",
            Some(start.elapsed()),
            "Empty and edgeless IRs produced well-formed empty views.",
        );
    }
}
