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

use hymeko::common::ids::DeclId;
use hymeko::ir::coherence::{CoherenceLimits, Finding, validate_hierarchy_index_coherence};
use hymeko::ir::ir::DeclKind;

use crate::unified_ir::support::builder::{S, TestIr};

pub fn label(f: &Finding) -> &'static str {
    match f {
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
    t.ir.decl_nodes[x.0].parent = DeclId::NONE; // path still says m.x
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
    t.ir.decl_nodes[a.0].next_sibling = a;
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
    t.ir.decl_nodes[a.0].parent = DeclId::new(3);
    assert!(has(&t, "ChildBacklinkMismatch"), "{:?}", findings(&t));
}

#[test]
fn mismatched_decl_to_map_is_detected() {
    let mut t = good();
    // claim a Node declaration is also an edge
    t.ir.decl_to_edge[1] = Some(hymeko::common::ids::EdgeId::new(0));
    let f = findings(&t);
    assert!(
        f.iter().any(|x| label(x) == "KindMapMismatch")
            || f.iter().any(|x| label(x) == "MultipleConcreteRecords"),
        "{f:?}"
    );
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
    t.ir.decl_to_node.pop();
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
