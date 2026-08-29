//! Part B — field-by-field hash coverage matrix, measured.
//!
//! APPROVED-CORE-EDIT: unified-ir-m0-6-hash-and-traversal (2026-08-28).
//!
//! Method: build a baseline IR, mutate **exactly one** semantic field, and
//! record which of the three identities notices:
//!
//! * `canonical_program_hash` (v1, `CanonHashCfg::default()`)
//! * `compute_merkle_hashes` → the `decl_hash` table, folded to one digest
//! * the independent semantic oracle from M0.5
//!
//! Nothing here asserts that a hash *ought* to cover a field. These are
//! **characterisation tests**: they record what the frozen v1 and the merkle
//! pass actually do, so the iterative rewrite in Part E has something to be
//! compared against, and so the v2 specification is grounded in measurement.
//!
//! Run against the unmodified implementations before any edit.

use blake3::Hasher;
use hymeko::common::ids::DeclId;
use hymeko::ir::canonical_hash::{CanonHashCfg, canonical_program_hash};
use hymeko::ir::hash::HashId;
use hymeko::ir::hash_pass::compute_merkle_hashes;
use hymeko::ir::ir::{AnnoR, SignedRefR, ValueR};

use crate::unified_ir::support::builder::{S, TestIr};
use crate::unified_ir::support::oracle::{Ordering, semantic_form};

/// All three identities of one IR.
pub struct Probe {
    pub v1: HashId,
    pub merkle: [u8; 32],
    pub oracle: String,
}

/// Fold the whole `decl_hash` table into one digest so a change anywhere is
/// visible. Missing entries contribute an explicit marker rather than nothing —
/// otherwise an absent hash and an all-zero hash would be indistinguishable.
fn fold_decl_hashes(t: &TestIr) -> [u8; 32] {
    let mut h = Hasher::new();
    for slot in &t.ir.decl_hash {
        match slot {
            Some(x) => {
                h.update(&[1]);
                h.update(&x.0);
            }
            None => {
                h.update(&[0]);
            }
        };
    }
    *h.finalize().as_bytes()
}

pub fn probe(t: &mut TestIr) -> Probe {
    compute_merkle_hashes(&mut t.ir, &t.it);
    Probe {
        v1: canonical_program_hash(CanonHashCfg::default(), &t.idx, &t.ir, &t.it),
        merkle: fold_decl_hashes(t),
        oracle: semantic_form(t, Ordering::Ordered),
    }
}

/// `(v1_changed, merkle_changed, oracle_changed)`
fn diff(a: &Probe, b: &Probe) -> (bool, bool, bool) {
    (a.v1 != b.v1, a.merkle != b.merkle, a.oracle != b.oracle)
}

/// Emit one matrix row and return the triple for assertion.
fn row(field: &str, a: &Probe, b: &Probe) -> (bool, bool, bool) {
    let (v1, mk, or) = diff(a, b);
    eprintln!(
        "[coverage] {field:<34} v1={:<5} merkle={:<5} oracle={:<5}",
        v1, mk, or
    );
    (v1, mk, or)
}

/// Baseline used by most rows: two nodes under a root, one edge, one arc.
fn base() -> TestIr {
    let mut t = TestIr::new();
    let root = t.node(&["m"], DeclId::NONE);
    let x = t.node(&["m", "x"], root);
    let y = t.node(&["m", "y"], root);
    let e = t.edge(&["m", "e"], root);
    t.arc(e, &[(x, S::Plus), (y, S::Minus)]);
    t
}

/// Wire the intrusive child list so `decl_children` (used by the merkle pass)
/// actually walks something. The M0.5 builder sets `parent` but not the
/// forward links; the merkle pass reads only the forward links.
fn link_children(t: &mut TestIr) {
    let n = t.ir.decl_nodes.len();
    for i in 0..n {
        t.ir.decl_nodes[i].first_child = DeclId::NONE;
        t.ir.decl_nodes[i].last_child = DeclId::NONE;
        t.ir.decl_nodes[i].next_sibling = DeclId::NONE;
    }
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
}

fn base_linked() -> TestIr {
    let mut t = base();
    t.attach_arc_decls();
    t.attach_arc_decls();
    link_children(&mut t);
    t
}

// ---------------------------------------------------------------------------
// The matrix.
// ---------------------------------------------------------------------------

