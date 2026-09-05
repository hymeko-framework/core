//! **Control matrix for the semantic model digest (APPROVED-CORE-EDIT: semantic-model-digest-v2).**
//!
//! Two instantiations of one matrix:
//!
//! * `legacy_*` runs it against [`compute_merkle_hashes`] and records the three measured defects.
//!   These are characterization assertions: they document what the old pass does so it cannot drift
//!   silently while consumers migrate.
//! * `v2_*` runs it against [`semantic_model_digest_v2`] and asserts the requirements.
//!
//! Every mutation control first proves that it changed exactly the intended IR field
//! (`assert_changed`). A comparison over two identical IRs is rejected as vacuous.

use hymeko::common::ids::DeclId;
use hymeko::ir::canonical_hash::CanonHashCfg;
use hymeko::ir::hash_pass::compute_merkle_hashes;
use hymeko::ir::ir::{AnnoR, Ir, RefAtomR, SignedRefR, ValueR};
use hymeko::ir::semantic_digest::{
    ProgramStructureHashV1, SEMANTIC_DIGEST_ALGO_VERSION, SEMANTIC_DIGEST_SCHEMA_VERSION,
    SemanticModelDigestV2, program_structure_hash_v1, semantic_declaration_hashes,
    semantic_model_digest_v2,
};

use crate::unified_ir::support::builder::{S, TestIr};

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

/// An unweighted, unannotated signed reference to `target`.
///
/// The shared builder's `S::wrap` is private and this module needs to replace one reference in an
/// already-built arc, which no builder method does.
fn plain(sign: S, target: DeclId) -> SignedRefR {
    let atom = RefAtomR {
        target,
        anno: AnnoR::default(),
        weights: None,
    };
    match sign {
        S::Plus => SignedRefR::Plus(atom),
        S::Minus => SignedRefR::Minus(atom),
        S::Neutral => SignedRefR::Neutral(atom),
    }
}

/// Link `child` into `parent`'s intrusive chain.
///
/// The shared builder deliberately does not maintain first/next/last (see its `decl` docs), and the
/// Merkle pass folds children, so the legacy arm needs a linked hierarchy to be exercised at all.
fn link(t: &mut TestIr, parent: DeclId, child: DeclId) {
    if parent.is_none() {
        return;
    }
    let last = t.ir.decl_nodes[parent.raw()].last_child;
    if last.is_none() {
        t.ir.decl_nodes[parent.raw()].first_child = child;
    } else {
        t.ir.decl_nodes[last.raw()].next_sibling = child;
    }
    t.ir.decl_nodes[parent.raw()].last_child = child;
}

/// `m { a<alpha>, b<beta>, r, @e ( +a, -b ) }`.
///
/// `r`'s annotation value is the caller's, so two fixtures can differ in exactly that. `r` is
/// declared **after** `a` and `b`, which is the ordinary lowering order and therefore the case that
/// exposes D3.
fn program(r_value: Option<ValueR>) -> TestIr {
    let mut t = TestIr::new();
    let root = t.node(&["m"], DeclId::NONE);
    let a = t.node(&["m", "a"], root);
    t.tag(a, &["alpha"]);
    let b = t.node(&["m", "b"], root);
    t.tag(b, &["beta"]);
    let r = t.node(&["m", "r"], root);
    t.ir.decl_nodes[r.raw()].anno.value = r_value;
    let e = t.edge(&["m", "e"], root);
    t.arc(e, &[(a, S::Plus), (b, S::Minus)]);
    t.attach_arc_decls();
    for c in [a, b, r, e] {
        link(&mut t, root, c);
    }
    t
}

const A: DeclId = DeclId::new(1);
const B: DeclId = DeclId::new(2);
const R: DeclId = DeclId::new(3);

/// The legacy table, as a consumer would obtain it.
#[allow(deprecated)]
fn legacy(t: &mut TestIr) -> Vec<Option<[u8; 32]>> {
    compute_merkle_hashes(&mut t.ir, &t.it);
    t.ir.decl_hash.iter().map(|e| e.map(|x| x.0)).collect()
}

fn v2(t: &TestIr) -> SemanticModelDigestV2 {
    semantic_model_digest_v2(&t.ir, &t.it)
}

