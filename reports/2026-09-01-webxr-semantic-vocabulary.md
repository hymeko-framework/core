# WebXR semantic vocabulary (PoW-0) — report

**APPROVED-CORE-EDIT: webxr-semantic-vocabulary** · 2026-09-01 11:23 CEDT
Branch `feat/core-webxr-semantic-vocabulary` @ base `70b4f36` (core `main`).

## Summary

A WebXR-demo **semantic** vocabulary, a compilable `.hymeko` demo scene, and a strict validator —
all in `hymeko_core`, on the canonical `Ir`. Semantic phase only: no JS/TS, WASM, WebXR session,
renderer, WebGL/WGPU, scene DTO, second scene graph, asset loader, or XR runtime handles in the Ir;
no new crate, no parser-grammar or Ir-layout change.

## Design basis (searched, not assumed)

The user asked whether standards or CogInfoCom could supply the schema. A bounded search found:
**X3D** (Web3D Consortium, ISO/IEC 19775) is the only ISO-standard *declarative* 3D scene language —
a typed node taxonomy that maps cleanly onto a HyMeKo `<isa>` vocabulary; **glTF** is the asset/PBR
format; **A-Frame** is the pragmatic entity-component model; the **WebXR Device API** is the *runtime*
layer (whose declarative counterparts we model, not its handles). **CogInfoCom** is a real field
(MaxWhere / Széchenyi / SZTAKI) but a bounded search found **no published scene meta-model/ontology**
to inherit — it supplies concepts (channels, cognitive entities), not a node taxonomy. The user chose
**X3D-mirror + CogInfoCom top**, so the top layer is authored (CogInfoCom-inspired) and the concrete
layer mirrors X3D + WebXR-declarative.

## What shipped

- `data/webxr/webxr_vocabulary.hymeko` — two-layer `<isa>` vocabulary in the `meta_kinematics` style:
  - abstract top: `cognitive_entity` → `spatial_frame` / `perceptual` / `channel`;
  - concrete kinds: `node`, `reference_space`, `anchor` (isa spatial_frame); `geometry`, `appearance`,
    `light` (isa perceptual); `viewpoint`, `input_source` (isa channel); plus `units`.
- `data/webxr/webxr_scene_demo.hymeko` — a demo scene importing the vocabulary (`@"…"` +
  `using … as e`): a `local-floor` reference space containing a box node, a sphere node with a nested
  anchor, a viewpoint, a directional light, and a right-hand input source.
- `hymeko_core/tests/webxr/mod.rs` — `validate_scene(ir, it) -> Vec<Violation>`, a reusable strict
  validator reading only the canonical Ir, plus 6 tests.

Both `.hymeko` files pass the CLI gate (`target/release/hymeko_core <file>` → exit 0). The demo's
import resolves (`webxr_vocabulary -> webxr_vocabulary.hymeko`); combined Ir = 43 decls.

## Validator invariants (read-back — a green compile ≠ correct meaning)

`validate_scene` enforces, over the canonical Ir (bases / attributes / containment):
1. every scene element `<isa>` a known vocabulary kind (no bare non-instance nodes; no foreign base);
2. required attributes present per kind — `reference_space.type`, `geometry.primitive`, `light.type`,
   and `input_source.handedness ∈ {left, right, none}`;
3. every `anchor` is spatially contained (parent chain) within a `reference_space`;
4. the vocabulary's own two-layer structure — every element kind `<isa>` a meta kind, every meta kind
   `<isa>` `cognitive_entity`.

Proven **strict**, not vacuous: 4 negative tests compile deliberately-malformed in-memory scenes
(a bare `blob`, `handedness "sideways"`, a `reference_space` with no `type`, an anchor outside any
reference space) and assert the exact `Violation` is reported. The valid demo returns `[]`.

## Tests / gates

- **webxr module: 6/6 pass** — `demo_scene_is_valid`, `vocabulary_two_layer_isa_structure_holds`, and
  the four negative rejections.
- **Full core workspace: 508 passed / 0 failed / 3 ignored.** fmt clean; lib clippy `-D warnings` clean
  (test crate is `#![allow(warnings)]` by design; my edits are test/data only).
- The documented 57/58 `meta_geometry` drift is untouched and not "fixed."

## CORE.YAML items touched

- `data/**` (`lockdown: full`): **two new files** under `data/webxr/` — authorised by the
  `webxr-semantic-vocabulary` token; **no existing fixture edited** (git confirms no modified `data/**`).
- `**/tests/**` (allowlisted, editable): new `tests/webxr/mod.rs`; registered in `tests/mod.rs`; and
  the corpus golden bumped (below).
- No `src/**`, no `Cargo.toml`, no dependency, no parser/Ir-layout change.

## Corpus golden update (a consequence I caused and reconciled honestly)

Adding two lowerable fixtures under `data/**` moved `v1_corpus_digest_is_unchanged` from 96 to 98
fixtures (CORE.YAML warns `data/**` is content-addressed by tests). This is **corpus growth, not a
lowering regression**: the count guard proves the set grew by exactly +2 (my files), git confirms no
existing fixture was modified, and I touched no lowering code — so the existing 96 lower identically.
Regenerated via the repo's own `print_corpus_digest_for_capture` and updated
`KAT_CORPUS_COUNT 96→98`, `KAT_CORPUS_DIGEST → de65bba…`, with a rationale comment.

## Process calibration (honest)

The four-artifact plan bundle (`plan.tex/.tikz/.mmd`) is on disk under
`docs/plans/2026-09-01-webxr-semantic-vocabulary/`, but it is a **design record produced alongside**
the PoW-0 implementation, not before it: the brief arrived truncated and the design was settled
interactively with the user (the standards search + the X3D+CogInfoCom choice). Not back-dated —
dated today and labelled as such. plan.pdf is not built (no LaTeX toolchain on this host).

## Own mistakes / retracted assumptions

- I first assumed the import namespace `webxr_vocabulary` was a parent decl; it is a **prefix** in
  `namespace_roots` (U2.1), so the imported `xr` is itself a root. Fixed the lookup path to `["xr", …]`.
- I first skipped a *bare* no-base node as if it were an attribute leaf; a valued leaf (`type "x"`) is
  an attribute, a bare `blob {}` instantiates nothing. Split the two — the negative test now passes.

## Next step

Promote `validate_scene` from a test module to a reusable `hymeko_core` API (or a `pgraph`-style CLI
subcommand) if a downstream WebXR consumer needs it outside tests — a `src` addition, so a separate
approval. The vocabulary can grow X3D coverage (materials, navigation, environment) and glTF/KHR
attribute detail as the demo needs it.

## Provenance

rustc 1.97.1, x86_64-pc-windows-msvc. Core `main` = 70b4f36 (untouched, not merged). Fresh worktree
off remote `main`; no foreign files touched.
