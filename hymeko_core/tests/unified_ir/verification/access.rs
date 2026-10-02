//! Part D — accessor contract tests.
//!
//! APPROVED-CORE-EDIT: unified-ir-m2a-accessor-api (2026-08-28).
//!
//! Three concerns, kept apart: the accessors' own contract (absence, totality,
//! iteration), agreement with the direct field access M2b will replace, and
//! agreement with the pre-existing helpers that are *not* being replaced.

use hymeko::common::ids::{DeclId, EdgeId, HyperArcId, NodeId};
use hymeko::ir::coherence::{CoherenceLimits, validate_hierarchy_index_coherence};
use hymeko::ir::ir::{DeclKind, Ir};

use crate::unified_ir::support::builder::{S, TestIr};

/// Root `m` with two child nodes, one edge, one arc, and a nested grandchild.
fn fixture() -> TestIr {
    let mut t = TestIr::new();
    let root = t.node(&["m"], DeclId::NONE);
    let x = t.node(&["m", "x"], root);
    let y = t.node(&["m", "y"], root);
    let deep = t.node(&["m", "x", "deep"], x);
    let e = t.edge(&["m", "e"], root);
    t.arc(e, &[(x, S::Plus), (y, S::Minus), (deep, S::Neutral)]);
    t.attach_arc_decls();
    // wire the intrusive child list to match the paths
    let n = t.ir.decl_nodes.len();
    for i in 0..n {
        let p = t.ir.decl_nodes[i].parent;
        if p.is_none() {
            continue;
        }
        let child = DeclId::new(i);
        let last = t.ir.decl_nodes[p.raw()].last_child;
        if last.is_none() {
            t.ir.decl_nodes[p.raw()].first_child = child;
        } else {
            t.ir.decl_nodes[last.raw()].next_sibling = child;
        }
        t.ir.decl_nodes[p.raw()].last_child = child;
    }
    t
}

const HUGE: usize = usize::MAX - 1;

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

#[test]
fn lookup_first_middle_last_declaration() {
    let t = fixture();
    let n = t.ir.decl_count();
    assert!(n >= 3);
    for i in [0usize, n / 2, n - 1] {
        let d = DeclId::new(i);
        assert!(
            t.ir.decl_node(d).is_some(),
            "declaration {i} should resolve"
        );
        assert!(t.ir.contains_decl(d));
    }
}

#[test]
fn lookup_rejects_none_and_out_of_range() {
    let t = fixture();
    let n = t.ir.decl_count();

    // NONE
    assert!(t.ir.decl_node(DeclId::NONE).is_none());
    assert!(!t.ir.contains_decl(DeclId::NONE));
    assert!(t.ir.node(NodeId::NONE).is_none());
    assert!(t.ir.edge(EdgeId::NONE).is_none());
    assert!(t.ir.arc(HyperArcId::NONE).is_none());
    assert!(t.ir.kind_of(DeclId::NONE).is_none());
    assert!(t.ir.parent_of(DeclId::NONE).is_none());
    assert!(t.ir.first_child_of(DeclId::NONE).is_none());
    assert!(t.ir.next_sibling_of(DeclId::NONE).is_none());
    assert_eq!(t.ir.child_decls(DeclId::NONE).count(), 0);

    // exactly one past the end
    let past = DeclId::new(n);
    assert!(t.ir.decl_node(past).is_none());
    assert!(!t.ir.contains_decl(past));
    assert!(t.ir.kind_of(past).is_none());
    assert!(t.ir.parent_of(past).is_none());
    assert_eq!(t.ir.child_decls(past).count(), 0);

    // very large raw id (not the NONE sentinel)
    let huge = DeclId::new(HUGE);
    assert!(t.ir.decl_node(huge).is_none());
    assert!(!t.ir.contains_decl(huge));
    assert!(t.ir.kind_of(huge).is_none());
    assert!(t.ir.node(NodeId::new(HUGE)).is_none());
    assert!(t.ir.edge(EdgeId::new(HUGE)).is_none());
    assert!(t.ir.arc(HyperArcId::new(HUGE)).is_none());
}

