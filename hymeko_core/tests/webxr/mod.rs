//! Source-first V&V for the WebXR semantic contract, over the production `hymeko::webxr::validate`.
//!
//! `*.hymeko` → RealParser → ModuleStore::compile → canonical Ir → coherence → webxr::validate.
//! Positive: the demo and a minimal scene validate clean. Negative: one fault per in-memory scene,
//! each asserting the exact `SemanticError` (NaN/Inf and dangling refs, which source cannot express,
//! are injected into a cloned Ir). Metamorphic: declaration-order and import-alias invariance.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use hymeko::common::ids::DeclId;
use hymeko::ir::ir::{AnnoR, Ir, RefAtomR, SignedRefR, ValueR};
use hymeko::module_store::module_store::{CompiledProgram, ModuleStore};
use hymeko::module_store::source_provider::{MemProvider, StdFsProvider};
use hymeko::resolution::interner::Interner;
use hymeko::util::real_parser::RealParser;
use hymeko::webxr::{RoleReason, SemanticError, validate};

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("data")
        .join("webxr")
}

struct Compiled<P: hymeko::module_store::source_provider::SourceProvider> {
    store: ModuleStore<P, RealParser>,
    prog: Arc<CompiledProgram>,
}
impl<P: hymeko::module_store::source_provider::SourceProvider> Compiled<P> {
    fn ir(&self) -> &Ir {
        &self.prog.ir
    }
    fn it(&self) -> &Interner {
        &self.store.it
    }
}

fn compile_demo() -> Compiled<StdFsProvider> {
    let mut store = ModuleStore::new(StdFsProvider::new(), RealParser);
    let prog = store
        .compile(&data_dir().join("webxr_hypergraph_demo.hymeko"))
        .unwrap_or_else(|e| panic!("demo must compile: {e:?}"));
    Compiled { store, prog }
}

fn compile_scene(scene_src: &str) -> Compiled<MemProvider> {
    let spatial = std::fs::read_to_string(data_dir().join("meta_spatial.hymeko")).unwrap();
    let webxr = std::fs::read_to_string(data_dir().join("meta_webxr.hymeko")).unwrap();
    let provider = MemProvider::default()
        .with_file("/meta_spatial.hymeko", spatial)
        .with_file("/meta_webxr.hymeko", webxr)
        .with_file("/scene.hymeko", scene_src.to_string());
    let mut store = ModuleStore::new(provider, RealParser);
    let prog = store
        .compile(Path::new("/scene.hymeko"))
        .unwrap_or_else(|e| panic!("scene must compile: {e:?}"));
    Compiled { store, prog }
}

const HEAD: &str = r#"
neg_scene {
    @"meta_webxr.hymeko";
    @"meta_spatial.hymeko";
    using meta_spatial.spatial as s;
    using meta_webxr.webxr as x;
}
"#;

fn wrap(body: &str) -> String {
    format!("{HEAD}\nscene: + <isa> s.kinds.scene {{\n{body}\n}}\n")
}

fn errs(body: &str) -> Vec<SemanticError> {
    let c = compile_scene(&wrap(body));
    validate(c.ir(), c.it())
}

const VALID_BODY: &str = r#"
    @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
    frame: + <isa> s.kinds.spatial_frame {}
    ent:   + <isa> s.kinds.spatial_entity {}
    @place: s.relations.placement { (+ ent, - frame); position [0.0, 0.0, 0.0]; orientation [0.0, 0.0, 0.0, 1.0]; scale [1.0, 1.0, 1.0]; }
"#;

// ---------------------------------------------------------------------------------------------
// positive
// ---------------------------------------------------------------------------------------------

#[test]
fn demo_validates_clean() {
    let c = compile_demo();
    let v = validate(c.ir(), c.it());
    assert!(v.is_empty(), "the demo must be valid, got: {v:#?}");
}

#[test]
fn minimal_valid_scene_validates_clean() {
    let v = errs(VALID_BODY);
    assert!(v.is_empty(), "minimal scene must be valid, got: {v:#?}");
}

// ---------------------------------------------------------------------------------------------
// negative — one fault per scene, exact SemanticError
// ---------------------------------------------------------------------------------------------

#[test]
fn missing_session_mode() {
    let v =
        errs(r#"@xr_session: x.session.session_config { (- x.reference_spaces.local_floor); }"#);
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::SessionModeCardinality { found: 0, .. })),
        "{v:#?}"
    );
}

#[test]
fn two_session_modes() {
    let v = errs(
        r#"@xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, + x.session_modes.immersive_vr, - x.reference_spaces.local_floor); }"#,
    );
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::SessionModeCardinality { found: 2, .. })),
        "{v:#?}"
    );
}

#[test]
fn missing_reference_space() {
    let v = errs(r#"@xr_session: x.session.session_config { (+ x.session_modes.immersive_ar); }"#);
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::ReferenceSpaceCardinality { found: 0, .. })),
        "{v:#?}"
    );
}

