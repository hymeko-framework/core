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

use crate::unified_ir::support::corpus::require_fixtures;

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
            let arc = &ir.arcs[aid.raw()];
            for r in &arc.refs {
                let target: DeclId = match r {
                    SignedRefR::Plus(a) | SignedRefR::Minus(a) | SignedRefR::Neutral(a) => a.target,
                };
                if let Some(nid) = ir.decl_to_node[target.raw()] {
                    let sgn = match r {
                        SignedRefR::Plus(_) => 1i8,
                        SignedRefR::Minus(_) => -1i8,
                        SignedRefR::Neutral(_) => 0i8,
                    };
                    out.insert((eid_usize, nid.raw(), sgn));
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
        let (lo, hi) =
            v.edge_span(hymeko_hnn::traversal::dense_index::DenseEdgeIndex::from_u32(e as u32));
        for k in lo..hi {
            out.insert((e, v.flat_edge_nodes[k].as_usize(), v.flat_edge_sign[k]));
        }
    }
    out
}

#[test]
fn hypergraph_view_incidences_match_the_direct_reference_on_the_corpus() {
    let files = require_fixtures();
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
    let files = require_fixtures();
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

// --- malformed input must be observable ------------------------------------
//
// M2b shipped `collect_triples` skipping a dangling arc with a bare `continue`,
// which produced a view holding fewer incidences than the IR describes with
// nothing downstream able to tell. The checked API reports it.

use hymeko_hnn::traversal::hypergraphview::ViewBuildError;

use crate::unified_ir::support::builder::{S, TestIr};

/// Two nodes joined by one edge, with a fully populated declaration table.
fn malformable() -> (TestIr, hymeko::common::ids::EdgeId) {
    let mut t = TestIr::new();
    let root = t.node(&["m"], DeclId::NONE);
    let x = t.node(&["m", "x"], root);
    let y = t.node(&["m", "y"], root);
    let e = t.edge(&["m", "e"], root);
    t.arc(e, &[(x, S::Plus), (y, S::Minus)]);
    let eid = t.ir.as_edge(e).expect("edge mapping");
    (t, eid)
}

#[test]
fn try_from_ir_reports_a_dangling_arc() {
    let (mut t, eid) = malformable();
    t.ir.edges[eid.raw()]
        .arcs
        .push(hymeko::common::ids::HyperArcId::new(9_999));

    let Err(err) = View::try_from_ir(&t.ir, &cfg(), &ScalarWeightExtractor) else {
        panic!("a dangling arc must be reported");
    };
    assert!(
        matches!(err, ViewBuildError::DanglingArc { .. }),
        "expected DanglingArc, got {err:?}"
    );
}

#[test]
fn try_from_ir_reports_a_dangling_target() {
    let (mut t, _eid) = malformable();
    // Point the first reference at a declaration that does not exist. The
    // declaration table IS populated here, so it can serve as the oracle.
    let atom = t.ir.arcs[0].refs[0].atom().clone();
    let mut bad = atom;
    bad.target = DeclId::new(9_999);
    t.ir.arcs[0].refs[0] = hymeko::ir::ir::SignedRefR::Plus(bad);

    let Err(err) = View::try_from_ir(&t.ir, &cfg(), &ScalarWeightExtractor) else {
        panic!("a dangling target must be reported");
    };
    assert!(
        matches!(err, ViewBuildError::DanglingTarget { .. }),
        "expected DanglingTarget, got {err:?}"
    );
}

#[test]
fn try_from_ir_reports_a_node_declaration_with_no_record() {
    let (mut t, _eid) = malformable();
    // `m.x` stays a Node declaration but loses its node mapping.
    let x = DeclId::new(1);
    assert_eq!(t.ir.kind_of(x), Some(hymeko::ir::ir::DeclKind::Node));
    t.ir.decl_to_node[x.raw()] = None;

    let Err(err) = View::try_from_ir(&t.ir, &cfg(), &ScalarWeightExtractor) else {
        panic!("a Node without a record must be reported");
    };
    assert!(
        matches!(err, ViewBuildError::MissingNodeMapping { .. }),
        "expected MissingNodeMapping, got {err:?}"
    );
}

/// An edge-to-edge reference is **not** an error. This view is vertex-centric
/// and has always skipped them; turning that into a failure would break the
/// rewrite engine's output rather than catch a defect.
#[test]
fn try_from_ir_accepts_an_edge_to_edge_reference() {
    let (mut t, _eid) = malformable();
    let other_edge = t.edge(&["m", "e2"], DeclId::new(0));
    let atom = t.ir.arcs[0].refs[0].atom().clone();
    let mut to_edge = atom;
    to_edge.target = other_edge;
    t.ir.arcs[0].refs[0] = hymeko::ir::ir::SignedRefR::Plus(to_edge);

    let view = View::try_from_ir(&t.ir, &cfg(), &ScalarWeightExtractor)
        .expect("an edge-to-edge reference is legitimate and must be skipped");
    // Only the surviving node reference contributes.
    assert_eq!(view.num_edges(), 2);
}

/// The compatibility wrapper keeps the pre-M2b fail-fast contract.
#[test]
#[should_panic(expected = "malformed Ir")]
fn from_ir_still_fails_fast_on_malformed_input() {
    let (mut t, eid) = malformable();
    t.ir.edges[eid.raw()]
        .arcs
        .push(hymeko::common::ids::HyperArcId::new(9_999));
    let _ = View::from_ir(&t.ir, &cfg(), &ScalarWeightExtractor);
}

/// Checked and unchecked entry points agree on every valid fixture.
#[test]
fn try_from_ir_and_from_ir_agree_on_the_corpus() {
    let files = require_fixtures();
    let c = cfg();
    let ex = ScalarWeightExtractor;
    let mut checked = 0usize;
    for f in &files {
        let Ok((_store, compiled)) = crate::test_helpers::load_and_lower(f) else {
            continue;
        };
        let a = View::try_from_ir(&compiled.ir, &c, &ex)
            .unwrap_or_else(|e| panic!("corpus fixture {} is malformed: {e}", f.display()));
        let b = View::from_ir(&compiled.ir, &c, &ex);
        assert_eq!(a.flat_edge_nodes, b.flat_edge_nodes, "{}", f.display());
        assert_eq!(a.flat_edge_sign, b.flat_edge_sign, "{}", f.display());
        assert_eq!(a.edge_offsets, b.edge_offsets, "{}", f.display());
        assert_eq!(a.node_offsets, b.node_offsets, "{}", f.display());
        checked += 1;
    }
    eprintln!("[m2b-corr] try_from_ir == from_ir on {checked} fixtures");
    assert!(checked > 0, "no fixture compared");
}

/// `Display` is public contract — the message must name the offending ids.
///
/// The tests above match on the variant with `Debug`, which leaves the `Display`
/// arms unexecuted. `from_ir`'s panic message formats with `Display`, so these
/// are the strings a caller actually sees.
#[test]
fn view_build_error_display_names_the_offending_ids() {
    use hymeko::common::ids::HyperArcId;

    let cases = [
        (
            ViewBuildError::DanglingArc {
                edge: EdgeId::new(3),
                arc: HyperArcId::new(7),
            },
            vec!["3", "7"],
        ),
        (
            ViewBuildError::DanglingTarget {
                arc: HyperArcId::new(11),
                target: DeclId::new(13),
            },
            vec!["11", "13"],
        ),
        (
            ViewBuildError::MissingNodeMapping {
                target: DeclId::new(17),
            },
            vec!["17"],
        ),
    ];

    for (err, must_contain) in cases {
        let s = err.to_string();
        for needle in must_contain {
            assert!(
                s.contains(needle),
                "Display for {err:?} should name {needle}: {s:?}"
            );
        }
        // The std::error::Error impl must be usable as a source.
        let as_err: &dyn std::error::Error = &err;
        assert_eq!(as_err.to_string(), s);
    }
}

/// An `Ir` with no declaration table must be **rejected**, not quietly accepted.
///
/// This test is the inverse of the M2b-corrective
/// `try_from_ir_accepts_an_ir_with_no_declaration_table`, and it is kept in that
/// inverted form rather than deleted so the reversal stays legible.
///
/// The original argued that a consumer which resolves references through
/// `decl_to_node`, and has never read `decl_nodes`, must not invent an
/// existence requirement `from_ir` never had. That reasoning was sound about
/// the *consumer* and wrong about the *system*: it left `UIR-M2B-06` —
/// malformed IR is never silently accepted — false for the one input shape that
/// reached the escape. The only producer of that shape was
/// `hymeko_hnn::workloads::HyperSpec`, an incoherent synthetic fixture
/// (`UIR-DEFECT-04`). The fixture is fixed at the producer, so the escape has no
/// legitimate caller left and the strict API now rejects every unknown target.
#[test]
fn try_from_ir_rejects_an_ir_with_no_declaration_table() {
    use hymeko::ir::ir::{ArcRec, EdgeRec, Ir, NodeRec};

    let mut ir = Ir::default();
    // Two node records and one edge with one arc — but no declarations.
    ir.nodes.push(NodeRec::new(DeclId::new(0), Vec::new()));
    ir.nodes.push(NodeRec::new(DeclId::new(1), Vec::new()));
    ir.decl_to_node = vec![Some(NodeId::new(0)), Some(NodeId::new(1))];
    ir.arcs.push(ArcRec {
        anno: Default::default(),
        in_edge: DeclId::new(2),
        refs: vec![
            SignedRefR::Plus(hymeko::ir::ir::RefAtomR {
                target: DeclId::new(0),
                anno: Default::default(),
                weights: None,
            }),
            SignedRefR::Minus(hymeko::ir::ir::RefAtomR {
                target: DeclId::new(1),
                anno: Default::default(),
                weights: None,
            }),
            // The reference that used to be skipped: in neither `decl_to_node`
            // nor the declaration table.
            SignedRefR::Neutral(hymeko::ir::ir::RefAtomR {
                target: DeclId::new(7),
                anno: Default::default(),
                weights: None,
            }),
        ],
    });
    let mut e = EdgeRec::new(DeclId::new(2), Vec::new());
    e.arcs.push(hymeko::common::ids::HyperArcId::new(0));
    ir.edges.push(e);

    assert_eq!(ir.decl_count(), 0, "fixture precondition: no declarations");

    // The first two references resolve and are *not* errors: `as_node` is a
    // MAP-ONLY read, and a mapping that exists is an answer. Only the third
    // target is unknown, and one unknown target is enough to reject the whole
    // build — previously it was dropped and the view came back two-thirds
    // complete with nothing to say so.
    let Err(err) = View::try_from_ir(&ir, &cfg(), &ScalarWeightExtractor) else {
        panic!("an Ir with an unresolvable target must be rejected");
    };
    assert_eq!(
        err,
        ViewBuildError::DanglingTarget {
            arc: hymeko::common::ids::HyperArcId::new(0),
            target: DeclId::new(7),
        }
    );
}

/// The escape in its purest form: every reference unknown, `decl_count() == 0`.
///
/// Kept separate from the test above because the old behaviour here was worse
/// than partial loss — skipping the only reference produced a structurally
/// valid view with **zero** incidences and an `Ok` verdict. A consumer had no
/// way to distinguish that from a hypergraph whose edge is genuinely empty.
#[test]
fn try_from_ir_rejects_a_dangling_target_when_the_declaration_table_is_empty() {
    use hymeko::ir::ir::{ArcRec, EdgeRec, Ir, NodeRec};

    let mut ir = Ir::default();
    ir.nodes.push(NodeRec::new(DeclId::new(0), Vec::new()));
    ir.decl_to_node = vec![Some(NodeId::new(0))];
    ir.arcs.push(ArcRec {
        anno: Default::default(),
        in_edge: DeclId::new(1),
        refs: vec![SignedRefR::Neutral(hymeko::ir::ir::RefAtomR {
            // In neither `decl_to_node` nor `decl_nodes`.
            target: DeclId::new(9),
            anno: Default::default(),
            weights: None,
        })],
    });
    let mut e = EdgeRec::new(DeclId::new(1), Vec::new());
    e.arcs.push(hymeko::common::ids::HyperArcId::new(0));
    ir.edges.push(e);

    assert_eq!(ir.decl_count(), 0, "fixture precondition: no declarations");

    let Err(err) = View::try_from_ir(&ir, &cfg(), &ScalarWeightExtractor) else {
        panic!("an unknown target must be rejected regardless of decl_count");
    };
    assert_eq!(
        err,
        ViewBuildError::DanglingTarget {
            arc: hymeko::common::ids::HyperArcId::new(0),
            target: DeclId::new(9),
        }
    );
}