#[test]
fn typed_record_lookup_returns_the_matching_record() {
    let t = fixture();
    for (nid, rec) in t.ir.node_records() {
        assert_eq!(
            t.ir.node(nid).map(|r| r.decl),
            Some(rec.decl),
            "node id must resolve back to the same record"
        );
    }
    for (eid, rec) in t.ir.edge_records() {
        assert_eq!(t.ir.edge(eid).map(|r| r.decl), Some(rec.decl));
    }
    for (aid, rec) in t.ir.arc_records() {
        assert_eq!(t.ir.arc(aid).map(|r| r.in_edge), Some(rec.in_edge));
    }
}

#[test]
fn wrong_id_domain_does_not_alias_across_tables() {
    // NodeId(0) and EdgeId(0) share a raw value; the typed APIs must not
    // confuse them. There is no way to *pass* a NodeId where an EdgeId is
    // expected (the compiler rejects it), so the property under test is that
    // the same raw ordinal resolves to genuinely different records.
    let t = fixture();
    let n0 = t.ir.node(NodeId::new(0)).expect("node 0 exists");
    let e0 = t.ir.edge(EdgeId::new(0)).expect("edge 0 exists");
    assert_ne!(
        n0.decl, e0.decl,
        "raw ordinal 0 must not name the same declaration in both tables"
    );
}

#[test]
fn declaration_to_record_mapping_agrees_with_backlinks() {
    let t = fixture();
    for (d, node) in t.ir.decls() {
        match node.kind {
            DeclKind::Node => {
                let (nid, rec) = t.ir.node_for_decl(d).expect("node decl resolves");
                assert_eq!(rec.decl, d, "record backlink must name the declaration");
                assert_eq!(t.ir.as_node(d), Some(nid), "id must match as_node");
                assert!(t.ir.edge_for_decl(d).is_none());
            }
            DeclKind::Edge => {
                let (eid, rec) = t.ir.edge_for_decl(d).expect("edge decl resolves");
                assert_eq!(rec.decl, d);
                assert_eq!(t.ir.as_edge(d), Some(eid));
                assert!(t.ir.node_for_decl(d).is_none());
            }
            DeclKind::HyperArc => {
                assert!(t.ir.node_for_decl(d).is_none());
                assert!(t.ir.edge_for_decl(d).is_none());
            }
        }
    }
}

#[test]
fn valid_declaration_without_the_requested_record_returns_none() {
    let t = fixture();
    // The root is a Node declaration; it has no edge or arc record.
    let root = DeclId::new(0);
    assert!(t.ir.node_for_decl(root).is_some());
    assert!(t.ir.edge_for_decl(root).is_none());
    assert!(t.ir.arc_for_decl(root).is_none());
}

#[test]
fn mapping_pointing_at_a_foreign_record_returns_none_rather_than_the_wrong_record() {
    // Part C: a malformed mapping must not yield a record belonging to another
    // declaration. This is the case that would otherwise be a silent wrong
    // answer rather than a visible absence.
    let mut t = fixture();
    let foreign = t.ir.as_node(DeclId::new(1)).expect("decl 1 is a node");
    // Corrupt a valid mapping by swapping its concrete records.
    let dx = t.node(&["extra"], DeclId::NONE);
    let own = t.ir.as_node(dx).unwrap();
    t.ir.nodes.swap(own.raw(), foreign.raw());

    assert_eq!(
        t.ir.as_node(dx),
        Some(own),
        "the raw mapping still reports the foreign id"
    );
    assert!(
        t.ir.node_for_decl(dx).is_none(),
        "the checked accessor must refuse a record whose backlink names another declaration"
    );
}

#[test]
fn mapping_pointing_out_of_range_returns_none_and_does_not_panic() {
    let mut t = fixture();
    // Truncating a record table after valid construction leaves a dangling mapping.
    let dx = t.node(&["extra"], DeclId::NONE);
    let out_of_table = t.ir.as_node(dx).unwrap();
    t.ir.nodes.pop(); // corrupt storage after a valid binding
    assert!(t.ir.node_for_decl(dx).is_none());
    assert!(t.ir.node(out_of_table).is_none());
    // Beyond the 62-bit dispatch domain the binding is refused at encode time,
    // never silently truncated or stored.
    assert!(t.ir.bind_node_decl(dx, NodeId::new(HUGE)).is_err());
}

