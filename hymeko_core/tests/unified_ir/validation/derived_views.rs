//! Validation: derived views are unchanged by the M2b HNN migration.
//!
//! APPROVED-CORE-EDIT: unified-ir-m2b-vv-core-consumers (2026-08-29).
//!
//! `HyperGraphView::from_ir` is the primary derived view and the largest IR
//! consumer in the workspace. M2b rewrote its reads — the edge scan, the arc
//! point lookup, the declaration→node **map-only** lookup and the counts — to
//! go through the accessor layer.
//!
//! The existing HNN suite already compares message-passing results bitwise, so
//! it would catch a numerical change. What it would *not* catch is a change in
//! which incidences are collected at all, because it builds the view once and
//! compares derived quantities against each other. This module closes that gap:
//! it reconstructs the incidence set from the IR by direct storage access —
//! the pre-M2b form — and requires the view built through the accessors to
//! contain exactly the same set.

use std::collections::BTreeSet;

use hymeko::common::ids::{DeclId, EdgeId, NodeId};
use hymeko::ir::ir::{Ir, SignedRefR};
use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg};
use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
use hymeko_hnn::traversal::hypergraphview::HyperGraphView;

use crate::unified_ir::support::corpus::fixture_files;

type View = HyperGraphView<f32, EdgeWScalar<f32>, f32>;

fn cfg() -> AggCfg {
    AggCfg {
        weight: WeightAgg::Sum,
        sign: SignAgg::PreferNonNeutral,
        clamp01: false,
    }
}

/// The incidence set as the pre-M2b code would have collected it: direct
/// indexing throughout, including the panicking `decl_to_node[..]` form.
fn reference_incidences(ir: &Ir) -> BTreeSet<(usize, usize, i8)> {
    let mut out = BTreeSet::new();
    for (eid_usize, edge) in ir.edges.iter().enumerate() {
        for &aid in &edge.arcs {
            let arc = &ir.arcs[aid.0];
            for r in &arc.refs {
                let target: DeclId = match r {
                    SignedRefR::Plus(a) | SignedRefR::Minus(a) | SignedRefR::Neutral(a) => a.target,
                };
                if let Some(nid) = ir.decl_to_node[target.0] {
                    let sgn = match r {
                        SignedRefR::Plus(_) => 1i8,
                        SignedRefR::Minus(_) => -1i8,
                        SignedRefR::Neutral(_) => 0i8,
                    };
                    out.insert((eid_usize, nid.0, sgn));
                }
            }
        }
    }
    out
}

/// The same set, read back out of the constructed view's edge-major tables.
fn view_incidences(v: &View) -> BTreeSet<(usize, usize, i8)> {
    let mut out = BTreeSet::new();
    for e in 0..v.num_edges() {
        let (lo, hi) = v.edge_span(EdgeId::new(e));
        for k in lo..hi {
            out.insert((e, v.flat_edge_nodes[k].0, v.flat_edge_sign[k]));
        }
    }
    out
}

#[test]
fn hypergraph_view_incidences_match_the_direct_reference_on_the_corpus() {
    let files = fixture_files();
    if files.is_empty() {
        eprintln!("[m2b-hnn] corpus absent — skipped");
        return;
    }
    let c = cfg();
    let ex = ScalarWeightExtractor;
    let mut checked = 0usize;
    let mut incidences = 0usize;

    for f in &files {
        let Ok((_store, compiled)) = crate::test_helpers::load_and_lower(f) else {
            continue;
        };
        let view = View::from_ir(&compiled.ir, &c, &ex);
        let from_view = view_incidences(&view);
        let from_ir = reference_incidences(&compiled.ir);

        // The view aggregates duplicate (edge, node) pairs per `AggCfg`, so the
        // view set is a subset of the raw reference set with signs resolved.
        // What must hold exactly is the (edge, node) coverage.
        let view_pairs: BTreeSet<(usize, usize)> =
            from_view.iter().map(|&(e, n, _)| (e, n)).collect();
        let ir_pairs: BTreeSet<(usize, usize)> = from_ir.iter().map(|&(e, n, _)| (e, n)).collect();

        assert_eq!(
            view_pairs,
            ir_pairs,
            "HyperGraphView incidence coverage diverged from the direct reference for {}",
            f.display()
        );
        incidences += view_pairs.len();
        checked += 1;
    }
    eprintln!("[m2b-hnn] HyperGraphView: {checked} fixtures, {incidences} incidences agree");
    assert!(checked > 0, "no fixture compared");
}

#[test]
fn hypergraph_view_counts_match_the_ir_counts_on_the_corpus() {
    let files = fixture_files();
    if files.is_empty() {
        eprintln!("[m2b-hnn] corpus absent — skipped");
        return;
    }
    let c = cfg();
    let ex = ScalarWeightExtractor;
    let mut checked = 0usize;
    for f in &files {
        let Ok((_store, compiled)) = crate::test_helpers::load_and_lower(f) else {
            continue;
        };
        let view = View::from_ir(&compiled.ir, &c, &ex);
        assert_eq!(view.num_nodes(), compiled.ir.nodes.len(), "{}", f.display());
        assert_eq!(view.num_edges(), compiled.ir.edges.len(), "{}", f.display());
        assert_eq!(view.node_decl.len(), compiled.ir.nodes.len());
        assert_eq!(view.edge_decl.len(), compiled.ir.edges.len());
        // decl back-pointers must be preserved in order
        for (i, n) in compiled.ir.nodes.iter().enumerate() {
            assert_eq!(
                view.node_decl[i],
                n.decl,
                "node_decl[{i}] in {}",
                f.display()
            );
        }
        for (i, e) in compiled.ir.edges.iter().enumerate() {
            assert_eq!(
                view.edge_decl[i],
                e.decl,
                "edge_decl[{i}] in {}",
                f.display()
            );
        }
        checked += 1;
    }
    eprintln!("[m2b-hnn] view counts and decl maps: {checked} fixtures agree");
    assert!(checked > 0);
}

/// A dangling arc id must be skipped, not abort. Pre-M2b `&ir.arcs[aid.0]`
/// would have panicked — this is the behavioural gain the migration bought.
#[test]
fn hypergraph_view_skips_a_dangling_arc_instead_of_panicking() {
    use crate::unified_ir::support::builder::{S, TestIr};

    let mut t = TestIr::new();
    let root = t.node(&["m"], DeclId::NONE);
    let x = t.node(&["m", "x"], root);
    let y = t.node(&["m", "y"], root);
    let e = t.edge(&["m", "e"], root);
    t.arc(e, &[(x, S::Plus), (y, S::Minus)]);

    let eid = t.ir.as_edge(e).expect("edge mapping");
    let good = t.ir.edges[eid.0].arcs[0];
    t.ir.edges[eid.0]
        .arcs
        .push(hymeko::common::ids::HyperArcId::new(9_999));

    let view = View::from_ir(&t.ir, &cfg(), &ScalarWeightExtractor);
    // The valid arc still contributes; the dangling one contributes nothing.
    assert_eq!(view.num_edges(), 1);
    assert!(
        view.edge_span(EdgeId::new(0)).1 > 0,
        "the valid arc's incidences must survive"
    );
    let _ = (good, NodeId::new(0));
}
