//! Reverse-incidence navigation V&V (APPROVED-CORE-EDIT: canonical-ir-incidence-navigation).
//!
//! Golden hypergraph shapes, an INDEPENDENT O(V*E) scan oracle (which does not
//! use the index), a metamorphic reachability check (edge-major scan vs the
//! reverse index), and a dangling-target fault. Everything goes through the real
//! Ir (`TestIr` builder -> canonical incidences).

use hymeko::common::ids::{DeclId, HyperArcId};
use hymeko::ir::incidence::{IncidenceError, build_incidence_index, index_matches_ir};
use hymeko::ir::ir::Ir;

use crate::unified_ir::support::builder::{S, TestIr};

/// Independent oracle: scan every arc in id order; push the arc once per ref that
/// targets `node`. Preserves duplicates and arc-id order — no index involved.
fn oracle_incident_arcs(ir: &Ir, node: DeclId) -> Vec<HyperArcId> {
    let mut out = Vec::new();
    for (ai, arc) in ir.arcs.iter().enumerate() {
        for sref in &arc.refs {
            if sref.target() == node {
                out.push(HyperArcId::new(ai));
            }
        }
    }
    out
}

fn assert_matches_oracle(t: &TestIr) {
    let idx = build_incidence_index(&t.ir).expect("well-formed index builds");
    assert!(index_matches_ir(&t.ir, &idx), "index disagrees with the Ir");
    for d in 0..t.ir.decl_nodes.len() {
        let did = DeclId::new(d);
        assert_eq!(
            idx.incident_arcs(did),
            oracle_incident_arcs(&t.ir, did).as_slice(),
            "incident_arcs disagrees with the scan oracle at decl {d}"
        );
    }
}

// --- golden shapes ----------------------------------------------------------

#[test]
fn multiway_hyperedge_with_all_three_signs() {
    let mut t = TestIr::new();
    let a = t.node(&["m", "a"], DeclId::NONE);
    let b = t.node(&["m", "b"], DeclId::NONE);
    let c = t.node(&["m", "c"], DeclId::NONE);
    let e = t.edge(&["m", "e"], DeclId::NONE);
    let arc = t.arc(e, &[(a, S::Plus), (b, S::Minus), (c, S::Neutral)]);
    let idx = build_incidence_index(&t.ir).unwrap();
    assert_eq!(idx.incident_arcs(a), &[arc]);
    assert_eq!(idx.incident_arcs(b), &[arc]);
    assert_eq!(idx.incident_arcs(c), &[arc]);
    assert_matches_oracle(&t);
}

#[test]
fn duplicate_incidence_is_preserved() {
    let mut t = TestIr::new();
    let a = t.node(&["d", "a"], DeclId::NONE);
    let e = t.edge(&["d", "e"], DeclId::NONE);
    let arc = t.arc(e, &[(a, S::Plus), (a, S::Plus)]); // node a referenced twice
    let idx = build_incidence_index(&t.ir).unwrap();
    assert_eq!(
        idx.incident_arcs(a),
        &[arc, arc],
        "duplicate incidence must survive"
    );
    assert_eq!(idx.degree(a), 2);
    assert_matches_oracle(&t);
}

#[test]
fn self_reference_and_multiple_arcs_per_edge() {
    let mut t = TestIr::new();
    let a = t.node(&["s", "a"], DeclId::NONE);
    let b = t.node(&["s", "b"], DeclId::NONE);
    let e = t.edge(&["s", "e"], DeclId::NONE);
    let a1 = t.arc(e, &[(a, S::Plus), (b, S::Minus)]);
    let a2 = t.arc(e, &[(a, S::Neutral)]); // same edge, second arc, self-ish
    let idx = build_incidence_index(&t.ir).unwrap();
    assert_eq!(idx.incident_arcs(a), &[a1, a2]); // ascending arc id
    assert_eq!(idx.incident_arcs(b), &[a1]);
    assert_matches_oracle(&t);
}