/// Observation floor: the mutation must have changed the `Ir`, or the comparison proves nothing.
fn assert_changed(left: &Ir, right: &Ir, what: &str) {
    let differs = left.decl_nodes.len() != right.decl_nodes.len()
        || left
            .decl_nodes
            .iter()
            .zip(&right.decl_nodes)
            .any(|(x, y)| x.anno != y.anno || x.kind != y.kind || x.name != y.name)
        || left.arcs.len() != right.arcs.len()
        || left
            .arcs
            .iter()
            .zip(&right.arcs)
            .any(|(x, y)| x.refs != y.refs || x.anno != y.anno);
    assert!(
        differs,
        "observation floor: `{what}` did not change the Ir, so the digest result is vacuous"
    );
}

// ---------------------------------------------------------------------------------------------
// Legacy characterization — the three defects, so they cannot drift silently
// ---------------------------------------------------------------------------------------------

/// **D1** — the legacy pass is not idempotent; it settles only from the second call on.
#[test]
fn legacy_merkle_pass_is_not_idempotent() {
    let mut t = program(Some(ValueR::Ref(A)));
    let passes: Vec<_> = (0..5).map(|_| legacy(&mut t)).collect();
    assert_ne!(passes[0], passes[1], "first call disagrees with the second");
    for k in 2..5 {
        assert_eq!(passes[1], passes[k], "it settles at the second call");
    }
}

/// **D2** — an incidence target change is invisible to the legacy first pass.
#[test]
fn legacy_first_pass_misses_an_incidence_target_change() {
    let mut l = program(None);
    let mut r = program(None);
    r.ir.arcs[0].refs[0] = plain(S::Plus, B);
    assert_changed(&l.ir, &r.ir, "incidence target +a -> +b");
    assert_eq!(legacy(&mut l), legacy(&mut r), "measured: invisible");
}

/// **D3** — a backward `ValueR::Ref` contributes nothing to the legacy first pass.
#[test]
fn legacy_first_pass_misses_a_backward_reference_target() {
    let mut l = program(Some(ValueR::Ref(A)));
    let mut r = program(Some(ValueR::Ref(B)));
    assert_changed(&l.ir, &r.ir, "backward ValueR::Ref(a) -> Ref(b)");
    assert_eq!(legacy(&mut l), legacy(&mut r), "measured: invisible");
}

// ---------------------------------------------------------------------------------------------
// V2 requirements
// ---------------------------------------------------------------------------------------------

#[test]
fn v2_is_idempotent_from_the_first_call() {
    let t = program(Some(ValueR::Ref(A)));
    let first = v2(&t);
    for _ in 0..5 {
        assert_eq!(v2(&t), first, "every call returns the same value");
    }
}

/// A stale, pre-populated `decl_hash` must not influence the result: V2 never reads it.
#[test]
fn v2_ignores_a_stale_decl_hash_table() {
    let clean = program(Some(ValueR::Ref(A)));
    let expected = v2(&clean);

    let mut dirty = program(Some(ValueR::Ref(A)));
    #[allow(deprecated)]
    compute_merkle_hashes(&mut dirty.ir, &dirty.it); // pollute the destination
    assert!(
        dirty.ir.decl_hash.iter().any(Option::is_some),
        "the pollution must be real, or the control is vacuous"
    );
    assert_eq!(v2(&dirty), expected, "a stale table changes nothing");

    // And it does not write one either.
    let fresh = program(Some(ValueR::Ref(A)));
    let before = fresh.ir.decl_hash.clone();
    let _ = v2(&fresh);
    assert_eq!(fresh.ir.decl_hash, before, "V2 writes no destination table");
}

#[test]
fn v2_moves_on_an_incidence_target_change() {
    let l = program(None);
    let mut r = program(None);
    r.ir.arcs[0].refs[0] = plain(S::Plus, B);
    assert_changed(&l.ir, &r.ir, "incidence target");
    assert_ne!(v2(&l), v2(&r));
}

#[test]
fn v2_moves_on_a_backward_reference_target() {
    let l = program(Some(ValueR::Ref(A)));
    let r = program(Some(ValueR::Ref(B)));
    assert_changed(&l.ir, &r.ir, "backward ValueR::Ref");
    assert_ne!(v2(&l), v2(&r));
}