// ---------------------------------------------------------------------------
// Iteration
// ---------------------------------------------------------------------------

#[test]
fn iteration_over_an_empty_ir_yields_nothing() {
    let ir = Ir::default();
    assert_eq!(ir.decls().count(), 0);
    assert_eq!(ir.node_records().count(), 0);
    assert_eq!(ir.edge_records().count(), 0);
    assert_eq!(ir.arc_records().count(), 0);
    assert_eq!(ir.decl_count(), 0);
}

#[test]
fn iteration_over_a_singleton() {
    let mut t = TestIr::new();
    t.node(&["solo"], DeclId::NONE);
    assert_eq!(t.ir.decls().count(), 1);
    assert_eq!(t.ir.node_records().count(), 1);
    assert_eq!(t.ir.edge_records().count(), 0);
}

#[test]
fn counts_equal_iterator_lengths() {
    let t = fixture();
    assert_eq!(t.ir.decl_count(), t.ir.decls().count());
    assert_eq!(t.ir.node_count(), t.ir.node_records().count());
    assert_eq!(t.ir.edge_count(), t.ir.edge_records().count());
    assert_eq!(t.ir.arc_count(), t.ir.arc_records().count());
}

#[test]
fn iterated_ids_resolve_back_to_the_same_record_with_no_gaps_or_duplicates() {
    let t = fixture();

    let mut seen: Vec<DeclId> = Vec::new();
    for (d, node) in t.ir.decls() {
        assert!(t.ir.contains_decl(d));
        assert_eq!(
            t.ir.decl_node(d).map(|n| n.kind),
            Some(node.kind),
            "iterated id must resolve to the iterated record"
        );
        assert!(!seen.contains(&d), "duplicate id from iterator");
        seen.push(d);
    }
    assert_eq!(
        seen.len(),
        t.ir.decl_count(),
        "iterator missed live entities"
    );

    let mut ns: Vec<NodeId> = Vec::new();
    for (nid, _) in t.ir.node_records() {
        assert!(!ns.contains(&nid));
        ns.push(nid);
    }
    assert_eq!(ns.len(), t.ir.node_count());
}

#[test]
fn repeated_iteration_is_deterministic() {
    let t = fixture();
    // Order is deterministic for one snapshot. This asserts *stability*, not
    // that the order carries meaning — see the module docs.
    let a: Vec<DeclId> = t.ir.decls().map(|(d, _)| d).collect();
    let b: Vec<DeclId> = t.ir.decls().map(|(d, _)| d).collect();
    assert_eq!(a, b);
    let c: Vec<NodeId> = t.ir.node_records().map(|(n, _)| n).collect();
    let d: Vec<NodeId> = t.ir.node_records().map(|(n, _)| n).collect();
    assert_eq!(c, d);
}

#[test]
fn mixed_declaration_kinds_are_all_visited() {
    let t = fixture();
    let mut nodes = 0;
    let mut edges = 0;
    for (_, decl) in t.ir.decls() {
        match decl.kind {
            DeclKind::Node => nodes += 1,
            DeclKind::Edge => edges += 1,
            DeclKind::HyperArc => {}
        }
    }
    assert_eq!(nodes, t.ir.node_count());
    assert_eq!(edges, t.ir.edge_count());
}

// ---------------------------------------------------------------------------
// Hierarchy
// ---------------------------------------------------------------------------

#[test]
fn hierarchy_root_nested_siblings_and_leaf() {
    let t = fixture();
    let root = DeclId::new(0);

    assert_eq!(t.ir.parent_of(root), None, "root has no parent");
    assert!(t.ir.first_child_of(root).is_some(), "root has children");

    let kids: Vec<DeclId> = t.ir.child_decls(root).collect();
    assert!(
        kids.len() >= 3,
        "root should have several children: {kids:?}"
    );
    for k in &kids {
        assert_eq!(
            t.ir.parent_of(*k),
            Some(root),
            "each child's parent must be the root"
        );
    }

    // nested: `m.x.deep` sits under `m.x`
    let x = kids[0];
    let deep: Vec<DeclId> = t.ir.child_decls(x).collect();
    assert_eq!(deep.len(), 1, "m.x has one child");
    assert_eq!(t.ir.parent_of(deep[0]), Some(x));

    // leaf
    assert_eq!(t.ir.first_child_of(deep[0]), None);
    assert_eq!(t.ir.child_decls(deep[0]).count(), 0);
}