#[test]
fn same_node_in_multiple_hyperedges() {
    let mut t = TestIr::new();
    let hub = t.node(&["g", "hub"], DeclId::NONE);
    let x = t.node(&["g", "x"], DeclId::NONE);
    let y = t.node(&["g", "y"], DeclId::NONE);
    let e1 = t.edge(&["g", "e1"], DeclId::NONE);
    let e2 = t.edge(&["g", "e2"], DeclId::NONE);
    let a1 = t.arc(e1, &[(hub, S::Plus), (x, S::Plus)]);
    let a2 = t.arc(e2, &[(hub, S::Plus), (y, S::Plus)]);
    let idx = build_incidence_index(&t.ir).unwrap();
    assert_eq!(idx.incident_arcs(hub), &[a1, a2]);
    assert_matches_oracle(&t);
}

#[test]
fn empty_and_singleton_are_total() {
    let empty = Ir::default();
    let idx = build_incidence_index(&empty).unwrap();
    assert_eq!(idx.incidence_count(), 0);
    assert_eq!(idx.incident_arcs(DeclId::new(0)), &[] as &[HyperArcId]);

    let mut t = TestIr::new();
    let solo = t.node(&["solo"], DeclId::NONE);
    let idx = build_incidence_index(&t.ir).unwrap();
    assert_eq!(idx.incident_arcs(solo), &[] as &[HyperArcId]); // no incidences
}

// --- metamorphic: reverse-index reachability == edge-major scan reachability -

/// Berge-reachability via the reverse index (node -> incident arcs -> co-nodes).
fn reach_via_index(t: &TestIr, start: DeclId) -> Vec<bool> {
    let idx = build_incidence_index(&t.ir).unwrap();
    let n = t.ir.decl_nodes.len();
    let mut seen = vec![false; n];
    let mut q = vec![start];
    seen[start.raw()] = true;
    while let Some(node) = q.pop() {
        for &aid in idx.incident_arcs(node) {
            for sref in &t.ir.arcs[aid.raw()].refs {
                let ti = sref.target().raw();
                if !seen[ti] {
                    seen[ti] = true;
                    q.push(DeclId::new(ti));
                }
            }
        }
    }
    seen
}

/// The same, by scanning all arcs each step (no index) — the independent path.
fn reach_via_scan(t: &TestIr, start: DeclId) -> Vec<bool> {
    let n = t.ir.decl_nodes.len();
    let mut seen = vec![false; n];
    let mut q = vec![start];
    seen[start.raw()] = true;
    while let Some(node) = q.pop() {
        for arc in &t.ir.arcs {
            if arc.refs.iter().any(|r| r.target() == node) {
                for sref in &arc.refs {
                    let ti = sref.target().raw();
                    if !seen[ti] {
                        seen[ti] = true;
                        q.push(DeclId::new(ti));
                    }
                }
            }
        }
    }
    seen
}

#[test]
fn reverse_index_reachability_equals_edge_major_scan() {
    let mut t = TestIr::new();
    let n: Vec<DeclId> = (0..8)
        .map(|i| {
            t.node(
                &["r", Box::leak(format!("n{i}").into_boxed_str())],
                DeclId::NONE,
            )
        })
        .collect();
    let e = t.edge(&["r", "e"], DeclId::NONE);
    // two overlapping arcs -> one connected component {0,1,2,3,4}, {5,6} separate, 7 isolated
    t.arc(e, &[(n[0], S::Plus), (n[1], S::Minus), (n[2], S::Plus)]);
    t.arc(e, &[(n[2], S::Plus), (n[3], S::Minus), (n[4], S::Plus)]);
    t.arc(e, &[(n[5], S::Plus), (n[6], S::Minus)]);
    for &start in &n {
        assert_eq!(
            reach_via_index(&t, start),
            reach_via_scan(&t, start),
            "reachability diverged from start {}",
            start.raw()
        );
    }
}

// --- fault injection --------------------------------------------------------

#[test]
fn dangling_target_is_a_typed_error() {
    let mut t = TestIr::new();
    let a = t.node(&["f", "a"], DeclId::NONE);
    let e = t.edge(&["f", "e"], DeclId::NONE);
    // reference a target far out of range
    let bogus = DeclId::new(9_999);
    t.arc(e, &[(a, S::Plus), (bogus, S::Minus)]);
    let err = build_incidence_index(&t.ir).expect_err("dangling target must error");
    assert_eq!(
        err,
        IncidenceError::TargetOutOfRange {
            arc: HyperArcId::new(0),
            target: bogus,
        }
    );
}
