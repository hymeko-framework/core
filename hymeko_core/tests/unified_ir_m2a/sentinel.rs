//! M2a.1 — behaviour equivalence for the guard-free lookups.
//!
//! APPROVED-CORE-EDIT: unified-ir-m2a1-accessor-performance (2026-08-29).
//!
//! M2a.1 removed an explicit `if id.is_none() { return None; }` from every
//! `Option`-returning lookup, because `DeclId::NONE` is `usize::MAX` and a
//! `Vec` length can never reach it — the bounds check already rejects the
//! sentinel. These tests pin that reasoning to observable behaviour, so the
//! optimisation cannot silently change semantics for any id.
//!
//! The guards on `parent`, `first_child` and `next_sibling` were **kept**: those
//! index directly afterwards and would panic. That distinction is tested too.

use hymeko::common::ids::{DeclId, EdgeId, HyperArcId, NodeId};
use hymeko::ir::ir::Ir;

use crate::unified_ir_m05::builder::{S, TestIr};

fn fixture() -> TestIr {
    let mut t = TestIr::new();
    let root = t.node(&["m"], DeclId::NONE);
    let x = t.node(&["m", "x"], root);
    let y = t.node(&["m", "y"], root);
    let e = t.edge(&["m", "e"], root);
    t.arc(e, &[(x, S::Plus), (y, S::Minus)]);
    t
}

/// The representation fact the optimisation rests on.
#[test]
fn option_reference_is_niche_optimised() {
    use std::mem::size_of;
    assert_eq!(
        size_of::<&hymeko::ir::ir::DeclNode>(),
        size_of::<Option<&hymeko::ir::ir::DeclNode>>(),
        "Option<&DeclNode> must be niche-optimised; if this fails, the M2a \
         claim that Option carries no representation cost is wrong"
    );
    assert_eq!(
        size_of::<&hymeko::ir::ir::NodeRec>(),
        size_of::<Option<&hymeko::ir::ir::NodeRec>>()
    );
    assert_eq!(
        size_of::<&hymeko::ir::ir::EdgeRec>(),
        size_of::<Option<&hymeko::ir::ir::EdgeRec>>()
    );
    assert_eq!(
        size_of::<&hymeko::ir::ir::ArcRec>(),
        size_of::<Option<&hymeko::ir::ir::ArcRec>>()
    );
}

/// The other fact: the sentinel is out of range for any real container.
#[test]
fn the_none_sentinel_is_rejected_by_the_bounds_check_alone() {
    assert!(
        DeclId::NONE.raw() > isize::MAX as usize,
        "NONE must exceed any possible Vec length for the guard-free lookups \
         to be equivalent"
    );
    let empty: [u8; 0] = [];
    assert!(empty.get(DeclId::NONE.raw()).is_none());
    let some = [1u8, 2, 3];
    assert!(some.get(DeclId::NONE.raw()).is_none());
}

#[test]
fn guard_free_lookups_reject_none() {
    let t = fixture();
    assert!(t.ir.decl_node(DeclId::NONE).is_none());
    assert!(t.ir.as_node(DeclId::NONE).is_none());
    assert!(t.ir.as_edge(DeclId::NONE).is_none());
    assert!(t.ir.as_arc(DeclId::NONE).is_none());
    assert!(t.ir.node(NodeId::NONE).is_none());
    assert!(t.ir.edge(EdgeId::NONE).is_none());
    assert!(t.ir.arc(HyperArcId::NONE).is_none());
    assert!(t.ir.node_for_decl(DeclId::NONE).is_none());
    assert!(t.ir.edge_for_decl(DeclId::NONE).is_none());
    assert!(t.ir.arc_for_decl(DeclId::NONE).is_none());
    assert!(!t.ir.contains_decl(DeclId::NONE));
    assert!(!t.ir.contains_node(NodeId::NONE));
}

#[test]
fn guard_free_lookups_reject_exact_end_and_far_out_of_range() {
    let t = fixture();
    let n = t.ir.decl_count();
    for probe in [n, n + 1, usize::MAX - 1, usize::MAX / 2] {
        let d = DeclId::new(probe);
        assert!(t.ir.decl_node(d).is_none(), "decl_node({probe})");
        assert!(t.ir.as_node(d).is_none(), "as_node({probe})");
        assert!(t.ir.as_edge(d).is_none(), "as_edge({probe})");
        assert!(t.ir.as_arc(d).is_none(), "as_arc({probe})");
        assert!(t.ir.node_for_decl(d).is_none());
        assert!(!t.ir.contains_decl(d));
        assert!(t.ir.node(NodeId::new(probe)).is_none());
        assert!(t.ir.edge(EdgeId::new(probe)).is_none());
        assert!(t.ir.arc(HyperArcId::new(probe)).is_none());
    }
}