#[test]
fn v2_moves_on_a_forward_reference_target() {
    // Referrer first: r = 1, a = 2, b = 3.
    let build = |to_a: bool| {
        let mut t = TestIr::new();
        let root = t.node(&["m"], DeclId::NONE);
        let r = t.node(&["m", "r"], root);
        let a = t.node(&["m", "a"], root);
        t.tag(a, &["alpha"]);
        let b = t.node(&["m", "b"], root);
        t.tag(b, &["beta"]);
        t.ir.decl_nodes[r.raw()].anno.value = Some(ValueR::Ref(if to_a { a } else { b }));
        for c in [r, a, b] {
            link(&mut t, root, c);
        }
        t
    };
    let (l, r) = (build(true), build(false));
    assert_changed(&l.ir, &r.ir, "forward ValueR::Ref");
    assert_ne!(v2(&l), v2(&r));
}

#[test]
fn v2_moves_on_a_nested_reference_target() {
    let nest = |t: DeclId| ValueR::List(vec![ValueR::Num(1.0), ValueR::Ref(t)]);
    let (l, r) = (program(Some(nest(A))), program(Some(nest(B))));
    assert_changed(&l.ir, &r.ir, "nested ValueR::Ref");
    assert_ne!(v2(&l), v2(&r));
}

/// A shared reference is stable: the same structure hashed twice is the same value, and
/// multiplicity is not collapsed.
#[test]
fn v2_is_stable_under_sharing_and_sensitive_to_multiplicity() {
    let shared = program(Some(ValueR::List(vec![ValueR::Ref(A), ValueR::Ref(A)])));
    let same = program(Some(ValueR::List(vec![ValueR::Ref(A), ValueR::Ref(A)])));
    assert_eq!(v2(&shared), v2(&same), "sharing is stable");

    let mixed = program(Some(ValueR::List(vec![ValueR::Ref(A), ValueR::Ref(B)])));
    assert_changed(&shared.ir, &mixed.ir, "shared reference multiplicity");
    assert_ne!(v2(&shared), v2(&mixed), "multiplicity is not collapsed");
}

/// A reference cycle must terminate and be deterministic.
#[test]
fn v2_terminates_deterministically_on_a_reference_cycle() {
    let mut t = program(None);
    // r -> a and a -> r: a cycle through annotation values.
    t.ir.decl_nodes[R.raw()].anno.value = Some(ValueR::Ref(A));
    t.ir.decl_nodes[A.raw()].anno.value = Some(ValueR::Ref(R));
    let first = v2(&t);
    assert_eq!(v2(&t), first, "deterministic on a cyclic reference graph");
}

/// Mutating the *content* of a referenced declaration must move the **model** digest, because every
/// declaration contributes — even though the reference itself carries only the target's identity.
#[test]
fn v2_moves_when_the_referenced_declarations_content_moves() {
    let l = program(Some(ValueR::Ref(A)));
    let mut r = program(Some(ValueR::Ref(A)));
    r.tag(A, &["alpha", "changed"]);
    assert_changed(&l.ir, &r.ir, "referenced declaration's tags");
    assert_ne!(v2(&l), v2(&r));
}

#[test]
fn v2_moves_on_annotation_tag_value_weight_sign_slot_and_target() {
    // tag
    let l = program(None);
    let mut r = program(None);
    r.tag(R, &["hot"]);
    assert_changed(&l.ir, &r.ir, "annotation tag");
    assert_ne!(v2(&l), v2(&r), "tag");

    // value
    let l = program(Some(ValueR::Num(1.0)));
    let r = program(Some(ValueR::Num(2.0)));
    assert_changed(&l.ir, &r.ir, "annotation value");
    assert_ne!(v2(&l), v2(&r), "value");

    // weight
    let l = program(None);
    let mut r = program(None);
    if let SignedRefR::Plus(atom) = &mut r.ir.arcs[0].refs[0] {
        atom.weights = Some(vec![ValueR::Num(2.0)]);
    } else {
        panic!("slot 0 is Plus");
    }
    assert_changed(&l.ir, &r.ir, "incidence weight");
    assert_ne!(v2(&l), v2(&r), "weight");

    // sign
    let l = program(None);
    let mut r = program(None);
    r.ir.arcs[0].refs[0] = plain(S::Minus, A);
    assert_changed(&l.ir, &r.ir, "incidence sign");
    assert_ne!(v2(&l), v2(&r), "sign");

    // slot order
    let l = program(None);
    let mut r = program(None);
    r.ir.arcs[0].refs.swap(0, 1);
    assert_changed(&l.ir, &r.ir, "slot order");
    assert_ne!(v2(&l), v2(&r), "slot");

    // reference-atom annotation
    let l = program(None);
    let mut r = program(None);
    let marked = r.it.intern("marked");
    if let SignedRefR::Plus(atom) = &mut r.ir.arcs[0].refs[0] {
        atom.anno.tags.push(marked);
    } else {
        panic!("slot 0 is Plus");
    }
    assert_changed(&l.ir, &r.ir, "reference-atom annotation");
    assert_ne!(v2(&l), v2(&r), "atom annotation");
}

