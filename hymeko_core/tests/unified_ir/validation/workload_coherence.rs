//! Validation: the synthetic workload fixtures are coherent declaration programs.
//!
//! APPROVED-CORE-EDIT: unified-ir-hyperspec-coherence (2026-08-29).
//!
//! `HyperSpec` is the fixture behind every `from_ir` benchmark and several
//! tensor regression tests, and it is the only producer in the workspace of an
//! `Ir` that reached `HyperGraphView` with an empty declaration table. That
//! shape forced the strict view to carry an escape clause — *unknown targets are
//! acceptable when there is no table to consult* — which made `UIR-M2B-06`
//! ("malformed IR is never silently accepted") false for exactly the input that
//! reached it. Recorded as `UIR-DEFECT-04`.
//!
//! The fix is on the producer side: the workload now builds every table.
//! This module is the regression evidence for that, and it checks the fixture
//! against `hymeko::ir::coherence` — the same validator production `Ir`s are
//! held to — rather than against a hand-written list of expectations.

use std::collections::BTreeSet;

use hymeko::common::ids::DeclId;
use hymeko::ir::coherence::{CoherenceLimits, validate_hierarchy_index_coherence};
use hymeko::ir::ir::{DeclKind, Ir, SignedRefR};
use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg};
use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
use hymeko_hnn::workloads::{HyperSpec, HyperSpecProgram};

type View = HyperGraphView<f32, EdgeWScalar<f32>, f32>;

fn cfg() -> AggCfg {
    AggCfg {
        weight: WeightAgg::Sum,
        sign: SignAgg::PreferNonNeutral,
        clamp01: false,
    }
}

/// The two scales this module exercises.
///
/// `large` is deliberately excluded: 200 000 nodes would add ~30 s to every
/// `cargo test` run for a property `small` and `medium` already establish, and
/// the benchmark suite covers it at the scale where it matters.
fn scales() -> Vec<(&'static str, HyperSpec)> {
    vec![
        ("small", HyperSpec::small()),
        ("medium", HyperSpec::medium()),
    ]
}

// ---------------------------------------------------------------------------
// The validator's verdict
// ---------------------------------------------------------------------------