#[test]
fn guard_free_lookups_still_resolve_first_middle_and_last() {
    let t = fixture();
    let n = t.ir.decl_count();
    for i in [0usize, n / 2, n - 1] {
        let d = DeclId::new(i);
        assert!(t.ir.decl_node(d).is_some(), "declaration {i}");
        assert!(t.ir.contains_decl(d));
    }
    for i in 0..t.ir.node_count() {
        assert!(t.ir.node(NodeId::new(i)).is_some());
    }
    for i in 0..t.ir.edge_count() {
        assert!(t.ir.edge(EdgeId::new(i)).is_some());
    }
    for i in 0..t.ir.arc_count() {
        assert!(t.ir.arc(HyperArcId::new(i)).is_some());
    }
}

#[test]
fn an_empty_ir_rejects_every_id_including_zero() {
    let ir = Ir::default();
    for probe in [0usize, 1, usize::MAX - 1] {
        assert!(ir.decl_node(DeclId::new(probe)).is_none());
        assert!(ir.as_node(DeclId::new(probe)).is_none());
        assert!(ir.node(NodeId::new(probe)).is_none());
    }
    assert!(ir.decl_node(DeclId::NONE).is_none());
}

/// The guards that were deliberately **kept**: these index directly afterwards,
/// so removing their `NONE` check would turn absence into a panic.
#[test]
fn retained_guards_still_return_none_for_the_sentinel() {
    let t = fixture();
    assert!(t.ir.parent(DeclId::NONE).is_none());
    assert!(t.ir.first_child(DeclId::NONE).is_none());
    assert!(t.ir.next_sibling(DeclId::NONE).is_none());
    assert_eq!(t.ir.decl_children(DeclId::NONE).count(), 0);
}

/// Malformed mappings must still be refused rather than yielding a foreign
/// record — the optimisation must not have weakened back-link validation.
#[test]
fn backlink_validation_survives_the_optimisation() {
    let mut t = fixture();
    let d0 = DeclId::new(0);
    let foreign = t.ir.as_node(DeclId::new(1)).expect("decl 1 is a node");
    t.ir.decl_to_node[d0.0] = Some(foreign);
    assert_eq!(t.ir.as_node(d0), Some(foreign), "raw mapping unchanged");
    assert!(
        t.ir.node_for_decl(d0).is_none(),
        "back-link validation must still refuse a foreign record"
    );

    // A mapping pointing far out of range must also be refused, not panic.
    t.ir.decl_to_node[d0.0] = Some(NodeId::new(usize::MAX - 3));
    assert!(t.ir.node_for_decl(d0).is_none());
}

/// Hierarchy accessors remain total after the change.
#[test]
fn hierarchy_accessors_remain_total() {
    let t = fixture();
    for probe in [usize::MAX, usize::MAX - 1, t.ir.decl_count(), 0] {
        let d = DeclId::new(probe);
        // None of these may panic.
        let _ = t.ir.parent_of(d);
        let _ = t.ir.first_child_of(d);
        let _ = t.ir.last_child_of(d);
        let _ = t.ir.next_sibling_of(d);
        let _ = t.ir.kind_of(d);
        let _ = t.ir.child_decls(d).count();
    }
}

/// Equivalence against the pre-optimisation semantics, restated explicitly:
/// for every id, the guard-free form agrees with an explicitly guarded one.
#[test]
fn guard_free_agrees_with_an_explicitly_guarded_reference_implementation() {
    let t = fixture();
    let n = t.ir.decl_count();

    let guarded_decl = |id: DeclId| -> Option<&hymeko::ir::ir::DeclNode> {
        if id.is_none() {
            return None;
        }
        t.ir.decl_nodes.get(id.0)
    };
    let guarded_map = |id: DeclId| -> Option<NodeId> {
        if id.is_none() {
            return None;
        }
        t.ir.decl_to_node.get(id.0).copied().flatten()
    };

    let probes: Vec<usize> = (0..n)
        .chain([n, n + 1, usize::MAX - 1, usize::MAX])
        .collect();
    for p in probes {
        let d = DeclId::new(p);
        assert_eq!(
            t.ir.decl_node(d).map(|x| x.name),
            guarded_decl(d).map(|x| x.name),
            "decl_node disagrees with the guarded reference at {p}"
        );
        assert_eq!(
            t.ir.as_node(d),
            guarded_map(d),
            "as_node disagrees with the guarded reference at {p}"
        );
    }
}
