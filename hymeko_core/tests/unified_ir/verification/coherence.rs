//! Part F — hierarchy/`Index` coherence, exercised on real and corrupted input.
//!
//! APPROVED-CORE-EDIT: unified-ir-m0-6-hash-and-traversal (2026-08-28).
//!
//! The corpus run is the one that matters: U2 will make `parent`/`children`
//! canonical, so if current lowering already disagrees with the path `Index`,
//! that divergence has to be found now rather than during the migration.
//!
//! This phase reports divergence. It does not repair lowering.

use std::path::{Path, PathBuf};

use hymeko::common::ids::{DeclId, SymId};
use hymeko::common::pathkey::PathKey;
use hymeko::ir::coherence::{CoherenceLimits, Finding, validate_hierarchy_index_coherence};
use hymeko::ir::ir::DeclKind;

use crate::unified_ir::support::builder::{S, TestIr};

pub fn label(f: &Finding) -> &'static str {
    match f {
        Finding::InvalidSourceProvenance => "InvalidSourceProvenance",
        Finding::IndexedDeclOutOfRange { .. } => "IndexedDeclOutOfRange",
        Finding::DeclHasMultiplePaths { .. } => "DeclHasMultiplePaths",
        Finding::DeclMissingFromIndex { .. } => "DeclMissingFromIndex",
        Finding::ParentMismatch { .. } => "ParentMismatch",
        Finding::ChildBacklinkMismatch { .. } => "ChildBacklinkMismatch",
        Finding::SiblingChainBroken { .. } => "SiblingChainBroken",
        Finding::ParentCycle { .. } => "ParentCycle",
        Finding::KindMapMismatch { .. } => "KindMapMismatch",
        Finding::MultipleConcreteRecords { .. } => "MultipleConcreteRecords",
        Finding::RecordBacklinkMismatch { .. } => "RecordBacklinkMismatch",
        Finding::ReservedDispatchSlot { .. } => "ReservedDispatchSlot",
        Finding::ParallelArrayLengthMismatch { .. } => "ParallelArrayLengthMismatch",
    }
}

/// The corpus survey. Reports; does not gate on coherence, because divergence
/// is an expected finding this phase is forbidden to repair.

// ---------------------------------------------------------------------------
// Deliberate corruptions — each must be caught.
// ---------------------------------------------------------------------------

