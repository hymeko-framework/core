# Semantic ownership map — WebXR contract completion
Created-at: 2026-09-01 11:55 CEDT · from the two §0 read-only audits (vocabulary reuse; Ir accessor API)

## Reuse decisions (concept -> owner -> action)
| concept | existing | decision |
|---|---|---|
| multiway relation idiom | `@`-hyperedge signed `(+ src, - sink, ~ undirected)` (meta_kinematics @joint, meta_context @context_edge) | REUSE the idiom natively — relations are @-edges, not attributes |
| frame (physical/URDF) | `meta_kinematics.kinematics.elements.frame` (degrees, RPY, Z-up) | NOT imported — different domain/convention; documented; may later isa meta_spatial.spatial_frame |
| pose (structured) | `[[xyz],[rpy-deg]]` incidence weight (meta_kinematics/hymeko_geometry) — degrees, RPY, no quaternion | REUSE the numeric-list PATTERN; AUTHOR a quaternion+meters pose for meta_spatial (WebXR/glTF convention). Divergence documented. |
| geometry primitives | `meta_kinematics.geometry.{box,cylinder,sphere,capsule}` (collision, numeric dims) | NOT imported — demo needs a lightweight *visual* primitive hint, not collision solids |
| action (robot command) | `meta_task.task.@action` + `@move_to`/`@grip_close` (actor->target) | NOT imported — WebXR interaction verbs (select/grab) are a different domain; documented as related-not-same |
| anchor | my PoW-0 `anchor <isa> spatial_frame` + containment rule | EVOLVE into meta_spatial.semantic_anchor (carry the containment invariant forward) |
| appearance/material | my PoW-0 `appearance` (empty); kinematics `color [r,g,b,a]` list | EVOLVE: appearance kind + numeric `color [r,g,b,a]` (drop the PoW-0 string) |
| affordance / capability / feature-negotiation | NONE (only prose + one instance attr) | AUTHOR new (meta_spatial affordance; meta_webxr capability with required/optional/runtime-reported) |
| asset / URI / media-type | NONE in any vocabulary | AUTHOR new (meta_spatial visual_asset: uri/media_type/role) |

## PoW-0 supersession
My PoW-0 `data/webxr/webxr_vocabulary.hymeko` + `webxr_scene_demo.hymeko` + `tests/webxr/mod.rs` are
the draft. They are REPLACED by `meta_spatial.hymeko` + `meta_webxr.hymeko` + `webxr_hypergraph_demo.hymeko`
+ a production `src/webxr` validator. Removing 2 and adding 3 fixtures re-bumps the corpus golden
(reconciled at the end; §data-fixture-changes-corpus-digest).

## Accessor-API decisions (validator over &Ir + &Interner)
- Traverse via `ir::access`: `decls()` (filter parent_of==None for roots), `child_decls` (budget-bounded,
  cycle-safe), `parent_of`, `kind_of`, `node_for_decl`/`edge_for_decl`/`arc_for_decl`, `decl_node` for values.
- Bases via `node_for_decl(d).1.bases` / `edge_for_decl(d).1.bases` (accessor-obtained record, not ir.nodes[..]).
- Incidences via `build_incidence_index(ir)` (O(decls+nnz)) OR forward `edge_for_decl->arcs->arc(a).refs`.
- Values: `decl_node(d).anno.value: ValueR::{Str,Num,List,Ref}`; List = Vec<ValueR>, match each Num(f64).
- isa tag: `it.get_id("isa")`, `anno.tags.contains(&sym)`. Errors hold DeclId/HyperArcId/SymId — never strings.
- Cycle-safety: frame-parent-cycle + anchor-containment walks use an explicit visited-set / decl-count
  budget (Ir does NOT guarantee acyclic containment; the existing coherence checker needs an &Index we lack).

## Convention divergence (documented, deliberate)
meta_spatial = WebXR/glTF convention: **meters, quaternion order x,y,z,w, right-handed, +X right, +Y up,
-Z forward, scale separate from the rigid transform**. This differs from meta_kinematics (URDF: degrees,
RPY Euler, Z-up). They are different domains (visual/XR vs physical/robot); not a duplication.
