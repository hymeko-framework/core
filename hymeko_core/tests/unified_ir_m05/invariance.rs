//! Part C — M0.5 canonical-hash invariance.
//!
//! APPROVED-CORE-EDIT: unified-ir-m0-5-tests-and-benchmarks (2026-08-28).
//!
//! Each case builds two IRs that mean the same thing and were built
//! differently, proves the equivalence with the independent oracle
//! (`super::oracle`, which never calls the hash), and only then compares
//! `canonical_program_hash`.
//!
//! Run against the **unmodified** implementation. This phase does not fix
//! canonical hashing.

use hymeko::ir::canonical_hash::{CanonHashCfg, canonical_program_hash};
use hymeko::ir::hash::HashId;

use super::builder::{S, TestIr};
use super::oracle::{Ordering, dense_id_form, intrusive_hierarchy_form, semantic_form};

fn hash(t: &TestIr) -> HashId {
    canonical_program_hash(CanonHashCfg::default(), &t.idx, &t.ir, &t.it)
}

/// One comparison, reported in full so a failure is diagnosable from the
/// assertion message alone.
struct Verdict {
    case: &'static str,
    oracle_ordered_eq: bool,
    oracle_multiset_eq: bool,
    hash_eq: bool,
    dense_differs: bool,
}

impl Verdict {
    fn report(&self) -> String {
        format!(
            "[{}] oracle(Ordered)={} oracle(Multiset)={} hash={} dense_ids_differ={}",
            self.case,
            self.oracle_ordered_eq,
            self.oracle_multiset_eq,
            self.hash_eq,
            self.dense_differs
        )
    }
}

fn compare(case: &'static str, a: &TestIr, b: &TestIr) -> Verdict {
    let v = Verdict {
        case,
        oracle_ordered_eq: semantic_form(a, Ordering::Ordered)
            == semantic_form(b, Ordering::Ordered),
        oracle_multiset_eq: semantic_form(a, Ordering::Multiset)
            == semantic_form(b, Ordering::Multiset),
        hash_eq: hash(a) == hash(b),
        dense_differs: dense_id_form(a) != dense_id_form(b),
    };
    eprintln!("{}", v.report());
    v
}

/// Assert the pair is oracle-equivalent under the strict (Ordered) reading and
/// that the hash agrees. Used where the case is unambiguous.
fn assert_invariant(case: &'static str, a: &TestIr, b: &TestIr) {
    let v = compare(case, a, b);
    assert!(
        v.oracle_ordered_eq,
        "{} — the two IRs are NOT semantically equal under the oracle; the case \
         itself is malformed, not the hash.\nA:\n{}\nB:\n{}",
        case,
        semantic_form(a, Ordering::Ordered),
        semantic_form(b, Ordering::Ordered)
    );
    assert!(
        v.hash_eq,
        "{} — oracle says equivalent, canonical_program_hash disagrees. \
         COUNTEREXAMPLE.\nA dense: {}\nB dense: {}",
        case,
        dense_id_form(a),
        dense_id_form(b)
    );
}

// ---------------------------------------------------------------------------
// Shared shape: two nodes, one edge, one arc joining them.
// ---------------------------------------------------------------------------

/// Case 1 — declarations inserted forward versus reverse independent order.
#[test]
fn c1_forward_versus_reverse_declaration_order() {
    let mut a = TestIr::new();
    let na = a.node(&["m", "alpha"], hymeko::common::ids::DeclId::NONE);
    let nb = a.node(&["m", "beta"], hymeko::common::ids::DeclId::NONE);
    let ea = a.edge(&["m", "link"], hymeko::common::ids::DeclId::NONE);
    a.arc(ea, &[(na, S::Plus), (nb, S::Minus)]);

    let mut b = TestIr::new();
    // Reverse: beta first, so every dense id differs.
    let bb = b.node(&["m", "beta"], hymeko::common::ids::DeclId::NONE);
    let ba = b.node(&["m", "alpha"], hymeko::common::ids::DeclId::NONE);
    let eb = b.edge(&["m", "link"], hymeko::common::ids::DeclId::NONE);
    b.arc(eb, &[(ba, S::Plus), (bb, S::Minus)]);

    assert_invariant("C1 forward vs reverse decl order", &a, &b);
}