fn good() -> TestIr {
    let mut t = TestIr::new();
    let root = t.node(&["m"], DeclId::NONE);
    let x = t.node(&["m", "x"], root);
    let y = t.node(&["m", "y"], root);
    let e = t.edge(&["m", "e"], root);
    t.arc(e, &[(x, S::Plus), (y, S::Minus)]);
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

fn findings(t: &TestIr) -> Vec<Finding> {
    match validate_hierarchy_index_coherence(&t.ir, &t.idx, &t.it, CoherenceLimits::default()) {
        Ok(()) => Vec::new(),
        Err(r) => r.findings,
    }
}

fn has(t: &TestIr, want: &str) -> bool {
    findings(t).iter().any(|f| label(f) == want)
}

#[test]
fn a_coherent_ir_passes() {
    let t = good();
    let f = findings(&t);
    assert!(f.is_empty(), "coherent fixture reported: {f:?}");
}

#[test]
fn corrupted_parent_is_detected() {
    let mut t = good();
    let x = DeclId::new(1);
    t.ir.decl_nodes[x.raw()].parent = DeclId::NONE; // path still says m.x
    assert!(has(&t, "ParentMismatch"), "{:?}", findings(&t));
}

#[test]
fn parent_cycle_is_detected_and_terminates() {
    let mut t = good();
    // 1 -> 2 -> 1
    t.ir.decl_nodes[1].parent = DeclId::new(2);
    t.ir.decl_nodes[2].parent = DeclId::new(1);
    let f = findings(&t);
    assert!(
        f.iter().any(|x| label(x) == "ParentCycle"),
        "cycle not reported: {f:?}"
    );
}

#[test]
fn sibling_cycle_is_detected_and_terminates() {
    let mut t = good();
    let a = t.ir.decl_nodes[0].first_child;
    assert!(a.is_some());
    // make the first child point at itself
    t.ir.decl_nodes[a.raw()].next_sibling = a;
    let f = findings(&t);
    assert!(
        f.iter().any(|x| label(x) == "SiblingChainBroken"),
        "sibling cycle not reported: {f:?}"
    );
}

#[test]
fn missing_child_backlink_is_detected() {
    let mut t = good();
    let a = t.ir.decl_nodes[0].first_child;
    t.ir.decl_nodes[a.raw()].parent = DeclId::new(3);
    assert!(has(&t, "ChildBacklinkMismatch"), "{:?}", findings(&t));
}

#[test]
fn mismatched_decl_to_map_is_detected() {
    let mut t = good();
    let edge = t.ir.edges[0].decl;
    t.ir.decl_nodes[edge.raw()].kind = DeclKind::Node;
    let f = findings(&t);
    assert!(
        f.iter().any(|x| label(x) == "KindMapMismatch")
            || f.iter().any(|x| label(x) == "MultipleConcreteRecords"),
        "{f:?}"
    );
}

#[test]
fn edge_record_backlink_mismatch_is_detected() {
    let mut t = good();
    t.ir.edges[0].decl = DeclId::new(0);
    assert!(has(&t, "RecordBacklinkMismatch"), "{:?}", findings(&t));
}

#[test]
fn arc_record_out_of_range_is_detected() {
    let mut t = good();
    t.attach_arc_decls();
    t.ir.arcs.clear();
    assert!(has(&t, "RecordBacklinkMismatch"), "{:?}", findings(&t));
}

#[test]
fn record_backlink_mismatch_is_detected() {
    let mut t = good();
    // point the node record at the wrong declaration
    t.ir.nodes[0].decl = DeclId::new(3);
    assert!(has(&t, "RecordBacklinkMismatch"), "{:?}", findings(&t));
}

#[test]
fn index_path_disagreement_is_detected() {
    let mut t = good();
    // remove `m` from the index; `m.x` then has no indexed prefix parent
    let key = t
        .idx
        .by_path
        .keys()
        .find(|pk| pk.0.len() == 1)
        .cloned()
        .expect("root path present");
    t.idx.by_path.remove(&key);
    let f = findings(&t);
    assert!(
        f.iter()
            .any(|x| matches!(x, Finding::ParentMismatch { .. }))
            || f.iter().any(|x| matches!(
                x,
                Finding::DeclMissingFromIndex {
                    kind: DeclKind::Node,
                    ..
                }
            )),
        "{f:?}"
    );
}

#[test]
fn parallel_array_length_mismatch_is_detected() {
    let mut t = good();
    t.ir.decl_nodes.pop();
    assert!(has(&t, "ParallelArrayLengthMismatch"), "{:?}", findings(&t));
}

#[test]
fn findings_are_bounded_and_deterministic() {
    let mut t = good();
    for i in 0..t.ir.decl_nodes.len() {
        t.ir.decl_nodes[i].parent = DeclId::new((i + 1) % t.ir.decl_nodes.len());
    }
    let limits = CoherenceLimits {
        max_findings: 3,
        ..CoherenceLimits::default()
    };
    let a = validate_hierarchy_index_coherence(&t.ir, &t.idx, &t.it, limits);
    let b = validate_hierarchy_index_coherence(&t.ir, &t.idx, &t.it, limits);
    let (ra, rb) = (a.unwrap_err(), b.unwrap_err());
    assert!(ra.findings.len() <= 3, "cap not honoured");
    assert!(ra.truncated, "truncation not reported");
    assert_eq!(
        ra.findings, rb.findings,
        "finding order is not deterministic"
    );
}

#[test]
fn index_findings_keep_exact_order_and_caps_across_insertion_orders() {
    let mut t = good();
    let m = t.it.get_id("m").unwrap();
    let x = t.it.get_id("x").unwrap();
    let extra = t.it.intern("extra");
    let other = t.it.intern("other");
    let mut entries: Vec<_> = t.idx.by_path.iter().map(|(p, d)| (p.clone(), *d)).collect();
    entries.extend([
        (PathKey(vec![m, x, extra]), DeclId::new(1)),
        (PathKey(vec![other, x]), DeclId::new(1)),
        (PathKey(vec![extra]), DeclId::new(900)),
        (PathKey(vec![other]), DeclId::new(800)),
    ]);
    let expected = vec![
        Finding::DeclHasMultiplePaths {
            decl: DeclId::new(1),
        },
        Finding::DeclHasMultiplePaths {
            decl: DeclId::new(1),
        },
        Finding::IndexedDeclOutOfRange {
            decl: DeclId::new(800),
            decl_count: t.ir.decl_nodes.len(),
        },
        Finding::IndexedDeclOutOfRange {
            decl: DeclId::new(900),
            decl_count: t.ir.decl_nodes.len(),
        },
    ];
    for _ in 0..2 {
        t.idx.by_path.clear();
        t.idx.by_path.extend(entries.iter().cloned());
        for max_findings in [0, 2, 64] {
            let report = validate_hierarchy_index_coherence(
                &t.ir,
                &t.idx,
                &t.it,
                CoherenceLimits {
                    max_findings,
                    max_walk: 16,
                },
            )
            .unwrap_err();
            assert_eq!(report.total, expected.len());
            assert_eq!(report.truncated, max_findings < expected.len());
            assert_eq!(
                report.findings,
                expected[..max_findings.min(expected.len())]
            );
        }
        entries.reverse();
    }
}

#[test]
fn parent_lookup_uses_nearest_indexed_prefix_including_empty_root() {
    let mut t = good();
    let m = t.it.get_id("m").unwrap();
    let x = t.it.get_id("x").unwrap();
    let gap = t.it.intern("unindexed");
    t.idx.by_path.remove(&[m][..]);
    t.idx.by_path.insert(PathKey(vec![]), DeclId::new(0));
    t.idx.by_path.remove(&[m, x][..]);
    t.idx
        .by_path
        .insert(PathKey(vec![gap, gap, x]), DeclId::new(1));
    assert!(findings(&t).is_empty());
}

#[test]
#[cfg(target_pointer_width = "64")]
fn distinct_wide_symbols_do_not_alias_during_parent_lookup() {
    let mut t = good();
    let m = t.it.get_id("m").unwrap();
    let x = t.it.get_id("x").unwrap();
    t.idx.by_path.remove(&[m, x][..]);
    t.idx.by_path.insert(
        PathKey(vec![SymId::new(m.raw() + (1usize << 32)), x]),
        DeclId::new(1),
    );
    // This validator compares hierarchy with paths, not interner membership.
    // The fabricated prefix must not match `m` through integer truncation.
    assert_eq!(
        findings(&t),
        vec![Finding::ParentMismatch {
            decl: DeclId::new(1),
            by_path: DeclId::NONE,
            by_link: DeclId::new(0),
        }]
    );
}
