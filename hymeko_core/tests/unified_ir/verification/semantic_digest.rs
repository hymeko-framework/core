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
use hymeko::ir::canonical_hash::{Canon, CanonHashCfg};
use hymeko::ir::hash_pass::compute_merkle_hashes;
use hymeko::ir::ir::{AnnoR, Ir, RefAtomR, SignedRefR, ValueR};
use hymeko::ir::semantic_digest::{
    ProgramStructureHashV1, SEMANTIC_DIGEST_ALGO_VERSION, SEMANTIC_DIGEST_SCHEMA_VERSION,
    SemanticModelDigestV2,
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
    let atom = RefAtomR::plain(target);
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
    let previous = std::mem::replace(&mut t.ir.decl_nodes[parent.raw()].last_child, child);
    let slot = if previous.is_none() {
        &mut t.ir.decl_nodes[parent.raw()].first_child
    } else {
        &mut t.ir.decl_nodes[previous.raw()].next_sibling
    };
    *slot = child;
}

fn link_children(t: &mut TestIr, parent: DeclId, children: &[DeclId]) {
    for &child in children {
        link(t, parent, child);
    }
}

/// Assign dense IDs in the requested order; callers retain control of linkage.
fn named_nodes<const N: usize>(
    names: [&str; N],
    preintern: Option<&str>,
) -> (TestIr, DeclId, [DeclId; N]) {
    let mut t = TestIr::new();
    if let Some(symbol) = preintern {
        t.it.intern(symbol);
    }
    let root = t.node(&["m"], DeclId::NONE);
    let nodes = names.map(|name| t.node(&["m", name], root));
    (t, root, nodes)
}

fn declare_anonymous_arc(
    t: &mut TestIr,
    edge: DeclId,
    arc: hymeko::common::ids::HyperArcId,
) -> DeclId {
    let declaration = t.ir.push_anonymous_arc();
    t.ir.decl_nodes[declaration.raw()].parent = edge;
    t.ir.bind_arc_decl(declaration, arc).unwrap();
    link(t, edge, declaration);
    declaration
}

fn nested_number(depth: usize, leaf: f64) -> ValueR {
    (0..depth).fold(ValueR::Num(leaf), |value, _| ValueR::List(vec![value]))
}

/// One edge and one record-only arc; anonymous dispatch remains caller-controlled.
fn signed_edge(
    t: &mut TestIr,
    root: DeclId,
    [plus, minus]: [DeclId; 2],
) -> (DeclId, hymeko::common::ids::HyperArcId) {
    let edge = t.edge(&["m", "e"], root);
    let arc = t.arc(edge, &[(plus, S::Plus), (minus, S::Minus)]);
    (edge, arc)
}

fn tagged_node(t: &mut TestIr, root: DeclId, name: &str, tags: &[&str]) -> DeclId {
    let node = t.node(&["m", name], root);
    t.tag(node, tags);
    node
}

fn declared_edge_fixture(mut t: TestIr, root: DeclId, [a, b]: [DeclId; 2]) -> TestIr {
    let (edge, _) = signed_edge(&mut t, root, [a, b]);
    t.attach_arc_decls();
    link_children(&mut t, root, &[a, b, edge]);
    t
}

/// `m { a<alpha>, b<beta>, r, @e ( +a, -b ) }`.
///
/// `r`'s annotation value is the caller's, so two fixtures can differ in exactly that. `r` is
/// declared **after** `a` and `b`, which is the ordinary lowering order and therefore the case that
/// exposes D3.
fn program(r_value: Option<ValueR>) -> TestIr {
    let (mut t, root, [a]) = named_nodes(["a"], None);
    t.tag(a, &["alpha"]);
    let b = tagged_node(&mut t, root, "b", &["beta"]);
    let r = t.node(&["m", "r"], root);
    t.ir.decl_nodes[r.raw()].anno.value = r_value;
    let (e, _) = signed_edge(&mut t, root, [a, b]);
    t.attach_arc_decls();
    link_children(&mut t, root, &[a, b, r, e]);
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
    Canon::new(&t.ir, &t.it).hash()
}

