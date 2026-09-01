# WebXR semantic contract completion — return report

**APPROVED-CORE-EDIT: webxr-semantic-contract-completion** · 2026-09-01 12:23 CEDT
Branch `feat/webxr-semantic-pow0` @ base `70b4f36` (core `main`). Core main **not merged**.

## 1. Commit SHAs and remote feature-tip
Commits (newest first): `4c1492c` docs+coverage · `012b7fd` perf(linear validator)+bench ·
`b7925f4` vocab+validator+demo+tests · `0d35e8e` plan/audit · (branched from PoW-0 `cee3066`).

## 2. Modified files
New: `data/webxr/{meta_spatial,meta_webxr,webxr_hypergraph_demo}.hymeko`; `hymeko_core/src/webxr/mod.rs`;
`docs/webxr-semantic-model.md`; the plan bundle; this report. Rewritten: `hymeko_core/tests/webxr/mod.rs`.
Edited: `hymeko_core/src/lib.rs` (`pub mod webxr;`), `tests/unified_ir/validation/corpus.rs` (golden 98->99).
Removed: PoW-0 `data/webxr/{webxr_vocabulary,webxr_scene_demo}.hymeko`.

## 3. Reused existing meta-elements
The multiway `@`-hyperedge idiom (`+`src/`-`sink/`~`peer), from meta_kinematics @joint / meta_context
@context_edge. Considered-and-rejected imports (ownership map): meta_kinematics frame/pose (URDF degrees/
RPY/Z-up, a different domain), its collision primitives, and meta_task @action robot verbs. Reused the
pattern (numeric-list values, signed incidences), not the declarations.

## 4. New meta_spatial elements
Kinds: scene, spatial_frame, spatial_entity, visual_asset, appearance, semantic_anchor, affordance,
action, input_role. Relations: @placement, @parent_frame, @appearance_binding, @anchoring, @interaction,
@effect. Pose: position[x,y,z]/orientation[x,y,z,w]/scale[sx,sy,sz]; contract metres/quaternion-xyzw/
right-handed/+X/+Y/-Z.

## 5. New meta_webxr elements
Session modes (inline/immersive_vr/immersive_ar), reference spaces (viewer/local/local_floor/bounded_floor/
unbounded), capabilities (hit_test/hand_tracking/dom_overlay stable; anchors/layers draft, each with
stability), input roles (gaze/screen_pointer/transient_pointer/controller/left_hand/right_hand/any_hand),
actions (select/grab/release/activate/move/rotate/inspect/place), and @session_config / @capability_request.

## 6. Role / cardinality contract
Placement: one `+` spatial_entity, every `-` a spatial_frame. Interaction: one `+` input, >=1 `-` target,
each `~` a known action. Anchoring: `+` entity, `-` frame, `~` semantic_anchor. Session config: one primary
mode + one primary reference space per scene, `~` fallback optional. Capability request: `-` a known
capability, requirement in {required,optional}, required-cap-optional disjoint. Table in the model doc.

## 7. Fordithato demo -- exact path
data/webxr/webxr_hypergraph_demo.hymeko -> RealParser -> ModuleStore::compile -> canonical Ir (167 decls,
26 edges, 18 arcs) -> coherence -> webxr::validate = CLEAN. A 6-service dependency network as a spatial
hypergraph: two multiway domain @depends edges (3- and 4-way), two frames, one @session_config
(immersive-ar primary, local-floor space, inline fallback), optional hit-test/anchors, controller
grab/release/select interactions, a semantic anchor, quaternion poses, appearance + glTF asset hints,
accessibility labels, a highlight-connected-component @effect on select. Every .hymeko is compiled
through the real parser.

## 8. Validator error taxonomy
SemanticError (ids-only, one variant per family): VocabularyMissing, SessionModeCardinality,
ReferenceSpaceCardinality, CapabilityRequiredOptionalOverlap, UnknownCapability, BadCapabilityRequirement,
FrameMissingOrNotAFrame, FrameCycle, PoseBadLength, PoseNonFinite, QuaternionNotNormalizable,
ScaleNonPositive, RelationRole{SubjectCount|WrongKind|NoTarget}, UnknownAction, AssetUriNotString,
DanglingReference, RuntimeHandleField. No panic/unwrap; canonical DeclId/SymId only.

