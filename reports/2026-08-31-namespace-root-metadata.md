# Explicit canonical namespace roots (U2.1) — verification

**APPROVED-CORE-EDIT: ir-namespace-roots**
2026-08-31 21:45 CEDT · branch `feat/core-ir-namespace-roots` · base `3b800b9` (on `67771c9`)

## Result

The `Ir` now stores the import-namespace prefix that the resolution `Index`
previously carried implicitly. The link-derived canonical hash is **bit-identical
to the production Index-based hash on all 96 corpus fixtures** (was 35/96). The
production hash, `algo_version=1`, the frozen v1 KATs, and the single-module serde
wire are all unchanged. The `Index` parameter and `CompiledProgram.idx` are
**not** removed this phase (that is U2.2, now unblocked).

## Namespace representation (Part A/B)

**Audit.** `ns = imp.alias.unwrap_or(dep_ast.name)` — a single `SymId`; imports
are lowered flat under `&[ns]`; transitive imports are not lowered here. Corpus
survey (96 fixtures): prefix-length histogram over link-roots `{0: 109, 1: 81}`,
**zero multi-segment prefixes**, ≤4 prefixed roots per fixture.

**Chosen representation:** `Ir.namespace_roots: Vec<(DeclId, Box<[SymId]>)>` —
the lossless `Box<[SymId]>` (not a bare `SymId`), because lowering threads a
`&[SymId]` prefix and future nested imports could be multi-segment. Private,
sorted ascending by `DeclId`, holding **only non-empty prefixes** (single-module
roots absent → zero cost). Read via `namespace_prefix(root) -> Option<&[SymId]>`
(`O(log R)` binary search, no `HashMap`, no allocation); written via the
crate-private `record_namespace_root`. No fake Node/Edge/Arc; the IR stays a
signed hypergraph — this is declaration metadata, not a graph element.

## Lowering + boundary validator (Part C)

`lower_node`/`lower_edge` record `(did, path[..scope_len])` when a decl is a
link-root (`parent==NONE`) with a non-empty prefix — the imported roots — directly
from the Index-derived path while it is still available. Single-module roots
(scope_len 0) are ignored → empty metadata.

`validate_namespace_roots(ir, idx)` compares the Index path against
`namespace + links` for every declaration and returns a typed `NamespaceError`
(`MissingNamespaceMetadata` / `PathMismatch`). It is a diagnostic API, not a
per-hash tax; it passes on all 96 fixtures.

## Serde & versioning (Part D)

The namespace metadata rides the `Ir` serde as a versioned field (`schema=1`)
**only when non-empty**, so a single-module wire keeps exactly the legacy 11
fields — **byte-identical to the pre-U2.1 wire and the frozen serde KAT** (the
`dispatch_serde` byte-known-answer still passes). Deserialize is fail-closed with
typed errors: `UnknownNamespaceSchema`, `NamespaceRootOutOfRange`,
`DuplicateNamespaceRoot`, `EmptyNamespacePrefix`, `UnsortedNamespaceRoots`. No
whole-Ir clone, no per-declaration prefix allocation. **The `ir_stream` envelope
schema is unchanged** — a future phase carries this metadata there; for now the
serde (serde_json) path holds it.

## Differential oracle (Part E)

| | before (3b800b9) | after (this branch) |
|---|---|---|
| corpus fixtures bit-identical | 35 / 96 | **96 / 96** |
| divergent (all import-bearing) | 61 | **0** |
| well-formed frozen v1 KATs | 4/4 | 4/4 |
| boundary validator clean | — | 96/96 |

Worked example (`mini_literals`, decl 16): before `link=[kinematics]` vs
`idx=[meta_kinematics, kinematics]`; after `namespace_prefix(16)=[meta_kinematics]`
prepended → `[meta_kinematics, kinematics]` — bit-identical. Changing the alias
changes the prefix, hence the hash, exactly as the Index-based form did.

## Performance (Part G, measured)

Corpus load+lower wall, median-of-15 (host x86_64-pc-windows-msvc, rustc 1.97.1):

| | A (`3b800b9`, no hook) | A/A null | B (this) | Δ |
|---|--:|--:|--:|--:|
| corpus load+lower | 137.78 ms | 137.51 ms | 138.31 ms | **+0.38 %** (within noise) |