fn v1(t: &TestIr) -> ProgramStructureHashV1 {
    Canon::new(&t.ir, &t.it).structure(CanonHashCfg::default())
}

fn assert_distinct_models(left: &TestIr, right: &TestIr, changed: &str, message: &str) {
    assert_changed(&left.ir, &right.ir, changed);
    assert_ne!(v2(left), v2(right), "{message}");
}

fn assert_program_mutation(
    value: Option<ValueR>,
    changed: &str,
    message: &str,
    mutate: impl FnOnce(&mut TestIr),
) {
    let left = program(value.clone());
    let mut right = program(value);
    mutate(&mut right);
    assert_distinct_models(&left, &right, changed, message);
}

fn first_plus_atom(t: &mut TestIr) -> &mut RefAtomR {
    match &mut t.ir.arcs[0].refs[0] {
        SignedRefR::Plus(atom) => atom,
        _ => panic!("slot 0 is Plus"),
    }
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
    assert_program_mutation(None, "incidence target", "incidence target", |t| {
        t.ir.arcs[0].refs[0] = plain(S::Plus, B);
    });
}

#[test]
fn v2_moves_on_a_backward_reference_target() {
    let l = program(Some(ValueR::Ref(A)));
    let r = program(Some(ValueR::Ref(B)));
    assert_distinct_models(&l, &r, "backward ValueR::Ref", "backward ValueR::Ref");
}

#[test]
fn v2_moves_on_a_forward_reference_target() {
    // Referrer first: r = 1, a = 2, b = 3.
    let build = |to_a: bool| {
        let (mut t, root, [r, a]) = named_nodes(["r", "a"], None);
        t.tag(a, &["alpha"]);
        let b = tagged_node(&mut t, root, "b", &["beta"]);
        t.ir.decl_nodes[r.raw()].anno.value = Some(ValueR::Ref(if to_a { a } else { b }));
        link_children(&mut t, root, &[r, a, b]);
        t
    };
    let (l, r) = (build(true), build(false));
    assert_distinct_models(&l, &r, "forward ValueR::Ref", "forward ValueR::Ref");
}

#[test]
fn v2_moves_on_a_nested_reference_target() {
    let nest = |t: DeclId| ValueR::List(vec![ValueR::Num(1.0), ValueR::Ref(t)]);
    let (l, r) = (program(Some(nest(A))), program(Some(nest(B))));
    assert_distinct_models(&l, &r, "nested ValueR::Ref", "nested ValueR::Ref");
}

/// A shared reference is stable: the same structure hashed twice is the same value, and
/// multiplicity is not collapsed.
#[test]
fn v2_is_stable_under_sharing_and_sensitive_to_multiplicity() {
    let shared = program(Some(ValueR::List(vec![ValueR::Ref(A), ValueR::Ref(A)])));
    let same = program(Some(ValueR::List(vec![ValueR::Ref(A), ValueR::Ref(A)])));
    assert_eq!(v2(&shared), v2(&same), "sharing is stable");

    let mixed = program(Some(ValueR::List(vec![ValueR::Ref(A), ValueR::Ref(B)])));
    assert_distinct_models(
        &shared,
        &mixed,
        "shared reference multiplicity",
        "multiplicity is not collapsed",
    );
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
    assert_program_mutation(
        Some(ValueR::Ref(A)),
        "referenced declaration's tags",
        "referenced declaration's tags",
        |t| t.tag(A, &["alpha", "changed"]),
    );
}

