# Canonical hierarchy authority (U2) — decision

**APPROVED-CORE-EDIT: canonical-hierarchy-authority**
2026-08-31 20:30 CEDT · branch `feat/core-hierarchy-authority` · base `67771c9`

## Decision (up front)

**HARD STOP on the production authority-flip (Part D).** The v1 canonical hash
cannot be made to read the declaration hierarchy exclusively from the `Ir`
intrusive links while staying bit-identical, because **the v1 hash embeds the
resolution-`Index` import-namespace prefix, and the `Ir` stores no namespace
state**. A pure-links hash diverges on every multi-module program (measured: 35
of 96 corpus fixtures agree, 61 diverge — all 61 import-bearing). Preserving the
hash would require either an `algo_version` bump (forbidden this phase) or adding
namespace state to the `Ir` (a serde-wire + lowering change that is its own
phase). Per Part D's clause, HARD STOP; the enabling follow-up is specified below.

## What was verified (bit-identity is *almost* free)

The flip is sound and bit-identical **for single-module programs**, proven by an
additive link-derived hash (`canonical_program_hash_ir`) checked against the
index-based hash by a differential oracle:

- **4/4 well-formed frozen v1 KATs** — link-derived hash equals both the
  index-based hash and the frozen bytes (`frozen_hash.rs`).
- **35/96 corpus fixtures** — bit-identical.
- The link derivation matches by construction: lowering builds the links from the
  Index (`parent = idx.by_path[scope]`), arcs never parent a Node/Edge, so the
  link-derived path, skeleton vertices, parent, children (DeclId-ordered), and
  roots equal the index-derived ones — *except* the namespace prefix.

## The obstacle (root cause, fully isolated)