/// Building the same program through a different *construction order* — same paths, same
/// hierarchy, same incidences — must not move the digest. This is the control that proves the
/// digest is about the program, not about how the builder was called.
#[test]
fn v2_is_invariant_under_a_semantically_irrelevant_construction_order() {
    // Reference: nodes declared a, b; edge last.
    let mut left = TestIr::new();
    {
        let root = left.node(&["m"], DeclId::NONE);
        let a = left.node(&["m", "a"], root);
        let b = left.node(&["m", "b"], root);
        let e = left.edge(&["m", "e"], root);
        left.arc(e, &[(a, S::Plus), (b, S::Minus)]);
        left.attach_arc_decls();
        for c in [a, b, e] {
            link(&mut left, root, c);
        }
    }
    // Same program, but `b` is interned before `a` so the string table order differs.
    let mut right = TestIr::new();
    {
        let root = right.node(&["m"], DeclId::NONE);
        let _warm = right.it.intern("b");
        let a = right.node(&["m", "a"], root);
        let b = right.node(&["m", "b"], root);
        let e = right.edge(&["m", "e"], root);
        right.arc(e, &[(a, S::Plus), (b, S::Minus)]);
        right.attach_arc_decls();
        for c in [a, b, e] {
            link(&mut right, root, c);
        }
    }
    assert_eq!(
        v2(&left),
        v2(&right),
        "interner order is not part of the program's meaning"
    );
}

// ---------------------------------------------------------------------------------------------
// V1 preservation and V1/V2 separation
// ---------------------------------------------------------------------------------------------

/// The v1 wrapper returns exactly what `canonical_program_hash` returns: no byte moves.
#[test]
fn program_structure_hash_v1_is_the_unchanged_v1_value() {
    let t = program(Some(ValueR::Ref(A)));
    let direct =
        hymeko::ir::canonical_hash::canonical_program_hash(CanonHashCfg::default(), &t.ir, &t.it);
    let wrapped: ProgramStructureHashV1 =
        program_structure_hash_v1(CanonHashCfg::default(), &t.ir, &t.it);
    assert_eq!(
        wrapped.0, direct,
        "the wrapper renames, it does not recompute"
    );
}

/// V1 and V2 cannot be confused: different domains and versions, therefore different values on the
/// same input, and different types at every call site.
#[test]
fn v1_and_v2_are_domain_and_version_separated() {
    let t = program(Some(ValueR::Ref(A)));
    let v1 = program_structure_hash_v1(CanonHashCfg::default(), &t.ir, &t.it);
    let v2v = semantic_model_digest_v2(&t.ir, &t.it);
    assert_ne!(
        v1.0.0, v2v.0.0,
        "a shared value would mean a shared domain header"
    );
    assert_eq!(SEMANTIC_DIGEST_SCHEMA_VERSION, 2);
    assert_eq!(SEMANTIC_DIGEST_ALGO_VERSION, 1);
}

/// V1's blind spot is unchanged: it still does not see annotations. Recorded so the two contracts
/// stay legible side by side.
#[test]
fn v1_still_does_not_cover_annotations_and_v2_does() {
    let l = program(None);
    let mut r = program(None);
    r.tag(R, &["hot"]);
    assert_changed(&l.ir, &r.ir, "annotation tag");
    assert_eq!(
        program_structure_hash_v1(CanonHashCfg::default(), &l.ir, &l.it).0,
        program_structure_hash_v1(CanonHashCfg::default(), &r.ir, &r.it).0,
        "v1's contract is structural: annotations are outside it"
    );
    assert_ne!(v2(&l), v2(&r), "v2's contract includes them");
}

/// The per-declaration table is returned, not written, and has one entry per declaration.
#[test]
fn semantic_declaration_hashes_covers_every_declaration() {
    let t = program(Some(ValueR::Ref(A)));
    let table = semantic_declaration_hashes(&t.ir, &t.it);
    assert_eq!(table.len(), t.ir.decl_nodes.len());
    let mut sorted: Vec<_> = table.iter().map(|h| h.0).collect();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        table.len(),
        "distinct declarations hash distinctly in this fixture"
    );
}
