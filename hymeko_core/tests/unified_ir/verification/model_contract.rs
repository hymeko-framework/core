//! **Acceptance gates for the optional executable-model contract (Tier 2A).**
//!
//! APPROVED-CORE-EDIT: executable-model-contract (2026-09-06).
//!
//! The contract is a *facet*, not a widening of the universal declaration: most HyMeKo documents
//! describe service networks, robotics meta-graphs or P-graphs and carry no facet at all. These
//! tests therefore split into three groups: the non-model document must be completely unaffected,
//! a model document must have its typed fields owned, validated and digest-covered, and the
//! boundary between the two must be explicit.

use hymeko::common::ids::DeclId;
use hymeko::ir::canonical_hash::{Canon, CanonHashCfg};
use hymeko::ir::coherence::{ModelFinding, validate_model_contract};
use hymeko::ir::ir::{
    DType, DeclKind, Extent, ModelFacet, OperatorContract, TensorContract, TensorRole,
};

use crate::unified_ir::support::builder::{S, TestIr};

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

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

fn tensor(role: TensorRole, shape: &[u64], dtype: DType) -> ModelFacet {
    ModelFacet::Tensor(TensorContract {
        role,
        shape: shape.iter().copied().map(Extent::Static).collect(),
        dtype,
    })
}

/// `m { x, W, y, @op(+y, -x, -W) }` with no facet: the ordinary, non-model document.
fn plain() -> (TestIr, [DeclId; 4]) {
    let mut t = TestIr::new();
    let root = t.node(&["m"], DeclId::NONE);
    let x = t.node(&["m", "x"], root);
    let w = t.node(&["m", "W"], root);
    let y = t.node(&["m", "y"], root);
    let e = t.edge(&["m", "op"], root);
    t.arc(e, &[(y, S::Plus), (x, S::Minus), (w, S::Minus)]);
    t.attach_arc_decls();
    for c in [x, w, y, e] {
        link(&mut t, root, c);
    }
    (t, [x, w, y, e])
}

/// The same program, declared as an executable model.
fn model() -> (TestIr, [DeclId; 4]) {
    let (mut t, [x, w, y, e]) = plain();
    t.ir.set_model_facet(x, Some(tensor(TensorRole::Input, &[1, 4], DType::F32)));
    t.ir.set_model_facet(w, Some(tensor(TensorRole::Parameter, &[4, 3], DType::F32)));
    t.ir.set_model_facet(y, Some(tensor(TensorRole::Output, &[1, 3], DType::F32)));
    let id = t.it.intern("linear");
    t.ir.set_model_facet(
        e,
        Some(ModelFacet::Operator(OperatorContract { identity: id })),
    );
    (t, [x, w, y, e])
}

fn digest(t: &TestIr) -> [u8; 32] {
    Canon::new(&t.ir, &t.it).hash().0.0
}

fn v1(t: &TestIr) -> [u8; 32] {
    Canon::new(&t.ir, &t.it)
        .structure(CanonHashCfg::default())
        .0
        .0
}

fn roundtrip(t: &TestIr) -> hymeko::ir::ir::Ir {
    let bytes = serde_json::to_vec(&t.ir).expect("serialize");
    serde_json::from_slice(&bytes).expect("deserialize")
}

// ---------------------------------------------------------------------------------------------
// 1. The non-model document is untouched
// ---------------------------------------------------------------------------------------------

/// A document with no facet is not an executable model, and the validator says so by doing nothing.
#[test]
fn a_document_without_a_facet_is_not_a_model_and_is_not_validated() {
    let (t, _) = plain();
    assert!(!t.ir.declares_executable_model());
    assert_eq!(t.ir.model_facet_table_len(), 0, "the table stays empty");
    assert_eq!(validate_model_contract(&t.ir, &t.it), Ok(()));
}

/// Its wire is unchanged: no `decl_model` field is emitted at all.
#[test]
fn a_document_without_a_facet_emits_no_facet_field() {
    let (t, _) = plain();
    let json = serde_json::to_string(&t.ir).expect("serialize");
    assert!(
        !json.contains("decl_model"),
        "the pre-facet wire must be byte-identical, so the field must be absent"
    );
    let back = roundtrip(&t);
    assert!(!back.declares_executable_model());
    assert_eq!(back.model_facet_table_len(), 0);
}