## 9. Test count and coverage
Full workspace 525 passed / 0 failed / 4 ignored. webxr suite: 27 tests (2 positive, 16 source-negative,
3 hand-injected NaN/Inf/dangling, 2 metamorphic, 4 coverage) + 1 ignored bench. src/webxr line coverage
95.3%, functions 100% (cargo-llvm-cov 0.9.0); residual = depth-budget defensive tripwires. fmt + lib
clippy -D warnings clean. 57/58 drift untouched.

## 10. Fault-mutant results (mutant -> catching test)
frame-cycle guard -> frame_cycle; quaternion-norm -> zero_quaternion; role-cardinality ->
placement_two_subjects / interaction_missing_input; required-optional overlap ->
capability_required_optional_conflict; entity-kind -> placement_frame_not_a_frame /
anchoring_non_entity_subject; pose length/finite/scale -> position_wrong_length / nan_position /
inf_orientation / scale_non_positive; runtime-handle -> runtime_handle_field_rejected; dangling ->
dangling_reference_is_rejected. No mutant committed.

## 11. Compile/validation benchmark and memory
Release, median-of-3, parse+lowering vs validation:
  n=1e2  parse+lower 1.9 ms  validate 0.01 ms (0.10 us/ent)
  n=1e3  13.7 ms  0.07 ms (0.07)
  n=1e4  161 ms   1.66 ms (0.17)
  n=1e5  2265 ms  32.1 ms (0.32)
Validator O(decls+incidences): one classification pass, one reusable participant buffer (no per-entity
temp Vec), allocation-free bounded transitive-isa, no full-Ir clone, no scene DTO. The ~2x per-decl rise
(1e4->1e5) is memory latency at ~5e5 decls, not extra work. Parse+lowering (parser cost) dominates
end-to-end. Peak RSS << 16 GiB.

## 12. WebXR standard / draft capabilities
Stable (W3C CR/TR): session modes, reference spaces, hit_test, hand_tracking, dom_overlay. Draft
(Immersive-Web editor drafts): anchors, layers. Each carries a stability tag; the demo requests hit_test
and anchors as optional only.

## 13. Canonical vs runtime state boundary
Documented + enforced: identity/authoring-pose/placement/affordance/capability-intent/asset stay in the
Ir; XRSession/XRFrame/XRSpace/XRAnchor, GL/DOM, button indices, per-frame poses, permission/tracking state
do not. RuntimeHandleField rejects reserved attribute names. Semantic anchor DeclId != XRAnchor handle.

## 14. Own mistakes / retracted hypotheses
- Session mode first modelled by inherited-kind counting -> flagged the legitimate fallback; replaced with
  an explicit @session_config distinguishing primary from fallback.
- Validator first rescanned all decls ~7x + HashSet/Vec per call (superlinear, 5.2 us/entity); reworked to
  one bucket pass + reusable buffer + allocation-free isa (0.32 us/entity). Profiled, not assumed.
- Corrected my PoW-0's string coordinates to structured numeric lists.

## 15. PoW-1 smallest next step
A read-only webxr::query surface (borrowed semantic accessors over &Ir by canonical ID) -- the seam a
browser-side WebXR binding consumes, zero runtime handle in the Ir. The IRDelta durable-return path stays
a documented contract until the mutable-IR phase.

## 16. Worktree and remote state
Worktree /d/hnnwork/core-webxr clean at the feature tip; fresh branch off core main 70b4f36; no foreign
files touched. Core main = 70b4f36 (unchanged, not merged). Feature-tip pushed + remote-verified below.
HARD STOP.

## Provenance
rustc 1.97.1, x86_64-pc-windows-msvc; cargo-llvm-cov 0.9.0. Single-host timings (diagnostic).