/// Case 2 — sibling insertion permutations under a shared parent.
#[test]
fn c2_sibling_permutation() {
    fn build(order: &[&str]) -> TestIr {
        let mut t = TestIr::new();
        let root = t.node(&["m"], hymeko::common::ids::DeclId::NONE);
        let mut ids = Vec::new();
        for name in order {
            ids.push((*name, t.node(&["m", name], root)));
        }
        let e = t.edge(&["m", "e"], root);
        // Reference in a fixed *semantic* order regardless of insertion order.
        let mut sorted: Vec<_> = ids.clone();
        sorted.sort_by_key(|(n, _)| *n);
        let refs: Vec<_> = sorted.iter().map(|&(_, d)| (d, S::Plus)).collect();
        t.arc(e, &refs);
        t
    }
    let a = build(&["x", "y", "z"]);
    let b = build(&["z", "x", "y"]);
    assert_invariant("C2 sibling permutation", &a, &b);
}

/// Case 3 — nodes and edges created in different interleavings.
#[test]
fn c3_node_edge_interleaving() {
    let mut a = TestIr::new();
    let a1 = a.node(&["p", "n1"], hymeko::common::ids::DeclId::NONE);
    let a2 = a.node(&["p", "n2"], hymeko::common::ids::DeclId::NONE);
    let ae1 = a.edge(&["p", "e1"], hymeko::common::ids::DeclId::NONE);
    let ae2 = a.edge(&["p", "e2"], hymeko::common::ids::DeclId::NONE);
    a.arc(ae1, &[(a1, S::Plus)]);
    a.arc(ae2, &[(a2, S::Neutral)]);

    let mut b = TestIr::new();
    // node, edge, node, edge — fully interleaved.
    let b1 = b.node(&["p", "n1"], hymeko::common::ids::DeclId::NONE);
    let be1 = b.edge(&["p", "e1"], hymeko::common::ids::DeclId::NONE);
    let b2 = b.node(&["p", "n2"], hymeko::common::ids::DeclId::NONE);
    let be2 = b.edge(&["p", "e2"], hymeko::common::ids::DeclId::NONE);
    b.arc(be1, &[(b1, S::Plus)]);
    b.arc(be2, &[(b2, S::Neutral)]);

    assert_invariant("C3 node/edge interleaving", &a, &b);
}

/// Case 4 — incidence insertion order.
///
/// **This is the ambiguous case** flagged in `oracle.rs`. The hash writes the
/// slot index into the incidence bytes, so permuting references within an arc
/// is *meaning-changing by the hash's definition*. The test therefore asserts
/// the observed contract rather than a preferred one, and records the open
/// language question.
#[test]
fn c4_incidence_slot_order_is_treated_as_semantic() {
    let mut a = TestIr::new();
    let x = a.node(&["q", "x"], hymeko::common::ids::DeclId::NONE);
    let y = a.node(&["q", "y"], hymeko::common::ids::DeclId::NONE);
    let e = a.edge(&["q", "e"], hymeko::common::ids::DeclId::NONE);
    a.arc(e, &[(x, S::Plus), (y, S::Plus)]);

    let mut b = TestIr::new();
    let bx = b.node(&["q", "x"], hymeko::common::ids::DeclId::NONE);
    let by = b.node(&["q", "y"], hymeko::common::ids::DeclId::NONE);
    let be = b.edge(&["q", "e"], hymeko::common::ids::DeclId::NONE);
    // Same references, opposite slots. Same sign, so the *bag* is identical.
    b.arc(be, &[(by, S::Plus), (bx, S::Plus)]);

    let v = compare("C4 incidence slot permutation", &a, &b);

    assert!(
        !v.oracle_ordered_eq,
        "C4: under the Ordered reading these should differ — the fixture is wrong"
    );
    assert!(
        v.oracle_multiset_eq,
        "C4: under the Multiset reading these must be equal — the fixture is wrong"
    );
    // The observed contract: the hash follows the Ordered reading.
    assert!(
        !v.hash_eq,
        "C4: the hash was expected to treat slot order as semantic (it writes \
         `slot as u16` into the incidence bytes). If this now passes, the hash \
         changed and the ambiguity in oracle.rs has been resolved toward \
         Multiset — update the documentation."
    );
}