#[test]
fn workload_programs_pass_the_coherence_validator() {
    for (label, spec) in scales() {
        for (variant, program) in [
            ("weighted", spec.program()),
            ("unweighted", spec.program_unweighted()),
        ] {
            let HyperSpecProgram {
                ir,
                index,
                interner,
            } = program;
            if let Err(report) = validate_hierarchy_index_coherence(
                &ir,
                &index,
                &interner,
                CoherenceLimits::default(),
            ) {
                panic!(
                    "{label}/{variant} workload is not coherent: {} finding(s), first {:?}",
                    report.total,
                    report.findings.first()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Exact counts
// ---------------------------------------------------------------------------

#[test]
fn workload_declaration_counts_are_exact() {
    for (label, spec) in scales() {
        let p = spec.program();
        let num_nodes = p.ir.nodes.len();
        let num_edges = p.ir.edges.len();
        let num_arcs = p.ir.arcs.len();

        assert_eq!(num_edges, spec.edges, "{label}: edge record count");
        assert_eq!(
            num_arcs, num_edges,
            "{label}: the workload emits exactly one arc per hyperedge"
        );
        assert_eq!(
            p.ir.decl_nodes.len(),
            num_nodes + num_edges + num_arcs,
            "{label}: one declaration per node, edge and arc"
        );

        // The five declaration-parallel arrays move together or not at all.
        for (name, len) in [
            ("decl_hash", p.ir.decl_hash.len()),
            ("decl_to_node", p.ir.decl_to_node.len()),
            ("decl_to_edge", p.ir.decl_to_edge.len()),
            ("decl_to_arc", p.ir.decl_to_arc.len()),
        ] {
            assert_eq!(
                len,
                p.ir.decl_nodes.len(),
                "{label}: {name} is out of step with decl_nodes"
            );
        }
        assert_eq!(
            p.ir.arc_hash.len(),
            num_arcs,
            "{label}: arc_hash has one slot per ArcRec"
        );

        // Named declarations are indexed; arc declarations are anonymous, which
        // is what production lowering produces.
        assert_eq!(
            p.index.by_path.len(),
            num_nodes + num_edges,
            "{label}: exactly the named declarations carry a path"
        );
    }
}

// ---------------------------------------------------------------------------
// Mapping and back-link invariants
// ---------------------------------------------------------------------------

#[test]
fn workload_maps_and_backlinks_agree() {
    for (label, spec) in scales() {
        let ir = spec.ir();
        let arc_decls = arc_declarations(&ir);

        for (i, rec) in ir.nodes.iter().enumerate() {
            assert_eq!(
                ir.as_node(rec.decl).map(|n| n.raw()),
                Some(i),
                "{label}: node {i} back-link does not round-trip"
            );
            assert_eq!(
                ir.kind_of(rec.decl),
                Some(DeclKind::Node),
                "{label}: node {i} declaration is not a Node"
            );
        }

        for (j, rec) in ir.edges.iter().enumerate() {
            assert_eq!(
                ir.as_edge(rec.decl).map(|e| e.raw()),
                Some(j),
                "{label}: edge {j} back-link does not round-trip"
            );
            assert_eq!(
                ir.kind_of(rec.decl),
                Some(DeclKind::Edge),
                "{label}: edge {j} declaration is not an Edge"
            );

            // Every arc the edge claims exists, and its declaration is a
            // HyperArc parented to this edge and reachable as its child.
            for &aid in &rec.arcs {
                assert!(
                    ir.arc(aid).is_some(),
                    "{label}: edge {j} lists arc {} which does not exist",
                    aid.raw()
                );
                let arc_decl = arc_decls[aid.raw()]
                    .unwrap_or_else(|| panic!("no declaration maps to arc {}", aid.raw()));
                assert_eq!(
                    ir.kind_of(arc_decl),
                    Some(DeclKind::HyperArc),
                    "{label}: arc {} declaration is not a HyperArc",
                    aid.raw()
                );
                assert_eq!(
                    ir.parent(arc_decl),
                    rec.decl,
                    "{label}: arc {} is not parented to its edge",
                    aid.raw()
                );
                assert_eq!(
                    ir.first_child(rec.decl),
                    arc_decl,
                    "{label}: edge {j} does not list its arc as a child"
                );
            }
        }
    }
}

/// `HyperArcId` -> the declaration that maps to it, built in one pass.
///
/// A per-arc linear scan would be O(arcs x declarations) — 4.4e9 comparisons at
/// the `medium` scale, which is minutes of test time for a lookup the storage
/// layout can answer in one sweep (CLAUDE.md §5).
fn arc_declarations(ir: &Ir) -> Vec<Option<DeclId>> {
    let mut out = vec![None; ir.arcs.len()];
    for d in 0..ir.decl_count() {
        let did = DeclId::new(d);
        if let Some(aid) = ir.as_arc(did) {
            out[aid.raw()] = Some(did);
        }
    }
    out
}

#[test]
fn every_workload_reference_targets_a_resolvable_node() {
    for (label, spec) in scales() {
        let ir = spec.ir();
        for (a, arc) in ir.arcs.iter().enumerate() {
            for r in &arc.refs {
                let target = match r {
                    SignedRefR::Plus(x) | SignedRefR::Minus(x) | SignedRefR::Neutral(x) => x.target,
                };
                assert_eq!(
                    ir.kind_of(target),
                    Some(DeclKind::Node),
                    "{label}: arc {a} references {} which is not a Node declaration",
                    target.raw()
                );
                assert!(
                    ir.as_node(target).is_some(),
                    "{label}: arc {a} references Node declaration {} with no record",
                    target.raw()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Weighted / unweighted structural identity
// ---------------------------------------------------------------------------

#[test]
fn weighted_and_unweighted_workloads_are_structurally_identical() {
    for (label, spec) in scales() {
        let w = spec.ir();
        let u = spec.ir_unweighted();

        assert_eq!(w.decl_count(), u.decl_count(), "{label}: declaration count");
        assert_eq!(w.nodes.len(), u.nodes.len(), "{label}: node count");
        assert_eq!(w.edges.len(), u.edges.len(), "{label}: edge count");
        assert_eq!(w.arcs.len(), u.arcs.len(), "{label}: arc count");
        assert_eq!(w.decl_to_node, u.decl_to_node, "{label}: decl_to_node");
        assert_eq!(w.decl_to_edge, u.decl_to_edge, "{label}: decl_to_edge");
        assert_eq!(w.decl_to_arc, u.decl_to_arc, "{label}: decl_to_arc");

        for (a, (wa, ua)) in w.arcs.iter().zip(&u.arcs).enumerate() {
            assert_eq!(wa.in_edge, ua.in_edge, "{label}: arc {a} in_edge");
            assert_eq!(
                wa.refs.len(),
                ua.refs.len(),
                "{label}: arc {a} reference count"
            );
            for (k, (wr, ur)) in wa.refs.iter().zip(&ua.refs).enumerate() {
                let (wt, ws) = classify(wr);
                let (ut, us) = classify(ur);
                assert_eq!((wt, ws), (ut, us), "{label}: arc {a} ref {k}");
            }
        }

        // The only permitted difference.
        assert!(
            w.arcs
                .iter()
                .flat_map(|a| &a.refs)
                .all(|r| weights_of(r).is_some()),
            "{label}: the weighted workload must carry weights"
        );
        assert!(
            u.arcs
                .iter()
                .flat_map(|a| &a.refs)
                .all(|r| weights_of(r).is_none()),
            "{label}: the unweighted workload must carry none"
        );
    }
}

fn classify(r: &SignedRefR) -> (DeclId, i8) {
    match r {
        SignedRefR::Plus(x) => (x.target, 1),
        SignedRefR::Minus(x) => (x.target, -1),
        SignedRefR::Neutral(x) => (x.target, 0),
    }
}

fn weights_of(r: &SignedRefR) -> Option<&Vec<hymeko::ir::ir::ValueR>> {
    match r {
        SignedRefR::Plus(x) | SignedRefR::Minus(x) | SignedRefR::Neutral(x) => x.weights.as_ref(),
    }
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

#[test]
fn repeated_workload_construction_is_bit_identical() {
    for (label, spec) in scales() {
        let a = spec.program();
        let b = spec.program();

        assert_eq!(a.ir.decl_count(), b.ir.decl_count(), "{label}");
        assert_eq!(a.ir.decl_to_node, b.ir.decl_to_node, "{label}");
        assert_eq!(a.ir.decl_to_edge, b.ir.decl_to_edge, "{label}");
        assert_eq!(a.ir.decl_to_arc, b.ir.decl_to_arc, "{label}");
        assert_eq!(a.index.by_path.len(), b.index.by_path.len(), "{label}");

        // Names are a function of the index, not of the seed, so the two
        // interners agree symbol for symbol.
        for d in 0..a.ir.decl_count() {
            let did = DeclId::new(d);
            let (an, bn) = (
                a.ir.decl_node(did).expect("declaration exists").name,
                b.ir.decl_node(did).expect("declaration exists").name,
            );
            assert_eq!(
                a.interner.resolve(an),
                b.interner.resolve(bn),
                "{label}: declaration {d} name"
            );
        }

        // Weights are seeded, so bit-equality here is the RNG determinism claim.
        let wa: Vec<f64> = numeric_weights(&a.ir);
        let wb: Vec<f64> = numeric_weights(&b.ir);
        assert_eq!(wa.len(), wb.len(), "{label}: weight count");
        assert!(
            wa.iter().zip(&wb).all(|(x, y)| x.to_bits() == y.to_bits()),
            "{label}: weights are not bit-identical across construction"
        );
    }
}

fn numeric_weights(ir: &Ir) -> Vec<f64> {
    ir.arcs
        .iter()
        .flat_map(|a| &a.refs)
        .filter_map(weights_of)
        .flatten()
        .map(|v| match v {
            hymeko::ir::ir::ValueR::Num(x) => *x,
            _ => f64::NAN,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Canonical hash stability
// ---------------------------------------------------------------------------

/// The fixture now carries the full input `canonical_program_hash` needs — an
/// `Ir`, an `Index` and an `Interner` — so the identity of a workload is a
/// stable function of its spec. Before this phase the call was not even
/// expressible: there was no `Index`.
#[test]
fn workload_canonical_hash_is_stable_across_construction() {
    use hymeko::ir::canonical_hash::{CanonHashCfg, canonical_program_hash};

    let cfg = CanonHashCfg::default();
    for (label, spec) in scales() {
        let a = spec.program();
        let b = spec.program();
        let ha = canonical_program_hash(cfg, &a.index, &a.ir, &a.interner);
        let hb = canonical_program_hash(cfg, &b.index, &b.ir, &b.interner);
        assert_eq!(ha, hb, "{label}: canonical hash is not construction-stable");

        // Structure, not weights: v1 is structural-only (`UIR-HASH-02`), so the
        // unweighted variant must hash the same. This is an assertion about the
        // hash's documented coverage, not about the fixture.
        let u = spec.program_unweighted();
        let hu = canonical_program_hash(cfg, &u.index, &u.ir, &u.interner);
        assert_eq!(
            ha, hu,
            "{label}: v1 is structural-only, so weights must not change the hash"
        );
    }
}

// ---------------------------------------------------------------------------
// The consumer that forced the escape clause
// ---------------------------------------------------------------------------

#[test]
fn try_from_ir_accepts_the_coherent_workload() {
    for (label, spec) in scales() {
        let ir = spec.ir();
        let view = View::try_from_ir(&ir, &cfg(), &ScalarWeightExtractor)
            .unwrap_or_else(|e| panic!("{label}: coherent workload rejected: {e}"));

        assert_eq!(view.num_nodes(), ir.nodes.len(), "{label}: node count");
        assert_eq!(view.num_edges(), ir.edges.len(), "{label}: edge count");

        // Nothing was dropped: every reference reaches the view.
        let refs: usize = ir.arcs.iter().map(|a| a.refs.len()).sum();
        let kept = view.flat_edge_nodes.len();
        assert!(
            kept <= refs,
            "{label}: the view cannot hold more incidences than the IR describes"
        );
        let irr = &ir;
        let distinct: BTreeSet<(usize, usize)> = irr
            .arcs
            .iter()
            .flat_map(|a| {
                let e = irr
                    .as_edge(a.in_edge)
                    .expect("workload arcs are owned by edges")
                    .raw();
                a.refs.iter().map(move |r| {
                    let t = classify(r).0;
                    (e, irr.as_node(t).expect("workload targets resolve").raw())
                })
            })
            .collect();
        assert_eq!(
            kept,
            distinct.len(),
            "{label}: the view holds one incidence per distinct (edge, node) pair"
        );
    }
}

#[test]
fn from_ir_and_try_from_ir_agree_on_the_workload() {
    for (label, spec) in scales() {
        let ir = spec.ir();
        let checked = View::try_from_ir(&ir, &cfg(), &ScalarWeightExtractor)
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        let compat = View::from_ir(&ir, &cfg(), &ScalarWeightExtractor);

        assert_eq!(compat.node_offsets, checked.node_offsets, "{label}");
        assert_eq!(compat.edge_offsets, checked.edge_offsets, "{label}");
        assert_eq!(compat.flat_node_edges, checked.flat_node_edges, "{label}");
        assert_eq!(compat.flat_edge_nodes, checked.flat_edge_nodes, "{label}");
        assert_eq!(compat.flat_node_sign, checked.flat_node_sign, "{label}");
        assert_eq!(compat.flat_edge_sign, checked.flat_edge_sign, "{label}");
        assert!(
            compat
                .flat_edge_w
                .iter()
                .zip(&checked.flat_edge_w)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "{label}: weights differ between the compatibility and checked paths"
        );
    }
}