#[test]
fn capability_required_optional_conflict() {
    let v = errs(
        r#"
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
        @req: x.session.capability_request { (+ scene, - x.capabilities.hit_test); requirement "required"; }
        @opt: x.session.capability_request { (+ scene, - x.capabilities.hit_test); requirement "optional"; }
    "#,
    );
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::CapabilityRequiredOptionalOverlap { .. })),
        "{v:#?}"
    );
}

#[test]
fn unknown_capability() {
    let v = errs(
        r#"
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
        notcap: + <isa> s.kinds.spatial_frame {}
        @req: x.session.capability_request { (+ scene, - notcap); requirement "optional"; }
    "#,
    );
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::UnknownCapability { .. })),
        "{v:#?}"
    );
}

#[test]
fn bad_capability_requirement() {
    let v = errs(
        r#"
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
        @req: x.session.capability_request { (+ scene, - x.capabilities.hit_test); requirement "maybe"; }
    "#,
    );
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::BadCapabilityRequirement { .. })),
        "{v:#?}"
    );
}

#[test]
fn position_wrong_length() {
    let v = errs(
        r#"
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
        frame: + <isa> s.kinds.spatial_frame {}
        ent:   + <isa> s.kinds.spatial_entity {}
        @place: s.relations.placement { (+ ent, - frame); position [0.0, 0.0]; }
    "#,
    );
    assert!(
        v.iter().any(|e| matches!(
            e,
            SemanticError::PoseBadLength {
                expected: 3,
                found: 2,
                ..
            }
        )),
        "{v:#?}"
    );
}

#[test]
fn zero_quaternion() {
    let v = errs(
        r#"
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
        frame: + <isa> s.kinds.spatial_frame {}
        ent:   + <isa> s.kinds.spatial_entity {}
        @place: s.relations.placement { (+ ent, - frame); orientation [0.0, 0.0, 0.0, 0.0]; }
    "#,
    );
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::QuaternionNotNormalizable { .. })),
        "{v:#?}"
    );
}

#[test]
fn scale_non_positive() {
    let v = errs(
        r#"
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
        frame: + <isa> s.kinds.spatial_frame {}
        ent:   + <isa> s.kinds.spatial_entity {}
        @place: s.relations.placement { (+ ent, - frame); scale [-1.0, 1.0, 1.0]; }
    "#,
    );
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::ScaleNonPositive { index: 0, .. })),
        "{v:#?}"
    );
}

#[test]
fn frame_cycle() {
    let v = errs(
        r#"
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
        fa: + <isa> s.kinds.spatial_frame {}
        fb: + <isa> s.kinds.spatial_frame {}
        @p1: s.relations.parent_frame { (+ fa, - fb); }
        @p2: s.relations.parent_frame { (+ fb, - fa); }
    "#,
    );
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::FrameCycle { .. })),
        "{v:#?}"
    );
}

#[test]
fn placement_two_subjects_is_wrong_role() {
    let v = errs(
        r#"
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
        frame: + <isa> s.kinds.spatial_frame {}
        e1: + <isa> s.kinds.spatial_entity {}
        e2: + <isa> s.kinds.spatial_entity {}
        @place: s.relations.placement { (+ e1, + e2, - frame); }
    "#,
    );
    assert!(
        v.iter().any(|e| matches!(
            e,
            SemanticError::RelationRole {
                reason: RoleReason::SubjectCount { found: 2 },
                ..
            }
        )),
        "{v:#?}"
    );
}

#[test]
fn placement_frame_not_a_frame() {
    let v = errs(
        r#"
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
        e1: + <isa> s.kinds.spatial_entity {}
        e2: + <isa> s.kinds.spatial_entity {}
        @place: s.relations.placement { (+ e1, - e2); }
    "#,
    );
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::FrameMissingOrNotAFrame { .. })),
        "{v:#?}"
    );
}

#[test]
fn interaction_missing_input() {
    let v = errs(
        r#"
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
        ent: + <isa> s.kinds.spatial_entity {}
        act: + <isa> x.actions.select {}
        @ix: s.relations.interaction { (~ act, - ent); }
    "#,
    );
    assert!(
        v.iter().any(|e| matches!(
            e,
            SemanticError::RelationRole {
                reason: RoleReason::SubjectCount { found: 0 },
                ..
            }
        )),
        "{v:#?}"
    );
}

#[test]
fn unknown_action() {
    let v = errs(
        r#"
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
        ent: + <isa> s.kinds.spatial_entity {}
        inp: + <isa> x.input.controller {}
        notact: + <isa> s.kinds.spatial_frame {}
        @ix: s.relations.interaction { (+ inp, ~ notact, - ent); }
    "#,
    );
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::UnknownAction { .. })),
        "{v:#?}"
    );
}