/// Case 5 — identical paths, deliberately different dense ID assignment.
#[test]
fn c5_dense_id_assignment_does_not_affect_the_hash() {
    let mut a = TestIr::new();
    let ax = a.node(&["r", "x"], hymeko::common::ids::DeclId::NONE);
    let ay = a.node(&["r", "y"], hymeko::common::ids::DeclId::NONE);
    let ae = a.edge(&["r", "e"], hymeko::common::ids::DeclId::NONE);
    a.arc(ae, &[(ax, S::Plus), (ay, S::Minus)]);

    // Insert filler declarations first so every real decl lands at a different
    // index, then the same content on top.
    let mut b = TestIr::new();
    for i in 0..7 {
        b.node(
            &["zz", &format!("filler{i}")],
            hymeko::common::ids::DeclId::NONE,
        );
    }
    let bx = b.node(&["r", "x"], hymeko::common::ids::DeclId::NONE);
    let by = b.node(&["r", "y"], hymeko::common::ids::DeclId::NONE);
    let be = b.edge(&["r", "e"], hymeko::common::ids::DeclId::NONE);
    b.arc(be, &[(bx, S::Plus), (by, S::Minus)]);
    // Mirror the fillers into A so the two are semantically identical.
    let mut a2 = TestIr::new();
    for i in 0..7 {
        a2.node(
            &["zz", &format!("filler{i}")],
            hymeko::common::ids::DeclId::NONE,
        );
    }
    let _ = (ax, ay, ae);
    let a2x = a2.node(&["r", "x"], hymeko::common::ids::DeclId::NONE);
    let a2y = a2.node(&["r", "y"], hymeko::common::ids::DeclId::NONE);
    let a2e = a2.edge(&["r", "e"], hymeko::common::ids::DeclId::NONE);
    a2.arc(a2e, &[(a2x, S::Plus), (a2y, S::Minus)]);

    // Now shift B's indices by inserting fillers *after* instead of before.
    let mut c = TestIr::new();
    let cx = c.node(&["r", "x"], hymeko::common::ids::DeclId::NONE);
    let cy = c.node(&["r", "y"], hymeko::common::ids::DeclId::NONE);
    let ce = c.edge(&["r", "e"], hymeko::common::ids::DeclId::NONE);
    c.arc(ce, &[(cx, S::Plus), (cy, S::Minus)]);
    for i in 0..7 {
        c.node(
            &["zz", &format!("filler{i}")],
            hymeko::common::ids::DeclId::NONE,
        );
    }

    assert_invariant("C5 dense id shift", &a2, &c);
}

/// Case 6 (simulated) — add/remove/re-add history producing the same model.
///
/// **Limitation, stated:** `Ir` has no deletion operation, and the editor IR
/// that does live in the `transformation` repository, which `hymeko_core` does
/// not and must not depend on. So this simulates only the *observable residue*
/// of such a history — non-contiguous dense IDs for the surviving entities —
/// not the history itself. The editor-side case belongs in a
/// `transformation` test and is recorded as such in the M0.5 report.
#[test]
fn c6_simulated_remove_readd_residue() {
    // "Never deleted": three nodes in order.
    let mut a = TestIr::new();
    let a1 = a.node(&["h", "keep1"], hymeko::common::ids::DeclId::NONE);
    let a2 = a.node(&["h", "keep2"], hymeko::common::ids::DeclId::NONE);
    let ae = a.edge(&["h", "e"], hymeko::common::ids::DeclId::NONE);
    a.arc(ae, &[(a1, S::Plus), (a2, S::Plus)]);

    // "Churned": a doomed decl occupied index 0, survivors landed later.
    let mut b = TestIr::new();
    b.node(&["h", "doomed"], hymeko::common::ids::DeclId::NONE);
    let b1 = b.node(&["h", "keep1"], hymeko::common::ids::DeclId::NONE);
    let b2 = b.node(&["h", "keep2"], hymeko::common::ids::DeclId::NONE);
    let be = b.edge(&["h", "e"], hymeko::common::ids::DeclId::NONE);
    b.arc(be, &[(b1, S::Plus), (b2, S::Plus)]);
    // Simulate the removal: drop it from the index so it is not a live
    // declaration. The arena slot stays occupied, exactly as a tombstone would.
    let doomed_key = b
        .idx
        .by_path
        .iter()
        .find(|(pk, _)| pk.0.len() == 2 && b.it.resolve(pk.0[1]) == "doomed")
        .map(|(pk, _)| pk.clone())
        .expect("doomed present");
    b.idx.by_path.remove(&doomed_key);

    assert_invariant("C6 simulated remove/re-add residue", &a, &b);
}