/// A payload written before the facet existed decodes to "no declaration is a model element".
#[test]
fn a_pre_facet_payload_decodes_as_no_model() {
    let (t, _) = plain();
    let mut json: serde_json::Value =
        serde_json::from_slice(&serde_json::to_vec(&t.ir).expect("serialize")).expect("parse");
    // Belt and braces: even if a future writer emits the field, removing it must still decode.
    json.as_object_mut().expect("object").remove("decl_model");
    let back: hymeko::ir::ir::Ir = serde_json::from_value(json).expect("legacy payload decodes");
    assert!(!back.declares_executable_model());
    assert_eq!(digest(&t), Canon::new(&back, &t.it).hash().0.0);
}

// ---------------------------------------------------------------------------------------------
// 2. The model document: ownership, validation, digest coverage, round-trip
// ---------------------------------------------------------------------------------------------

#[test]
fn a_well_formed_model_validates() {
    let (t, _) = model();
    assert!(t.ir.declares_executable_model());
    assert_eq!(validate_model_contract(&t.ir, &t.it), Ok(()));
}

/// Every typed field is recovered from the `Ir` alone — this is the ownership claim.
#[test]
fn the_typed_contract_is_recovered_from_the_ir_alone() {
    let (t, [x, w, y, e]) = model();
    match t.ir.model_facet(x) {
        Some(ModelFacet::Tensor(c)) => {
            assert_eq!(c.role, TensorRole::Input);
            assert_eq!(c.dtype, DType::F32);
            assert_eq!(c.shape, vec![Extent::Static(1), Extent::Static(4)]);
        }
        other => panic!("x must be an input tensor, got {other:?}"),
    }
    assert!(matches!(
        t.ir.model_facet(w),
        Some(ModelFacet::Tensor(c)) if c.role == TensorRole::Parameter
    ));
    assert!(matches!(
        t.ir.model_facet(y),
        Some(ModelFacet::Tensor(c)) if c.role == TensorRole::Output
    ));
    match t.ir.model_facet(e) {
        Some(ModelFacet::Operator(o)) => assert_eq!(t.it.resolve(o.identity), "linear"),
        other => panic!("op must be an operator, got {other:?}"),
    }
}

/// serialize -> parse preserves the typed contract **and** the digest.
#[test]
fn the_round_trip_preserves_the_typed_contract_and_the_digest() {
    let (t, [x, _, _, e]) = model();
    let back = roundtrip(&t);
    assert_eq!(back.model_facet(x), t.ir.model_facet(x));
    assert_eq!(back.model_facet(e), t.ir.model_facet(e));
    assert_eq!(
        Canon::new(&back, &t.it).hash().0.0,
        digest(&t),
        "the digest must survive the wire"
    );
    assert_eq!(validate_model_contract(&back, &t.it), Ok(()));
}

// ---------------------------------------------------------------------------------------------
// 3. Mutation controls — each proves it changed the intended field first
// ---------------------------------------------------------------------------------------------

/// Declaring the model at all moves the digest: the facet is part of the meaning.
#[test]
fn declaring_a_model_moves_the_digest() {
    let (plain_ir, _) = plain();
    let (model_ir, _) = model();
    assert!(!plain_ir.ir.declares_executable_model());
    assert!(model_ir.ir.declares_executable_model());
    assert_ne!(digest(&plain_ir), digest(&model_ir));
}

#[test]
fn a_role_change_moves_the_digest() {
    let (left, [x, ..]) = model();
    let (mut right, _) = model();
    right.ir.set_model_facet(
        x,
        Some(tensor(TensorRole::Intermediate, &[1, 4], DType::F32)),
    );
    assert_ne!(
        left.ir.model_facet(x),
        right.ir.model_facet(x),
        "observation floor"
    );
    assert_ne!(digest(&left), digest(&right));
}

#[test]
fn a_shape_change_moves_the_digest() {
    let (left, [x, ..]) = model();
    let (mut right, _) = model();
    right
        .ir
        .set_model_facet(x, Some(tensor(TensorRole::Input, &[1, 5], DType::F32)));
    assert_ne!(left.ir.model_facet(x), right.ir.model_facet(x));
    assert_ne!(digest(&left), digest(&right));
}

