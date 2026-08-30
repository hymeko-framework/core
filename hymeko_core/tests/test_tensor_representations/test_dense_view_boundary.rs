//! M2c.1 — the dense-view-index / canonical-id boundary.
//!
//! `APPROVED-CORE-EDIT: dense-view-index-compute-degree`. These tests pin the
//! separation between canonical `NodeId`/`EdgeId` and view-local
//! `DenseNodeIndex`/`DenseEdgeIndex`, and are the catchers for the four
//! fault-injection mutants in the report:
//!
//! 1. node/edge dense type swapped → orientation/mapping tests diverge;
//! 2. missing checked narrowing → `overflow_is_rejected`;
//! 3. wrong dense→canonical mapping → `mapping_is_exact_on_non_contiguous_decls`;
//! 4. one incidence orientation retaining canonical raw indexing →
//!    `both_orientations_agree`.

#[cfg(test)]
mod tests {
    use hymeko::common::ids::{DeclId, EdgeId, NodeId};
    use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg};
    use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
    use hymeko_hnn::traversal::dense_index::{DenseEdgeIndex, DenseIndexError, DenseNodeIndex};
    use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
    use hymeko_hnn::workloads::{DegreeShape, HyperSpec, SignPattern};
    use std::collections::BTreeSet;

    type View = HyperGraphView<f32, EdgeWScalar<f32>, f32>;

    fn cfg() -> AggCfg {
        AggCfg {
            weight: WeightAgg::Sum,
            sign: SignAgg::PreferNonNeutral,
            clamp01: false,
        }
    }

    fn sample_view() -> View {
        // A deterministic multi-edge workload, non-trivial for CSR. Reuses the
        // existing workload generator (no second generator).
        let spec = HyperSpec {
            nodes: 64,
            edges: 48,
            avg_cardinality: 4,
            shape: DegreeShape::Uniform,
            signs: SignPattern::Mixed,
            seed: 7,
        };
        View::from_ir(&spec.ir(), &cfg(), &ScalarWeightExtractor)
    }

    #[test]
    fn dense_indices_are_four_bytes_and_option_recorded() {
        assert_eq!(std::mem::size_of::<DenseNodeIndex>(), 4);
        assert_eq!(std::mem::size_of::<DenseEdgeIndex>(), 4);
        assert_eq!(std::mem::size_of::<Option<DenseNodeIndex>>(), 8);
    }

    #[test]
    fn overflow_is_rejected() {
        assert_eq!(
            DenseNodeIndex::new_checked((u32::MAX as usize) + 1),
            Err(DenseIndexError::NodeCountOverflow {
                count: (u32::MAX as usize) + 1
            })
        );
    }

    #[test]
    fn both_orientations_agree() {
        // edge-major and node-major must carry the same (node, edge) dense
        // multiset. If one orientation regressed to canonical raw indexing, or
        // the node/edge dense types were swapped, these disagree.
        let v = sample_view();
        let mut edge_major: BTreeSet<(usize, usize)> = BTreeSet::new();
        for e in v.edge_indices() {
            for &n in v.edge_nodes(e) {
                edge_major.insert((n.as_usize(), e.as_usize()));
            }
        }
        let mut node_major: BTreeSet<(usize, usize)> = BTreeSet::new();
        for n in v.node_indices() {
            for &e in v.node_edges(n) {
                node_major.insert((n.as_usize(), e.as_usize()));
            }
        }
        assert_eq!(edge_major, node_major, "orientations disagree");
        assert!(!edge_major.is_empty());
    }

    #[test]
    fn deterministic_rebuild_is_identical() {
        let a = sample_view();
        let b = sample_view();
        assert_eq!(a.flat_edge_nodes, b.flat_edge_nodes);
        assert_eq!(a.flat_node_edges, b.flat_node_edges);
        assert_eq!(a.edge_offsets, b.edge_offsets);
        assert_eq!(a.node_offsets, b.node_offsets);
    }

    #[test]
    fn dense_to_canonical_is_one_to_one_on_a_full_view() {
        // This view enumerates every node/edge in canonical order, so the dense
        // coordinate i maps to canonical NodeId::new(i) / EdgeId::new(i).
        let v = sample_view();
        for n in v.node_indices() {
            assert_eq!(v.dense_node_id(n), NodeId::new(n.as_usize()));
        }
        for e in v.edge_indices() {
            assert_eq!(v.dense_edge_id(e), EdgeId::new(e.as_usize()));
        }
    }

    #[test]
    fn mapping_is_exact_on_non_contiguous_decls() {
        // Fault-injection catcher #3: exercise the dense→declaration mapping with
        // deliberately NON-contiguous canonical decl raw values, so a mapping that
        // silently assumed `dense == decl.raw()` would be caught. We build the
        // mapping helper's input directly (a Vec<DeclId>) — not a second IR.
        let mut v = sample_view();
        // Overwrite the provenance with a non-contiguous decl table.
        v.node_decl = vec![DeclId::new(9), DeclId::new(2), DeclId::new(41)];
        v.edge_decl = vec![DeclId::new(100), DeclId::new(7)];
        assert_eq!(
            v.dense_node_decl(DenseNodeIndex::from_u32(0)),
            DeclId::new(9)
        );
        assert_eq!(
            v.dense_node_decl(DenseNodeIndex::from_u32(1)),
            DeclId::new(2)
        );
        assert_eq!(
            v.dense_node_decl(DenseNodeIndex::from_u32(2)),
            DeclId::new(41)
        );
        assert_eq!(
            v.dense_edge_decl(DenseEdgeIndex::from_u32(1)),
            DeclId::new(7)
        );
        // The dense coordinate is NOT the decl raw value: they are different.
        assert_ne!(
            v.dense_node_decl(DenseNodeIndex::from_u32(0)).raw(),
            DenseNodeIndex::from_u32(0).as_usize()
        );
    }
}