Lowering an import lowers the dependency under a single-segment namespace prefix
(`lower_program_to_ir` → `lower_into_ir(ir, dep, idx, it, &[ns])`). The imported
declarations' **`Index` paths carry that prefix** (`[meta_kinematics, kinematics,
…]`), but:

- the imported module's root gets `parent == NONE` (there is no declaration at
  path `[meta_kinematics]` — a namespace is not a decl), so walking the parent
  links yields `[kinematics, …]` — **the prefix is dropped**;
- the `Ir` struct has **no field** that stores the namespace (confirmed: fields
  are meta, doc_hash, decl_hash, arc_hash, decl_nodes, nodes, edges, arcs,
  decl_to_record).

So post-lowering the namespace exists **only** in the `Index`. The v1 hash's
`did_to_path`/`compute_tiebreak`/`canonical_incidence_bytes` all consume the
namespaced path, so a links-only hash changes for every imported declaration.

Evidence (differential probe, `mini_literals.hymeko`):

```
decl  15  Edge  j            idx=[mini_with_consts, j]            link=[mini_with_consts, j]        (agree)
decl  16  Node  kinematics   idx=[meta_kinematics, kinematics]    link=[kinematics]        <<< MISMATCH
decl  17  Node  units        idx=[meta_kinematics, kinematics, units]  link=[kinematics, units] <<< MISMATCH
```

The skeleton *tree* is identical (imported module is a separate root-tree in
both); only the path **strings** differ, by exactly the namespace prefix.

## Why not just fix it here

- **Bump `algo_version`** → drops namespaces from the canonical identity (two
  modules defining `kinematics` would collide). A semantic change the phase
  forbids.
- **Materialize namespaces in the `Ir`** → the correct fix, but it is a new
  field on `Ir` (serde-wire change) populated by a lowering change, plus a
  link-path builder that prepends the stored namespace. "Provably required by the
  flip", but beyond this phase's "bit-identical, no wire change" envelope, and it
  needs its own frozen-oracle pass (the imported-module fixtures' hashes must be
  re-frozen deliberately). It is the right next phase, not a side effect.

## Proposed enabling follow-up (next phase)

`APPROVED-CORE-EDIT: ir-namespace-roots` — give the `Ir` explicit import-namespace
state so it is a *complete* post-lowering authority:

- add a minimal, serde-versioned mapping (imported-root `DeclId` → namespace
  `SymId`) populated in `lower_program_to_ir`;
- the link-path builder prepends the namespace when it reaches an imported root,
  making `canonical_program_hash_ir` bit-identical on the full corpus;
- then flip `canonical_program_hash(cfg, ir, it)`, migrate `module_store.rs:263`,
  run the Part E boundary check once, and drop `CompiledProgram.idx`.

The alternative (make the namespace a real `DeclKind::Module` parent excluded from
the skeleton) also works but touches the 2-bit dispatch tag space and every
`DeclKind` match — heavier, and the phase forbids dispatch changes.

## What ships on this branch (study only — no production authority change)

- `canonical_hash.rs`: additive `canonical_program_hash_ir` + link-derived
  `build_did_to_path_from_ir` / `build_skeleton_forest_from_ir` (documented as the
  deferred form; **not** wired into `module_store`; the production hash is
  untouched).
- Differential oracle tests: `frozen_hash.rs` (link ≡ index ≡ frozen on the 4
  well-formed KATs) and `corpus.rs::u2_divergence_survey` (the 35/61 split,
  correlated 1:1 with imports).
- Plan bundle + Part A consumer audit under `docs/plans/2026-08-31-hierarchy-authority/`.
- **CORE.YAML items touched:** `ir/canonical_hash.rs` only (additive; the
  production `canonical_program_hash` is unchanged). No dependency change, no
  serde-wire change, no dispatch/ID/tensor/WGPU change. `algo_version = 1`
  unchanged. Full suite green (112 lib + 192 integration + doctests), fmt + clippy
  `-D warnings` clean.

## Return-report items

1. **Index lifecycle before/after** — unchanged this phase; `consumer-audit.md`
   maps the four categories. The post-lowering category-3 dependency (hash reads
   the Index) is *proven reducible to the links except namespaces*.
2. **Full-graph passes removed** — none in production (flip deferred). The
   link-derived form *would* remove `build_did_to_path(idx)` + the
   `HashMap<PathKey,DeclId>` prefix-parent reconstruction once namespaces are in
   the Ir.
3. **Hash/writer byte-identity** — index-based hash and writer unchanged;
   link-derived hash byte-identical on 35/96, diverges on 61/96 (imports).
4. **Benchmarks** — not run (no production change to measure; the flip is
   deferred).
5. **Malformed-hierarchy errors** — the existing typed catchers (coherence fault
   families) remain green; unchanged.
6. **Tests/coverage** — +3 differential/survey tests; suite green.
7. **Remaining dual authority** — the `Index` remains a post-lowering authority
   for the hash (unchanged); this is exactly what the follow-up removes.
8. **Own mistakes / retracted hypotheses** — (a) the plan asserted the flip was
   "bit-identical by construction"; true only for single-module programs — the
   import-namespace case was not anticipated until the corpus differential caught
   it. (b) First wrote the corpus differential as a hard-assert (correct — it
   surfaced the divergence immediately) then converted it to a survey once the
   scope was understood. (c) The initial `canonical_program_hash_ir` doc
   overclaimed "every hash byte unchanged"; corrected to state the divergence.
9. **Next-step recommendation** — `ir-namespace-roots` (above): the lowest-risk
   path that makes the Ir a complete authority and unblocks a bit-identical flip.
   A low-risk dead-code cleanup or a transaction overlay prototype are *not* the
   priority — the namespace gap is the one thing standing between here and U2.

## Provenance

Branch `feat/core-hierarchy-authority` @ `67771c9`; rustc 1.97.1,
x86_64-pc-windows-msvc. Corpus: 110 fixtures, 96 lowerable, max depth 5. Evidence
reproducible via `cargo test -p hymeko_core --test mod u2_divergence_survey --
--nocapture`.