#[test]
fn v2_moves_on_annotation_tag_value_weight_sign_slot_and_target() {
    assert_program_mutation(None, "annotation tag", "tag", |t| t.tag(R, &["hot"]));
    assert_distinct_models(
        &program(Some(ValueR::Num(1.0))),
        &program(Some(ValueR::Num(2.0))),
        "annotation value",
        "value",
    );
    assert_program_mutation(None, "incidence weight", "weight", |t| {
        first_plus_atom(t).weights = Some(vec![ValueR::Num(2.0)]);
    });
    assert_program_mutation(None, "incidence sign", "sign", |t| {
        t.ir.arcs[0].refs[0] = plain(S::Minus, A);
    });
    assert_program_mutation(None, "slot order", "slot", |t| t.ir.arcs[0].refs.swap(0, 1));
    assert_program_mutation(None, "reference-atom annotation", "atom annotation", |t| {
        let marked = t.it.intern("marked");
        first_plus_atom(t).anno.tags.push(marked);
    });
}

/// Building the same program through a different *construction order* — same paths, same
/// hierarchy, same incidences — must not move the digest. This is the control that proves the
/// digest is about the program, not about how the builder was called.
#[test]
fn v2_is_invariant_under_a_semantically_irrelevant_construction_order() {
    let build = |preintern: bool| {
        let mut t = TestIr::new();
        let root = t.node(&["m"], DeclId::NONE);
        // Preserve the original control: warm b after the root, before a.
        if preintern {
            t.it.intern("b");
        }
        let [a, b] = ["a", "b"].map(|name| t.node(&["m", name], root));
        declared_edge_fixture(t, root, [a, b])
    };
    let (left, right) = (build(false), build(true));
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
    let direct = Canon::new(&t.ir, &t.it)
        .structure(CanonHashCfg::default())
        .0;
    let wrapped: ProgramStructureHashV1 = v1(&t);
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
    let v1 = v1(&t);
    let v2v = Canon::new(&t.ir, &t.it).hash();
    assert_ne!(
        v1.0.0, v2v.0.0,
        "a shared value would mean a shared domain header"
    );
    assert_eq!(SEMANTIC_DIGEST_SCHEMA_VERSION, 2);
    // Algorithm 2 -> 3 on 2026-09-06 under APPROVED-CORE-EDIT: executable-model-contract. The
    // covered set grew: the optional model facet (role, shape, dtype, operator identity) is folded
    // into every declaration hash. The bump is safe because no artifact pins an absolute V2 value —
    // every assertion in this file is relative — while the v1 goldens are untouched because v1 does
    // not see the facet (`model_contract::v1_is_blind_to_the_model_facet`).
    assert_eq!(SEMANTIC_DIGEST_ALGO_VERSION, 3);
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
        v1(&l).0,
        v1(&r).0,
        "v1's contract is structural: annotations are outside it"
    );
    assert_ne!(v2(&l), v2(&r), "v2's contract includes them");
}

