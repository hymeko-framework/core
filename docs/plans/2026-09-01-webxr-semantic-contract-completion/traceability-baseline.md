# WebXR semantic contract — requirement / traceability baseline
Created-at: 2026-09-01 11:45 CEDT · APPROVED-CORE-EDIT: webxr-semantic-contract-completion

Status legend: PLANNED (this phase) · NOT_IMPLEMENTED (out of scope this phase — runtime/endpoint) ·
each requirement resolves to a test or a documented contract, never a blanket PASS.

## XR-SEM — semantic entities (meta_spatial)
- XR-SEM-01  scene identity                          PLANNED → validator + demo compile
- XR-SEM-02  spatial_frame                           PLANNED
- XR-SEM-03  spatial_entity                          PLANNED
- XR-SEM-04  visual_asset (URI/media-type/role only) PLANNED
- XR-SEM-05  appearance/material hint                PLANNED
- XR-SEM-06  semantic_anchor                         PLANNED
- XR-SEM-07  affordance                              PLANNED
- XR-SEM-08  action                                  PLANNED
- XR-SEM-09  input_role                              PLANNED
- XR-SEM-10  accessibility label                     PLANNED

## XR-SPACE — spatial contract (meta_spatial)
- XR-SPACE-01 pose relative to a named frame         PLANNED → validator (frame exists)
- XR-SPACE-02 position = 3 finite numbers            PLANNED → negative tests
- XR-SPACE-03 orientation = 4 finite numbers (quat)  PLANNED
- XR-SPACE-04 quaternion non-zero, normalizable      PLANNED
- XR-SPACE-05 scale = 3 finite positive numbers      PLANNED
- XR-SPACE-06 coordinate contract: m, right-handed, +X right, +Y up, -Z fwd, quat xyzw  PLANNED → docs + tests
- XR-SPACE-07 scale separate from rigid transform    PLANNED → docs
- XR-SPACE-08 parent-frame relation                  PLANNED
- XR-SPACE-09 frame hierarchy acyclic                PLANNED → cycle-safe validator + negative test
- XR-SPACE-10 placement hyperedge (entity+frame+pose) role cardinality  PLANNED

## XR-INTERACT — interaction (meta_spatial + meta_webxr)
- XR-INTERACT-01 interaction hyperedge (input+action+target[+context]) PLANNED
- XR-INTERACT-02 hardware-independent actions (select/grab/release/…)   PLANNED
- XR-INTERACT-03 input roles (gaze/screen/transient/controller/hands)   PLANNED
- XR-INTERACT-04 effect (e.g. highlight connected component)            PLANNED
- XR-INTERACT-05 anchoring relation (entity/frame/anchor)               PLANNED

## XR-SPACEXR — WebXR specialization (meta_webxr)
- XR-SPACEXR-01 exactly one session mode                         PLANNED → validator + negative
- XR-SPACEXR-02 exactly one primary reference space              PLANNED
- XR-SPACEXR-03 required/optional capability sets disjoint       PLANNED
- XR-SPACEXR-04 required vs optional vs runtime-reported capability distinction  PLANNED
- XR-SPACEXR-05 session modes: inline/immersive-vr/immersive-ar (stable)         PLANNED
- XR-SPACEXR-06 reference spaces: viewer/local/local-floor/bounded-floor/unbounded (stable) PLANNED
- XR-SPACEXR-07 capabilities: hit-test/hand-tracking/dom-overlay (stable), anchors/layers (draft) PLANNED

## XR-VALID — validator invariants (production API over &Ir)
- XR-VALID-01 every spatial entity's frame exists                PLANNED
- XR-VALID-02 no dangling reference                              PLANNED
- XR-VALID-03 no field interpretable as a runtime WebXR handle   PLANNED
- XR-VALID-04 asset URI is a string                              PLANNED
- XR-VALID-05 bounded, deterministic, canonical-ID errors, per-family variant, no panic/unwrap  PLANNED
- XR-VALID-06 O(declarations + incidences), cycle check linear   PLANNED → benchmark
- XR-VALID-07 accessor-based (no raw ir.nodes/ir.edges indexing) PLANNED

## XR-DYNAMIC — canonical vs runtime boundary (documented contract only)
- XR-DYNAMIC-01 canonical Ir holds identity/authoring-pose/placement/affordance/capability/asset  PLANNED → docs + test
- XR-DYNAMIC-02 Ir excludes XRSession/XRFrame/XRSpace/XRAnchor/GL/DOM/button-index/per-frame pose  PLANNED → validator (no runtime field)
- XR-DYNAMIC-03 semantic_anchor DeclId ≠ XRAnchor runtime handle                                   NOT_IMPLEMENTED (doc contract)
- XR-DYNAMIC-04 high-freq pose = runtime transient; durable place/grab = future IRDelta            NOT_IMPLEMENTED (doc contract)
- XR-DYNAMIC-05 WebXR endpoint binding / re-sync                                                    NOT_IMPLEMENTED (PoW-1)