#[test]
fn a_rank_change_moves_the_digest() {
    let (left, [x, ..]) = model();
    let (mut right, _) = model();
    right
        .ir
        .set_model_facet(x, Some(tensor(TensorRole::Input, &[1, 4, 1], DType::F32)));
    assert_ne!(left.ir.model_facet(x), right.ir.model_facet(x));
    assert_ne!(digest(&left), digest(&right));
}

#[test]
fn a_dtype_change_moves_the_digest() {
    let (left, [x, ..]) = model();
    let (mut right, _) = model();
    right
        .ir
        .set_model_facet(x, Some(tensor(TensorRole::Input, &[1, 4], DType::F64)));
    assert_ne!(left.ir.model_facet(x), right.ir.model_facet(x));
    assert_ne!(digest(&left), digest(&right));
}

#[test]
fn an_operator_identity_change_moves_the_digest() {
    let (left, [.., e]) = model();
    let (mut right, _) = model();
    let id = right.it.intern("relu");
    right.ir.set_model_facet(
        e,
        Some(ModelFacet::Operator(OperatorContract { identity: id })),
    );
    assert_ne!(left.ir.model_facet(e), right.ir.model_facet(e));
    assert_ne!(digest(&left), digest(&right));
}

/// A symbolic extent is hashed by its resolved TEXT, so interner order cannot move the digest.
#[test]
fn a_symbolic_extent_is_interner_order_independent() {
    let build = |warm: bool| {
        let (mut t, [x, ..]) = model();
        if warm {
            let _ = t.it.intern("zzz_unused");
        }
        let batch = t.it.intern("batch");
        t.ir.set_model_facet(
            x,
            Some(ModelFacet::Tensor(TensorContract {
                role: TensorRole::Input,
                shape: vec![Extent::Symbolic(batch), Extent::Static(4)],
                dtype: DType::F32,
            })),
        );
        t
    };
    assert_eq!(digest(&build(false)), digest(&build(true)));
}

// ---------------------------------------------------------------------------------------------
// 4. Typed fields are the authority, not the tags
// ---------------------------------------------------------------------------------------------

/// Replacing every annotation tag with a meaningless word leaves the model VALID: validation reads
/// the typed contract, never the tags. (The semantic digest covers annotations, which is why
/// the claim under test is about authority, not about digest identity.)
#[test]
fn meaningless_tags_do_not_invalidate_a_typed_model() {
    let (mut t, _) = model();
    let junk = t.it.intern("qqq");
    for d in t.ir.decl_nodes.iter_mut() {
        if !d.anno.tags.is_empty() {
            d.anno.tags = vec![junk];
        }
    }
    assert_eq!(
        validate_model_contract(&t.ir, &t.it),
        Ok(()),
        "tags are decoration; the typed facet is the authority"
    );
}

/// Removing a required typed element is rejected, even though every tag is still in place.
#[test]
fn a_missing_typed_output_is_rejected_with_tags_intact() {
    let (mut t, [_, _, y, _]) = model();
    t.ir.set_model_facet(y, None);
    let err = validate_model_contract(&t.ir, &t.it).expect_err("must be rejected");
    assert!(err.contains(&ModelFinding::NoOutput), "{err:?}");
}

// ---------------------------------------------------------------------------------------------
// 5. Negative controls
// ---------------------------------------------------------------------------------------------

#[test]
fn a_tensor_facet_on_an_edge_is_rejected() {
    let (mut t, [.., e]) = model();
    t.ir.set_model_facet(e, Some(tensor(TensorRole::Intermediate, &[1], DType::F32)));
    let err = validate_model_contract(&t.ir, &t.it).expect_err("must be rejected");
    assert!(
        err.iter().any(|f| matches!(
            f,
            ModelFinding::TensorFacetOnNonNode {
                kind: DeclKind::Edge,
                ..
            }
        )),
        "{err:?}"
    );
}

#[test]
fn an_operator_facet_on_a_node_is_rejected() {
    let (mut t, [x, ..]) = model();
    let id = t.it.intern("linear");
    t.ir.set_model_facet(
        x,
        Some(ModelFacet::Operator(OperatorContract { identity: id })),
    );
    let err = validate_model_contract(&t.ir, &t.it).expect_err("must be rejected");
    assert!(
        err.iter().any(|f| matches!(
            f,
            ModelFinding::OperatorFacetOnNonEdge {
                kind: DeclKind::Node,
                ..
            }
        )),
        "{err:?}"
    );
}