#[test]
fn child_order_matches_the_existing_iterator() {
    let t = fixture();
    for (d, _) in t.ir.decls() {
        let new: Vec<DeclId> = t.ir.child_decls(d).collect();
        let old: Vec<DeclId> = t.ir.decl_children(d).collect();
        assert_eq!(new, old, "child order changed for {d:?}");
    }
}

#[test]
fn child_iteration_terminates_on_a_sibling_cycle() {
    // Every compatibility entry must use the same bounded iterator.
    let mut t = fixture();
    let root = DeclId::new(0);
    let first = t.ir.decl_nodes[root.raw()].first_child;
    t.ir.decl_nodes[first.raw()].next_sibling = first;
    let n = t.ir.decl_count();
    for count in [
        t.ir.child_decls(root).count(),
        t.ir.decl_children(root).count(),
        t.ir.children(root).count(),
    ] {
        assert!(count <= n, "walk was not bounded");
    }
    t.ir.decl_nodes[first.raw()].next_sibling = DeclId::new(HUGE);
    assert_eq!(t.ir.decl_children(root).collect::<Vec<_>>(), vec![first]);
    assert_eq!(t.ir.children(DeclId::new(HUGE)).count(), 0);
}

#[test]
fn last_child_of_agrees_with_the_final_child() {
    let t = fixture();
    for (d, _) in t.ir.decls() {
        let kids: Vec<DeclId> = t.ir.child_decls(d).collect();
        assert_eq!(t.ir.last_child_of(d), kids.last().copied());
    }
}

// ---------------------------------------------------------------------------
// Compatibility — accessors versus direct access and versus old helpers
// ---------------------------------------------------------------------------

/// `arc_for_decl` must NOT invent a back-link check.
///
/// Added after M2b fault injection: mutating `arc_for_decl` to filter on
/// `in_edge == id` — conflating the map-only and checked-composite contracts —
/// was caught by exactly one test, and that one was corpus-dependent. This is
/// the focused guard.
///
/// `ArcRec` has no own-declaration back-pointer: `in_edge` names the *owning
/// edge's* declaration. So `arc_for_decl` checks presence only, and must agree
/// with `as_arc` for every declaration.
#[test]
fn arc_for_decl_checks_presence_only_and_agrees_with_as_arc() {
    let t = fixture();
    let mut arcs_seen = 0usize;
    for (d, _) in t.ir.decls() {
        assert_eq!(
            t.ir.arc_for_decl(d).map(|(a, _)| a),
            t.ir.as_arc(d),
            "arc_for_decl must not filter beyond presence, at {d:?}"
        );
        if t.ir.as_arc(d).is_some() {
            arcs_seen += 1;
            let (_, rec) = t.ir.arc_for_decl(d).expect("mapping present");
            assert_ne!(
                rec.in_edge, d,
                "fixture precondition: in_edge names the owning edge, not the arc \
                 declaration — if these were equal the test could not distinguish \
                 a presence check from a back-link check"
            );
        }
    }
    assert!(
        arcs_seen > 0,
        "fixture must contain at least one arc mapping"
    );
}

/// The mirror guard: `node_for_decl` and `edge_for_decl` DO validate the
/// back-link, and must reject a foreign record even when the mapping resolves.
#[test]
fn node_and_edge_for_decl_validate_the_backlink_but_as_node_does_not() {
    let mut t = fixture();
    let foreign = t.ir.as_node(DeclId::new(1)).expect("decl 1 is a node");
    let dx = t.node(&["extra"], DeclId::NONE);
    let own = t.ir.as_node(dx).unwrap();
    t.ir.nodes.swap(own.raw(), foreign.raw());

    // map-only contract: reports the mapping verbatim
    assert_eq!(t.ir.as_node(dx), Some(own));
    // checked-composite contract: refuses it
    assert!(t.ir.node_for_decl(dx).is_none());
}