#[test]
fn hash_coverage_matrix() {
    eprintln!("[coverage] field                              v1    merkle oracle");

    // --- declaration kind ---
    {
        let mut a = base_linked();
        let pa = probe(&mut a);
        let mut b = base_linked();
        // Turn node `y` into an edge declaration.
        let y = *b
            .idx
            .by_path
            .iter()
            .find_map(|(pk, d)| (pk.0.len() == 2 && b.it.resolve(pk.0[1]) == "y").then_some(d))
            .expect("y present");
        b.ir.decl_nodes[y.0].kind = hymeko::ir::ir::DeclKind::Edge;
        let pb = probe(&mut b);
        let (v1, mk, _) = row("decl.kind", &pa, &pb);
        assert!(v1, "v1 must cover declaration kind");
        assert!(mk, "merkle must cover declaration kind");
    }

    // --- declaration name / path ---
    {
        let mut a = base_linked();
        let pa = probe(&mut a);
        let mut b = TestIr::new();
        let root = b.node(&["m"], DeclId::NONE);
        let x = b.node(&["m", "RENAMED"], root);
        let y = b.node(&["m", "y"], root);
        let e = b.edge(&["m", "e"], root);
        b.arc(e, &[(x, S::Plus), (y, S::Minus)]);
        b.attach_arc_decls();
        link_children(&mut b);
        let pb = probe(&mut b);
        let (v1, mk, _) = row("decl.name / path", &pa, &pb);
        assert!(v1, "v1 must cover declaration name");
        assert!(mk, "merkle must cover declaration name");
    }

    // --- annotation tags ---
    {
        let mut a = base_linked();
        let pa = probe(&mut a);
        let mut b = base_linked();
        let x = *b
            .idx
            .by_path
            .iter()
            .find_map(|(pk, d)| (pk.0.len() == 2 && b.it.resolve(pk.0[1]) == "x").then_some(d))
            .expect("x present");
        b.tag(x, &["hot"]);
        let pb = probe(&mut b);
        row("anno.tags", &pa, &pb);
    }

    // --- annotation value ---
    {
        let mut a = base_linked();
        let pa = probe(&mut a);
        let mut b = base_linked();
        let x = *b
            .idx
            .by_path
            .iter()
            .find_map(|(pk, d)| (pk.0.len() == 2 && b.it.resolve(pk.0[1]) == "x").then_some(d))
            .expect("x present");
        b.ir.decl_nodes[x.0].anno.value = Some(ValueR::Num(1.5));
        let pb = probe(&mut b);
        row("anno.value", &pa, &pb);
    }

    // --- node bases (inheritance) ---
    {
        let mut a = base_linked();
        let pa = probe(&mut a);
        let mut b = base_linked();
        let (x, y) = {
            let f = |n: &str, b: &TestIr| {
                *b.idx
                    .by_path
                    .iter()
                    .find_map(|(pk, d)| {
                        (pk.0.len() == 2 && b.it.resolve(pk.0[1]) == n).then_some(d)
                    })
                    .expect("present")
            };
            (f("x", &b), f("y", &b))
        };
        b.bases(y, &[(x, S::Plus)]);
        let pb = probe(&mut b);
        row("node.bases", &pa, &pb);
    }

    // --- reference sign ---
    {
        let mut a = base_linked();
        let pa = probe(&mut a);
        let mut b = base_linked();
        if let SignedRefR::Minus(atom) = b.ir.arcs[0].refs[1].clone() {
            b.ir.arcs[0].refs[1] = SignedRefR::Plus(atom);
        }
        let pb = probe(&mut b);
        let (v1, mk, _) = row("incidence.sign", &pa, &pb);
        assert!(v1, "v1 must cover incidence sign");
        assert!(mk, "merkle must cover incidence sign");
    }

    // --- reference target ---
    {
        let mut a = base_linked();
        let pa = probe(&mut a);
        let mut b = base_linked();
        let x = *b
            .idx
            .by_path
            .iter()
            .find_map(|(pk, d)| (pk.0.len() == 2 && b.it.resolve(pk.0[1]) == "x").then_some(d))
            .expect("x present");
        // point the second reference at x as well (duplicate target)
        let atom = b.ir.arcs[0].refs[1].atom().clone();
        let mut atom2 = atom;
        atom2.target = x;
        b.ir.arcs[0].refs[1] = SignedRefR::Minus(atom2);
        let pb = probe(&mut b);
        let (v1, _, _) = row("incidence.target", &pa, &pb);
        assert!(v1, "v1 must cover incidence target");
    }

    // --- reference SLOT order (U1: semantic) ---
    {
        let mut a = base_linked();
        let pa = probe(&mut a);
        let mut b = base_linked();
        b.ir.arcs[0].refs.swap(0, 1);
        let pb = probe(&mut b);
        let (v1, _, _) = row("incidence.slot_order (U1)", &pa, &pb);
        assert!(
            v1,
            "U1: reference slot order is semantic and v1 must see it"
        );
    }

    // --- incidence weights ---
    {
        let mut a = base_linked();
        let pa = probe(&mut a);
        let mut b = base_linked();
        let atom = b.ir.arcs[0].refs[0].atom().clone();
        let mut atom2 = atom;
        atom2.weights = Some(vec![ValueR::Num(2.0)]);
        b.ir.arcs[0].refs[0] = SignedRefR::Plus(atom2);
        let pb = probe(&mut b);
        row("incidence.weights", &pa, &pb);
    }

    // --- incidence atom annotation ---
    {
        let mut a = base_linked();
        let pa = probe(&mut a);
        let mut b = base_linked();
        let atom = b.ir.arcs[0].refs[0].atom().clone();
        let mut atom2 = atom;
        atom2.anno = AnnoR {
            tags: vec![b.it.intern("marked")],
            value: None,
        };
        b.ir.arcs[0].refs[0] = SignedRefR::Plus(atom2);
        let pb = probe(&mut b);
        row("incidence.anno", &pa, &pb);
    }

    // --- duplicate incidence / multiplicity (U3) ---
    {
        let mut a = base_linked();
        let pa = probe(&mut a);
        let mut b = base_linked();
        let dup = b.ir.arcs[0].refs[0].clone();
        b.ir.arcs[0].refs.push(dup);
        let pb = probe(&mut b);
        let (v1, _, _) = row("incidence.multiplicity (U3)", &pa, &pb);
        assert!(v1, "U3: multiplicity is semantic and v1 must see it");
    }

    // --- intrusive parent link (not the path) ---
    {
        let mut a = base_linked();
        let pa = probe(&mut a);
        let mut b = base_linked();
        // Detach x from the intrusive tree, leaving its Index path intact.
        let x = *b
            .idx
            .by_path
            .iter()
            .find_map(|(pk, d)| (pk.0.len() == 2 && b.it.resolve(pk.0[1]) == "x").then_some(d))
            .expect("x present");
        b.ir.decl_nodes[x.0].parent = DeclId::NONE;
        b.attach_arc_decls();
        link_children(&mut b);
        let pb = probe(&mut b);
        row("decl.parent (intrusive only)", &pa, &pb);
    }

    // --- dense-ID permutation (M0.5 C5, re-confirmed here) ---
    {
        let mut a = base_linked();
        let pa = probe(&mut a);
        let mut b = TestIr::new();
        for i in 0..3 {
            b.node(&["zz", &format!("f{i}")], DeclId::NONE);
        }
        let root = b.node(&["m"], DeclId::NONE);
        let x = b.node(&["m", "x"], root);
        let y = b.node(&["m", "y"], root);
        let e = b.edge(&["m", "e"], root);
        b.arc(e, &[(x, S::Plus), (y, S::Minus)]);
        b.attach_arc_decls();
        link_children(&mut b);
        let pb = probe(&mut b);
        // Not asserted equal: b has extra declarations, so it is a different
        // program. Recorded for the matrix only.
        row("dense-id shift (+fillers)", &pa, &pb);
    }
}