#[test]
fn a_model_without_an_input_is_rejected() {
    let (mut t, [x, ..]) = model();
    t.ir.set_model_facet(x, None);
    let err = validate_model_contract(&t.ir, &t.it).expect_err("must be rejected");
    assert!(err.contains(&ModelFinding::NoInput), "{err:?}");
}

#[test]
fn an_empty_operator_identity_is_rejected() {
    let (mut t, [.., e]) = model();
    let empty = t.it.intern("");
    t.ir.set_model_facet(
        e,
        Some(ModelFacet::Operator(OperatorContract { identity: empty })),
    );
    let err = validate_model_contract(&t.ir, &t.it).expect_err("must be rejected");
    assert!(
        err.iter()
            .any(|f| matches!(f, ModelFinding::EmptyOperatorIdentity { .. })),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// 6. V1 is untouched
// ---------------------------------------------------------------------------------------------

/// The v1 structural hash does not see the facet: same structure, same v1 value, model or not.
/// This is what keeps every frozen v1 golden valid.
#[test]
fn structural_hash_is_blind_to_the_model_facet() {
    let (plain_ir, _) = plain();
    let (model_ir, _) = model();
    assert_ne!(
        digest(&plain_ir),
        digest(&model_ir),
        "the semantic digest does see it"
    );
    assert_eq!(
        v1(&plain_ir),
        v1(&model_ir),
        "v1's contract is structural and must not have moved"
    );
}

// =================================================================================================
// Tier 2B — port direction, orthogonal to the algebraic sign
// =================================================================================================

use hymeko::ir::coherence::ModelFinding as MF;
use hymeko::ir::ir::{PortDirection, SignedRefR};

/// Set the direction of arc 0's slot `slot`.
fn set_dir(t: &mut TestIr, slot: usize, dir: PortDirection) {
    let atom = match &mut t.ir.arcs[0].refs[slot] {
        SignedRefR::Plus(a) | SignedRefR::Minus(a) | SignedRefR::Neutral(a) => a,
    };
    atom.direction = dir;
}

/// Flip the sign of arc 0's slot `slot`, leaving the atom (and its direction) intact.
fn flip_sign(t: &mut TestIr, slot: usize) {
    let r = t.ir.arcs[0].refs[slot].clone();
    t.ir.arcs[0].refs[slot] = match r {
        SignedRefR::Plus(a) => SignedRefR::Minus(a),
        SignedRefR::Minus(a) => SignedRefR::Plus(a),
        SignedRefR::Neutral(a) => SignedRefR::Plus(a),
    };
}

/// The model with directions declared: slot 0 (`y`) out, slots 1--2 (`x`, `W`) in.
fn directed() -> (TestIr, [DeclId; 4]) {
    let (mut t, ids) = model();
    set_dir(&mut t, 0, PortDirection::Out);
    set_dir(&mut t, 1, PortDirection::In);
    set_dir(&mut t, 2, PortDirection::In);
    (t, ids)
}

/// Every legacy incidence is `Unspecified`. Nothing invents a direction.
#[test]
fn a_legacy_incidence_has_no_direction() {
    let (t, _) = plain();
    for arc in &t.ir.arcs {
        for reference in &arc.refs {
            assert_eq!(reference.atom().direction, PortDirection::Unspecified);
        }
    }
    assert_eq!(validate_model_contract(&t.ir, &t.it), Ok(()));
}

#[test]
fn a_well_formed_directed_model_validates() {
    let (t, _) = directed();
    assert_eq!(validate_model_contract(&t.ir, &t.it), Ok(()));
}

/// **The orthogonality gate.** Same sign, changed direction: the digest must move.
#[test]
fn direction_moves_the_digest_at_a_fixed_sign() {
    let (left, _) = directed();
    let (mut right, _) = directed();
    set_dir(&mut right, 1, PortDirection::Out);
    assert_eq!(
        left.ir.arcs[0].refs[1].atom().target,
        right.ir.arcs[0].refs[1].atom().target,
        "observation floor: only the direction may differ"
    );
    assert_ne!(
        left.ir.arcs[0].refs[1].atom().direction,
        right.ir.arcs[0].refs[1].atom().direction
    );
    assert!(
        matches!(left.ir.arcs[0].refs[1], SignedRefR::Minus(_))
            && matches!(right.ir.arcs[0].refs[1], SignedRefR::Minus(_)),
        "the sign is held fixed"
    );
    assert_ne!(digest(&left), digest(&right));
}

/// **The other half of orthogonality.** Same direction, changed sign: a separate mutation, and it
/// also moves the digest. Neither axis is derived from the other.
#[test]
fn sign_moves_the_digest_at_a_fixed_direction() {
    let (left, _) = directed();
    let (mut right, _) = directed();
    flip_sign(&mut right, 1);
    assert_eq!(
        left.ir.arcs[0].refs[1].atom().direction,
        right.ir.arcs[0].refs[1].atom().direction,
        "the direction is held fixed"
    );
    assert!(
        matches!(left.ir.arcs[0].refs[1], SignedRefR::Minus(_))
            && matches!(right.ir.arcs[0].refs[1], SignedRefR::Plus(_)),
        "observation floor: the sign really changed"
    );
    assert_ne!(digest(&left), digest(&right));
}

/// The two axes are independent: four (sign, direction) combinations, four distinct digests.
#[test]
fn the_four_sign_direction_combinations_are_all_distinct() {
    let mut seen = Vec::new();
    for sign_flipped in [false, true] {
        for dir in [PortDirection::In, PortDirection::Out] {
            let (mut t, _) = directed();
            set_dir(&mut t, 1, dir);
            if sign_flipped {
                flip_sign(&mut t, 1);
            }
            seen.push(digest(&t));
        }
    }
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();
    assert_eq!(seen.len(), before, "sign and direction must not collapse");
}

/// Swapping the input and output ports is a coherence error: a model input is supplied, never
/// produced.
#[test]
fn swapping_input_and_output_is_a_coherence_error() {
    let (mut t, _) = directed();
    set_dir(&mut t, 0, PortDirection::In); // y: out -> in
    set_dir(&mut t, 1, PortDirection::Out); // x: in  -> out
    let err = validate_model_contract(&t.ir, &t.it).expect_err("the swap must be rejected");
    assert!(
        err.iter().any(|f| matches!(
            f,
            MF::ProducedInputOrParameter {
                role: TensorRole::Input,
                ..
            }
        )),
        "{err:?}"
    );
}

/// A parameter is learned, not produced.
#[test]
fn producing_a_parameter_is_a_coherence_error() {
    let (mut t, _) = directed();
    set_dir(&mut t, 2, PortDirection::Out); // W
    let err = validate_model_contract(&t.ir, &t.it).expect_err("must be rejected");
    assert!(
        err.iter().any(|f| matches!(
            f,
            MF::ProducedInputOrParameter {
                role: TensorRole::Parameter,
                ..
            }
        )),
        "{err:?}"
    );
}

/// An operator that declares directions must declare both an input and an output port.
#[test]
fn an_operator_with_only_inputs_is_rejected() {
    let (mut t, _) = directed();
    set_dir(&mut t, 0, PortDirection::In);
    let err = validate_model_contract(&t.ir, &t.it).expect_err("must be rejected");
    assert!(
        err.iter()
            .any(|f| matches!(f, MF::OperatorPortsIncomplete { has_out: false, .. })),
        "{err:?}"
    );
}

/// The round trip preserves both axes field-by-field.
#[test]
fn the_round_trip_preserves_both_axes() {
    let (t, _) = directed();
    let back = roundtrip(&t);
    for (slot, (before, after)) in t.ir.arcs[0]
        .refs
        .iter()
        .zip(back.arcs[0].refs.iter())
        .enumerate()
    {
        assert_eq!(
            before.atom().direction,
            after.atom().direction,
            "direction at slot {slot}"
        );
        assert_eq!(
            std::mem::discriminant(before),
            std::mem::discriminant(after),
            "sign at slot {slot}"
        );
    }
    assert_eq!(Canon::new(&back, &t.it).hash().0.0, digest(&t));
}

/// A pre-direction payload decodes to `Unspecified` and is not judged against the port rules.
#[test]
fn a_pre_direction_payload_decodes_as_unspecified() {
    let (t, _) = plain();
    let json = serde_json::to_string(&t.ir).expect("serialize");
    assert!(
        !json.contains("direction"),
        "an all-Unspecified document must not grow the wire"
    );
    let back: hymeko::ir::ir::Ir = serde_json::from_str(&json).expect("decode");
    for arc in &back.arcs {
        for reference in &arc.refs {
            assert_eq!(reference.atom().direction, PortDirection::Unspecified);
        }
    }
}