#[test]
fn asset_uri_not_string() {
    let v = errs(
        r#"
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
        asset: + <isa> s.kinds.visual_asset { uri [1.0]; }
    "#,
    );
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::AssetUriNotString { .. })),
        "{v:#?}"
    );
}

#[test]
fn runtime_handle_field_rejected() {
    let v = errs(
        r#"
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
        ent: + <isa> s.kinds.spatial_entity { button_index 3; }
    "#,
    );
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::RuntimeHandleField { .. })),
        "{v:#?}"
    );
}

// ---------------------------------------------------------------------------------------------
// negative — hand-injected (source cannot express NaN/Inf or a dangling DeclId)
// ---------------------------------------------------------------------------------------------

/// Overwrite the first attribute named `attr` with `value` (in a cloned Ir).
fn inject_attr(ir: &mut Ir, it: &Interner, attr: &str, value: ValueR) {
    let sym = it.get_id(attr).expect("attr interned");
    for node in ir.decl_nodes.iter_mut() {
        if node.name == sym {
            node.anno.value = Some(value);
            return;
        }
    }
    panic!("no `{attr}` attribute to inject");
}

#[test]
fn nan_position_is_rejected() {
    let c = compile_scene(&wrap(VALID_BODY));
    let mut ir = c.ir().clone();
    inject_attr(
        &mut ir,
        c.it(),
        "position",
        ValueR::List(vec![
            ValueR::Num(f64::NAN),
            ValueR::Num(0.0),
            ValueR::Num(0.0),
        ]),
    );
    let v = validate(&ir, c.it());
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::PoseNonFinite { index: 0, .. })),
        "{v:#?}"
    );
}

#[test]
fn inf_orientation_is_rejected() {
    let c = compile_scene(&wrap(VALID_BODY));
    let mut ir = c.ir().clone();
    inject_attr(
        &mut ir,
        c.it(),
        "orientation",
        ValueR::List(vec![
            ValueR::Num(f64::INFINITY),
            ValueR::Num(0.0),
            ValueR::Num(0.0),
            ValueR::Num(1.0),
        ]),
    );
    let v = validate(&ir, c.it());
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::PoseNonFinite { .. })),
        "{v:#?}"
    );
}

#[test]
fn dangling_reference_is_rejected() {
    let c = compile_scene(&wrap(VALID_BODY));
    let mut ir = c.ir().clone();
    // point the last arc's first participant at a non-existent decl (the placement's arc)
    let bogus = DeclId::new(ir.decl_nodes.len() + 500);
    let arc = ir.arcs.last_mut().expect("an arc exists");
    arc.refs[0] = SignedRefR::Plus(RefAtomR {
        target: bogus,
        anno: AnnoR::default(),
        weights: None,
    });
    let v = validate(&ir, c.it());
    assert!(
        v.iter()
            .any(|e| matches!(e, SemanticError::DanglingReference { .. })),
        "{v:#?}"
    );
}

// ---------------------------------------------------------------------------------------------
// metamorphic — order / alias invariance
// ---------------------------------------------------------------------------------------------

#[test]
fn declaration_order_permutation_is_invariant() {
    let permuted = r#"
        @place: s.relations.placement { (+ ent, - frame); position [0.0, 0.0, 0.0]; orientation [0.0, 0.0, 0.0, 1.0]; scale [1.0, 1.0, 1.0]; }
        ent:   + <isa> s.kinds.spatial_entity {}
        frame: + <isa> s.kinds.spatial_frame {}
        @xr_session: x.session.session_config { (+ x.session_modes.immersive_ar, - x.reference_spaces.local_floor); }
    "#;
    assert_eq!(
        errs(VALID_BODY),
        errs(permuted),
        "reordering declarations must not change the verdict"
    );
    assert!(errs(permuted).is_empty());
}

#[test]
fn import_alias_change_is_invariant() {
    // same scene, different aliases (sp/wx instead of s/x)
    let aliased = format!(
        "alias_scene {{\n@\"meta_webxr.hymeko\";\n@\"meta_spatial.hymeko\";\nusing meta_spatial.spatial as sp;\nusing meta_webxr.webxr as wx;\n}}\nscene: + <isa> sp.kinds.scene {{\n@xr_session: wx.session.session_config {{ (+ wx.session_modes.immersive_ar, - wx.reference_spaces.local_floor); }}\nframe: + <isa> sp.kinds.spatial_frame {{}}\nent: + <isa> sp.kinds.spatial_entity {{}}\n@place: sp.relations.placement {{ (+ ent, - frame); position [0.0, 0.0, 0.0]; }}\n}}\n"
    );
    let c = compile_scene(&aliased);
    assert!(
        validate(c.ir(), c.it()).is_empty(),
        "alias choice must not change the verdict"
    );
}