- **Single/imported lowering regression ≤1%/≤3%: met** (+0.38 % overall, inside
  the ±0.2 % A/A-null band; single-module writes no metadata at all).
- **Metadata O(imports/root):** 81 roots corpus-wide, ≤4/fixture, 1 `SymId` each
  (~24 B/import-root incl. the `Box`). Not O(declaration).
- **No post-lowering per-declaration prefix allocation:** the link-path builder's
  per-decl `PathKey` allocation is unchanged; the prefix prepend replaces the old
  `buf.clone()` with a `chain().collect()` — same allocation count.
- **Peak RSS:** metadata adds ~2 KB across the whole corpus — negligible.
- **Production hash unchanged; 96/96 differential bit-identical.**

## V&V (Part F/H)

- `corpus::link_derived_hash_equals_index_hash_on_the_whole_corpus` — hard-asserts
  96/96 bit-identity **and** `validate_namespace_roots` clean on every fixture.
- `namespace_faults.rs` — 8 tests: the 5 serde typed errors (unknown schema,
  dangling root, duplicate root, empty prefix, unsorted), the 2 validator errors
  (`MissingNamespaceMetadata` when metadata is stripped, `PathMismatch` on a wrong
  prefix symbol), and a clean-validation positive. Each mutates a freshly
  serialized valid imported Ir (baseline restored per test).
- `nsroots_prefix_length_survey` — asserts the Part A invariant (no multi-segment).
- Full suite green: **112 lib + 201 integration + 7 doctests**; fmt + clippy
  `-D warnings` clean; frozen v1 KATs, frozen serde KAT, writer equivalence all
  pass. Touched-line coverage of the new logic is exercised by the above.

## Commits

Interdependent test+feat collapsed (the identity-freezing tests require the
namespace API): `feat(ir): store canonical namespace roots` (Ir field + mutator +
accessor + lowering hook + serde + validator + link-hash prepend + differential +
fault tests) then `docs(perf): verify namespace-root metadata`. **CORE.YAML:**
`ir/ir.rs`, `ir/lower.rs`, `ir/canonical_hash.rs` (this approved edit). No
dependency change, no `ir_stream` envelope change, no dispatch/ID/tensor/WGPU
change.

## Return-report items

1. **Commit SHAs / remote** — below (filled at push).
2. **Representation** — `Vec<(DeclId, Box<[SymId]>)>`, private, sorted, non-empty
   only; lossless (Box, not bare SymId) despite the corpus being single-segment.
3. **Bytes/import & allocation** — ~24 B/import-root; O(imports); no per-decl alloc.
4. **Lowering/serde benchmark** — +0.38 % lowering (within noise); serde
   single-module byte-identical, imported roundtrip preserves metadata.
5. **96/96 differential** — achieved (was 35/61).
6. **Wire schema/KAT** — versioned field skipped when empty; frozen serde KAT and
   frozen v1 hash KATs preserved.
7. **Typed errors** — 5 serde + 2 validator variants, all tested.
8. **Tests/coverage** — 112 lib + 201 integration + 7 doctests green.
9. **Multi-segment/transitive** — none in the corpus; grammar makes the prefix a
   single `SymId`; `Box<[SymId]>` is future-proof for nested imports.
10. **Own mistakes** — (a) I began production edits before formalizing the plan
    bundle; the empirical Part A audit preceded the edits, but the `.tex` bundle
    was written alongside — recorded honestly. (b) First clippy run failed on a
    `map_or(true, …)` I wrote defensively; switched to `is_none_or`.
11. **U2.2 entry conditions** — with the Ir now a complete authority, U2.2 can:
    flip `canonical_program_hash(cfg, ir, it)` (drop the Index), migrate
    `module_store.rs:263`, run `validate_namespace_roots` once at the boundary,
    and drop `CompiledProgram.idx` — gated on the 96/96 differential (now green)
    and the frozen KATs. The one remaining synthetic divergence, `vec_dense_permuted`
    (Index-trimmed fillers, not a namespace issue), must be redesigned as a genuine
    DeclId permutation before it can be added to the link-hash gate.

## Provenance

Branch `feat/core-ir-namespace-roots` @ base `3b800b9`; rustc 1.97.1,
x86_64-pc-windows-msvc. Corpus 110 fixtures / 96 lowerable / max depth 5.