/// The per-declaration table is returned, not written, and has one entry per declaration.
#[test]
fn semantic_declaration_hashes_covers_every_declaration() {
    let t = program(Some(ValueR::Ref(A)));
    let table = Canon::new(&t.ir, &t.it).declaration_hashes();
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

// These controls reproduced failures on 112a254 before the correction.
// They remain permanent regressions for complete, canonical semantic identity.
#[test]
fn audit_v2_covers_validated_arcs_without_separate_declarations() {
    let build = |to_a: bool| {
        let (mut t, root, [a, b]) = named_nodes(["a", "b"], None);
        let (edge, _) = signed_edge(&mut t, root, [if to_a { a } else { b }, b]);
        link_children(&mut t, root, &[a, b, edge]);
        t
    };
    let (left, right) = (build(true), build(false));
    for t in [&left, &right] {
        assert_eq!(t.ir.arcs.len(), 1);
        assert!((0..t.ir.decl_nodes.len()).all(|i| t.ir.as_arc(DeclId::new(i)).is_none()));
        assert_cbor_valid_record_fixture(t);
    }
    assert_changed(&left.ir, &right.ir, "validated incidence target");
    assert_ne!(
        v1(&left),
        v1(&right),
        "V1 independently observes the changed incidence target"
    );
    assert_ne!(
        v2(&left),
        v2(&right),
        "V2 must cover every validated ArcRec"
    );
}

#[test]
fn audit_v2_distinguishes_values_at_depth_64() {
    let left = program(Some(nested_number(64, 1.0)));
    let right = program(Some(nested_number(64, 2.0)));
    assert_changed(&left.ir, &right.ir, "leaf at depth 64");
    assert_ne!(
        v2(&left),
        v2(&right),
        "different deep values are different models"
    );
}

#[test]
fn audit_v2_ignores_real_declaration_reindexing() {
    let build = |reverse: bool| {
        let names = if reverse { ["b", "a"] } else { ["a", "b"] };
        let (t, root, mut nodes) = named_nodes(names, None);
        if reverse {
            nodes.reverse();
        }
        let [a, b] = nodes;
        declared_edge_fixture(t, root, [a, b])
    };
    let (left, right) = (build(false), build(true));
    assert_ne!(left.path_of(DeclId::new(1)), right.path_of(DeclId::new(1)));
    assert_eq!(v1(&left), v1(&right), "the same named incidence structure");
    assert_eq!(
        v2(&left),
        v2(&right),
        "dense declaration IDs are storage choices"
    );
}

#[test]
fn audit_v2_ignores_arc_record_storage_order() {
    let build = |reverse: bool| {
        let (mut t, root, [a, b]) = named_nodes(["a", "b"], None);
        let [e, f] = ["e", "f"].map(|name| t.edge(&["m", name], root));
        let specifications = if reverse {
            [(f, b, a), (e, a, b)]
        } else {
            [(e, a, b), (f, b, a)]
        };
        let mut arcs = specifications
            .map(|(edge, plus, minus)| t.arc(edge, &[(plus, S::Plus), (minus, S::Minus)]));
        if reverse {
            arcs.reverse();
        }
        for (edge, aid) in [e, f].into_iter().zip(arcs) {
            declare_anonymous_arc(&mut t, edge, aid);
        }
        link_children(&mut t, root, &[a, b, e, f]);
        t
    };
    let (left, right) = (build(false), build(true));
    assert_ne!(left.ir.edges[0].arcs, right.ir.edges[0].arcs);
    assert_eq!(
        v1(&left),
        v1(&right),
        "each named edge retains its original ordered references"
    );
    assert_eq!(v2(&left), v2(&right), "ArcRec indices are storage choices");
}
// Additional controls beyond the four reproduced audit requirements.
// These use the existing test builder and production CBOR facade only.
#[test]
fn v2_anonymous_placeholder_is_independent_of_interner_allocation() {
    let build = |preintern: bool| {
        let (mut t, root, [a, b]) =
            named_nodes(["a", "b"], preintern.then_some("unrelated-first-symbol"));
        let (edge, arc) = signed_edge(&mut t, root, [a, b]);
        let declaration = declare_anonymous_arc(&mut t, edge, arc);
        link_children(&mut t, root, &[a, b, edge]);
        (t, declaration)
    };
    let ((left, ld), (right, rd)) = (build(false), build(true));
    assert_ne!(
        left.it.resolve(left.ir.decl_nodes[ld.raw()].name),
        right.it.resolve(right.ir.decl_nodes[rd.raw()].name)
    );
    assert_eq!(v2(&left), v2(&right));
}

#[test]
fn v2_distinguishes_sharing_of_identical_arc_payloads() {
    let build = |repeat_first: bool| {
        let (mut t, root, [a, b]) = named_nodes(["a", "b"], None);
        let (edge, first) = signed_edge(&mut t, root, [a, b]);
        let second = t.arc(edge, &[(a, S::Plus), (b, S::Minus)]);
        let eid = t.ir.as_edge(edge).unwrap();
        t.ir.edges[eid.raw()].arcs = if repeat_first {
            vec![first, first, second]
        } else {
            vec![first, second, second]
        };
        link_children(&mut t, root, &[a, b, edge]);
        t
    };
    let (left, right) = (build(true), build(false));
    assert_eq!(left.ir.arcs[0].refs, left.ir.arcs[1].refs);
    assert_eq!(left.ir.arcs[0].refs, right.ir.arcs[0].refs);
    assert_ne!(left.ir.edges[0].arcs, right.ir.edges[0].arcs);
    assert_ne!(v2(&left), v2(&right));
}

#[test]
fn v2_fallback_keeps_orphan_arc_payload_and_idempotence() {
    let mut left = program(None);
    let mut right = program(None);
    left.ir.edges[0].arcs.clear();
    right.ir.edges[0].arcs.clear();
    right.ir.arcs[0].refs[0] = plain(S::Plus, B);
    assert_distinct_models(&left, &right, "orphan arc target", "orphan arc target");
    assert_eq!(v2(&left), v2(&left));
    assert_eq!(v2(&right), v2(&right));
    assert_ne!(v2(&left), v2(&program(None)), "membership remains semantic");
}

#[test]
fn v2_values_remain_distinct_across_former_depth_boundary() {
    for depth in [0, 1, 63, 64, 65, 128, 256] {
        let left = program(Some(nested_number(depth, 1.0)));
        let right = program(Some(nested_number(depth, 2.0)));
        assert_distinct_models(&left, &right, "nested leaf", &format!("depth {depth}"));
    }
}

fn unbound_record_fixture(kind: hymeko::ir::ir::DeclKind, to_a: bool) -> TestIr {
    use hymeko::ir::ir::{DeclKind, EdgeRec, NodeRec};
    let (mut t, root, [a, b]) = named_nodes(["a", "b"], None);
    let unbound = t.decl(&["m", "unbound"], kind, root);
    let bases = vec![plain(S::Plus, if to_a { a } else { b })];
    match kind {
        DeclKind::Node => t.ir.nodes.push(NodeRec::new(unbound, bases)),
        DeclKind::Edge => t.ir.edges.push(EdgeRec::new(unbound, bases)),
        DeclKind::HyperArc => panic!("this fixture constructs a node or edge"),
    }
    assert!(t.ir.as_node(unbound).is_none());
    assert!(t.ir.as_edge(unbound).is_none());
    link_children(&mut t, root, &[a, b, unbound]);
    t
}

fn assert_cbor_valid_record_fixture(t: &TestIr) {
    let bytes = hymeko::writers::cbor_writer::encode("model.hymeko", &t.ir, &t.idx, &t.it, &[])
        .expect("unbound concrete record is accepted by the Core encoder");
    hymeko::writers::cbor_writer::decode(&bytes, &hymeko::ir_stream::DecodeLimits::DEFAULT)
        .expect("unbound concrete record is accepted by the Core decoder");
}

fn assert_unbound_record_change(
    kind: hymeko::ir::ir::DeclKind,
    bases: fn(&TestIr) -> &[SignedRefR],
) {
    let (left, right) = (
        unbound_record_fixture(kind, true),
        unbound_record_fixture(kind, false),
    );
    for t in [&left, &right] {
        assert_cbor_valid_record_fixture(t);
    }
    assert_ne!(bases(&left), bases(&right));
    assert_ne!(v2(&left), v2(&right));
}

#[test]
fn v2_covers_validated_unbound_node_record() {
    assert_unbound_record_change(hymeko::ir::ir::DeclKind::Node, |t| {
        &t.ir.nodes.last().unwrap().bases
    });
}

#[test]
fn v2_covers_validated_unbound_edge_record() {
    assert_unbound_record_change(hymeko::ir::ir::DeclKind::Edge, |t| {
        &t.ir.edges.last().unwrap().bases
    });
}

#[test]
fn v2_fallback_handles_unbound_and_aliased_anonymous_declarations() {
    let mut unbound = program(None);
    unbound.ir.push_anonymous_arc();
    assert_eq!(v2(&unbound), v2(&unbound));
    let mut aliased = program(None);
    let second = aliased.ir.push_anonymous_arc();
    aliased
        .ir
        .bind_arc_decl(second, hymeko::common::ids::HyperArcId::new(0))
        .unwrap();
    assert_eq!(v2(&aliased), v2(&aliased));
    assert_ne!(v2(&unbound), v2(&aliased), "the binding is observable");
}
