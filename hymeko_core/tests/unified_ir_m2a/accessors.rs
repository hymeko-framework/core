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

use crate::unified_ir_m05::builder::{S, TestIr};

/// Root `m` with two child nodes, one edge, one arc, and a nested grandchild.
fn fixture() -> TestIr {
    let mut t = TestIr::new();
    let root = t.node(&["m"], DeclId::NONE);
    let x = t.node(&["m", "x"], root);
    let y = t.node(&["m", "y"], root);
    let deep = t.node(&["m", "x", "deep"], x);
    let e = t.edge(&["m", "e"], root);
    t.arc(e, &[(x, S::Plus), (y, S::Minus), (deep, S::Neutral)]);
    // wire the intrusive child list to match the paths
    let n = t.ir.decl_nodes.len();
    for i in 0..n {
        let p = t.ir.decl_nodes[i].parent;
        if p.is_none() {
            continue;
        }
        let child = DeclId::new(i);
        let last = t.ir.decl_nodes[p.0].last_child;
        if last.is_none() {
            t.ir.decl_nodes[p.0].first_child = child;
        } else {
            t.ir.decl_nodes[last.0].next_sibling = child;
        }
        t.ir.decl_nodes[p.0].last_child = child;
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
    let d0 = DeclId::new(0);
    let foreign = t.ir.as_node(DeclId::new(1)).expect("decl 1 is a node");
    t.ir.decl_to_node[d0.0] = Some(foreign);

    assert_eq!(
        t.ir.as_node(d0),
        Some(foreign),
        "the raw mapping still reports the foreign id"
    );
    assert!(
        t.ir.node_for_decl(d0).is_none(),
        "the checked accessor must refuse a record whose backlink names another declaration"
    );
}

#[test]
fn mapping_pointing_out_of_range_returns_none_and_does_not_panic() {
    let mut t = fixture();
    let d0 = DeclId::new(0);
    t.ir.decl_to_node[d0.0] = Some(NodeId::new(HUGE));
    assert!(t.ir.node_for_decl(d0).is_none());
    assert!(t.ir.node(NodeId::new(HUGE)).is_none());
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
    // The pre-existing `decl_children` would loop forever here; the total
    // iterator is bounded by the declaration count.
    let mut t = fixture();
    let root = DeclId::new(0);
    let first = t.ir.decl_nodes[root.0].first_child;
    t.ir.decl_nodes[first.0].next_sibling = first;
    let n = t.ir.decl_count();
    assert!(t.ir.child_decls(root).count() <= n, "walk was not bounded");
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

#[test]
fn accessors_agree_with_direct_field_access_on_the_whole_corpus() {
    use std::path::{Path, PathBuf};
    let root = Path::new("../data");
    if !root.exists() {
        eprintln!("[m2a] ../data absent — corpus comparison skipped");
        return;
    }
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "hymeko") {
                files.push(p);
            }
        }
    }
    files.sort();

    let mut checked = 0usize;
    let mut coherent = 0usize;
    for f in &files {
        let Ok((store, compiled)) = crate::test_helpers::load_and_lower(f) else {
            continue;
        };
        let ir = &compiled.ir;
        checked += 1;

        // Precondition for the comparison: the fixture is structurally sound.
        if validate_hierarchy_index_coherence(
            ir,
            &compiled.idx,
            &store.it,
            CoherenceLimits::default(),
        )
        .is_ok()
        {
            coherent += 1;
        }

        for i in 0..ir.decl_nodes.len() {
            let d = DeclId::new(i);

            // direct index vs accessor
            assert_eq!(ir.decl_node(d).map(|n| n.kind), Some(ir.decl_nodes[i].kind));
            assert_eq!(ir.kind_of(d), Some(ir.decl_nodes[i].kind));
            assert_eq!(ir.kind_of(d), Some(ir.kind(d)), "kind_of vs kind");

            // hierarchy: old (total-valued) vs new (Option-valued)
            let p = ir.parent(d);
            assert_eq!(ir.parent_of(d), p.is_some().then_some(p));
            let fc = ir.first_child(d);
            assert_eq!(ir.first_child_of(d), fc.is_some().then_some(fc));
            let ns = ir.next_sibling(d);
            assert_eq!(ir.next_sibling_of(d), ns.is_some().then_some(ns));

            // children: old vs new
            let old: Vec<DeclId> = ir.decl_children(d).collect();
            let new: Vec<DeclId> = ir.child_decls(d).collect();
            assert_eq!(old, new);

            // mapping: old id-only vs new id+record
            assert_eq!(ir.node_for_decl(d).map(|(n, _)| n), ir.as_node(d));
            assert_eq!(ir.edge_for_decl(d).map(|(e, _)| e), ir.as_edge(d));
            assert_eq!(ir.arc_for_decl(d).map(|(a, _)| a), ir.as_arc(d));
        }

        for (i, rec) in ir.nodes.iter().enumerate() {
            assert_eq!(ir.node(NodeId::new(i)).map(|r| r.decl), Some(rec.decl));
        }
        for (i, rec) in ir.edges.iter().enumerate() {
            assert_eq!(ir.edge(EdgeId::new(i)).map(|r| r.decl), Some(rec.decl));
        }
        for (i, rec) in ir.arcs.iter().enumerate() {
            assert_eq!(
                ir.arc(HyperArcId::new(i)).map(|r| r.in_edge),
                Some(rec.in_edge)
            );
        }

        assert_eq!(ir.decl_count(), ir.decl_nodes.len());
        assert_eq!(ir.node_count(), ir.nodes.len());
        assert_eq!(ir.edge_count(), ir.edges.len());
        assert_eq!(ir.arc_count(), ir.arcs.len());
    }

    eprintln!("[m2a] corpus comparison: {checked} fixtures, {coherent} coherent");
    assert!(checked > 0, "no fixture compared");
    assert_eq!(
        checked, coherent,
        "some fixture was not coherent; the accessor comparison assumes sound input"
    );
}