// ---------------------------------------------------------------------------
// Part D — numeric determinism, measured through the hashes.
// ---------------------------------------------------------------------------

fn with_value(v: ValueR) -> TestIr {
    let mut t = TestIr::new();
    let root = t.node(&["m"], DeclId::NONE);
    let x = t.node(&["m", "x"], root);
    t.ir.decl_nodes[x.0].anno.value = Some(v);
    let e = t.edge(&["m", "e"], root);
    t.arc(e, &[(x, S::Plus)]);
    t.attach_arc_decls();
    link_children(&mut t);
    t
}

#[test]
fn numeric_special_values_are_distinguished_by_the_merkle_hash() {
    // U4: ValueR::Num preserves IEEE bit patterns, so these must not collide.
    let cases: Vec<(&str, ValueR, ValueR)> = vec![
        ("+0.0 vs -0.0", ValueR::Num(0.0_f64), ValueR::Num(-0.0_f64)),
        (
            "+Inf vs -Inf",
            ValueR::Num(f64::INFINITY),
            ValueR::Num(f64::NEG_INFINITY),
        ),
        (
            "NaN payload A vs B",
            ValueR::Num(f64::from_bits(0x7ff8_0000_0000_0001)),
            ValueR::Num(f64::from_bits(0x7ff8_0000_0000_0002)),
        ),
        (
            "list order [1,2] vs [2,1]",
            ValueR::List(vec![ValueR::Num(1.0), ValueR::Num(2.0)]),
            ValueR::List(vec![ValueR::Num(2.0), ValueR::Num(1.0)]),
        ),
    ];

    for (label, va, vb) in cases {
        let mut a = with_value(va);
        let mut b = with_value(vb);
        let pa = probe(&mut a);
        let pb = probe(&mut b);
        let (v1, mk, _) = diff(&pa, &pb);
        eprintln!("[numeric] {label:<28} v1={v1:<5} merkle={mk:<5}");
        assert!(
            mk,
            "U4 violated: `{label}` collided in the merkle hash — \
             ValueR::Num must preserve IEEE bit patterns"
        );
    }
}

#[test]
fn hash_bytes_are_stable_across_repeated_runs() {
    // Determinism, not coverage: the same IR must hash identically every time.
    let mut first: Option<(HashId, [u8; 32])> = None;
    for _ in 0..8 {
        let mut t = base_linked();
        let p = probe(&mut t);
        match &first {
            None => first = Some((p.v1, p.merkle)),
            Some((v1, mk)) => {
                assert_eq!(*v1, p.v1, "canonical_program_hash is not deterministic");
                assert_eq!(*mk, p.merkle, "compute_merkle_hashes is not deterministic");
            }
        }
    }
}