/// Case 9 — serde round trip.
#[test]
fn c9_serde_round_trip_preserves_the_hash() {
    let mut a = TestIr::new();
    let x = a.node(&["s", "x"], hymeko::common::ids::DeclId::NONE);
    let y = a.node(&["s", "y"], hymeko::common::ids::DeclId::NONE);
    let e = a.edge(&["s", "e"], hymeko::common::ids::DeclId::NONE);
    a.tag(x, &["hot", "sensor"]);
    a.bases(y, &[(x, S::Plus)]);
    a.arc(e, &[(x, S::Plus), (y, S::Minus)]);

    // `Interner` is not `Clone`, so capture everything derived from `a`
    // before moving the interner into the reconstructed side.
    let before_form = semantic_form(&a, Ordering::Ordered);
    let before = hash(&a);
    let json = serde_json::to_string(&a.ir).expect("Ir serializes");
    let ir_back: hymeko::ir::ir::Ir = serde_json::from_str(&json).expect("Ir deserializes");

    let b = TestIr {
        ir: ir_back,
        idx: a.idx.clone(),
        it: a.it,
    };
    let after = hash(&b);

    assert_eq!(
        before_form,
        semantic_form(&b, Ordering::Ordered),
        "C9: serde round trip changed the semantic form"
    );
    assert_eq!(
        before, after,
        "C9: serde round trip changed the canonical hash"
    );
    eprintln!(
        "[C9 serde round trip] hash stable, json_bytes={}",
        json.len()
    );
}

/// Case 10 — reconstruction from scratch with a different build order.
#[test]
fn c10_reconstruction_without_semantic_change() {
    fn build(reverse_edges: bool) -> TestIr {
        let mut t = TestIr::new();
        let root = t.node(&["w"], hymeko::common::ids::DeclId::NONE);
        let n: Vec<_> = (0..4)
            .map(|i| t.node(&["w", &format!("n{i}")], root))
            .collect();
        let specs: Vec<(usize, usize)> = vec![(0, 1), (1, 2), (2, 3)];
        let iter: Vec<_> = if reverse_edges {
            specs.iter().rev().copied().collect()
        } else {
            specs.clone()
        };
        // Edge declarations are created in different orders, but each edge's
        // own arc content is fixed, so meaning is unchanged.
        let mut made = Vec::new();
        for (i, j) in iter {
            let e = t.edge(&["w", &format!("e{i}_{j}")], root);
            made.push((e, n[i], n[j]));
        }
        for (e, a, b) in made {
            t.arc(e, &[(a, S::Plus), (b, S::Minus)]);
        }
        t
    }
    assert_invariant("C10 reconstruction order", &build(false), &build(true));
}

/// The intrusive hierarchy is not read by the canonical hash.
///
/// Not one of the ten requested cases; it records a structural fact the audit
/// inferred from reading `canonical_hash.rs` and that M0.5 can now *prove*.
/// If this test ever fails, the hash started reading `DeclNode.parent` and the
/// migration's assumptions about hierarchy change.
#[test]
fn intrusive_parent_links_do_not_affect_the_canonical_hash() {
    let mut a = TestIr::new();
    let root = a.node(&["t"], hymeko::common::ids::DeclId::NONE);
    let x = a.node(&["t", "x"], root);
    let e = a.edge(&["t", "e"], root);
    a.arc(e, &[(x, S::Plus)]);

    let mut b = TestIr::new();
    let broot = b.node(&["t"], hymeko::common::ids::DeclId::NONE);
    // Deliberately wrong intrusive parent: NONE instead of the real root.
    let bx = b.node(&["t", "x"], hymeko::common::ids::DeclId::NONE);
    let be = b.edge(&["t", "e"], hymeko::common::ids::DeclId::NONE);
    b.arc(be, &[(bx, S::Plus)]);
    let _ = broot;

    assert_ne!(
        intrusive_hierarchy_form(&a),
        intrusive_hierarchy_form(&b),
        "fixture error: the intrusive hierarchies were meant to differ"
    );
    assert_eq!(
        hash(&a),
        hash(&b),
        "canonical hash became sensitive to DeclNode.parent — the audit's \
         finding that the skeleton is built from Index path prefixes no longer \
         holds, and the migration must be revisited"
    );
    eprintln!(
        "[intrusive-parent] hash identical despite differing intrusive hierarchy \
         — confirms the hash reads Index paths, not DeclNode.parent"
    );
}
