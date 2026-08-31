# Remove post-lowering Index authority (U2.2) — verification

**APPROVED-CORE-EDIT: canonical-hierarchy-authority-flip**
2026-08-31 23:05 CEDT · branch `feat/core-hierarchy-authority-flip` · base `3db8924` (on `67771c9`)

## Result

The production canonical hash now reads the declaration hierarchy **exclusively
from the `Ir`** (intrusive links + `namespace_roots`) + `Interner` +
`CanonHashCfg` — **no `resolution::Index`**. `algo_version=1` and every hash byte
are unchanged (frozen v1 KATs + corpus digest + identity oracle all green). The
canonical hash is **14–26 % faster**. `CompiledProgram.idx` is retained but
non-authoritative (documented compatibility path). `main` unchanged.

## Part A — dense-permutation oracle repaired

`vec_dense_permuted` no longer adds fillers and removes them from the Index only
(an Index/Ir divergence, invalid under Ir authority). It now declares the **same
program in a different order** (edge first) so the dense `DeclId`s differ; no
fillers, both IRs coherent, v1 unchanged. `dense_permutation_uses_genuinely_
different_declids` proves the `DeclId` of `e` differs between the two builds and
the hash (index- and link-derived) is identical. Two sibling tests that encoded
the old Index authority were redesigned: `c6` (was a simulated-removal residue)
→ a genuine dense shift; `intrusive_parent_links_do_not_affect_the_canonical_hash`
→ `..._now_determine_the_canonical_hash` (the links now change the hash). The old
flawed forms are gone, not kept as alternative oracles.

## Part B — CompiledProgram.idx consumer inventory

| consumer | category | disposition |
|---|---|---|
| `module_store::compile` (the hash) | DeclId↔path (hash) | removed — hash reads the Ir |
| `util::pretty_print` | DeclId→path + iteration | migrated to `build_path_index` |
| `corpus` depth survey / `oracle` / `test_import_graphs` | iteration / resolution-time (test) | test-only; still read the retained `idx` |
| `namespace_faults` / boundary validator | test-only fault injection | need an Index; retained `idx` serves them |

No production consumer reads `idx` after the flip; the remaining ones are
test-only or the retained compat field.

## Part C — derived path index

`canonical_hash::build_path_index(&Ir) -> Result<DerivedPathIndex, PathIndexError>`
— a view derived from the Ir (links + `namespace_roots`): total `path_of` /
`decl_of` / deterministic path-ordered `iter` / `len`; `DuplicatePath` typed
error; no hidden cache, no lock, no per-lookup rebuild, caller-owned (a future
mutable IR documents its invalidation point). `pretty_print` uses it;
`derived_path_index_round_trips_on_the_corpus` proves `path_of`/`decl_of` round-trip
and the path count matches the Index on 96/96 fixtures.

## Part D/E — the flip and the boundary

`canonical_program_hash` collapsed to a single implementation `(cfg, ir, it)` (the
U2.1 link+namespace form); the index-based twin and the `_ir` alias are deleted —
one hash, no dual code. `compile`: lower → merkle → **once**
`validate_namespace_roots(&ir, &idx)` (returns `ModuleLoadError` on mismatch,
before the Index is dropped) → hash from the `Ir`. The full Index↔Ir comparison
runs **once at the boundary**, never on a later hash/read.

`CompiledProgram.idx` is **retained, non-authoritative**, with a doc-note and a
breaking-release removal plan. Rationale (the brief's sanctioned compatibility
path): the `pub` field may have external (sibling-repo) consumers that cannot be
verified from this repo, and the validator + namespace fault tests need an Index
to compare against. The dual **authority** is nonetheless eliminated — nothing
reads `idx` as canonical state after the boundary check. Not fixed here (separate
phases): the `node.bases` v1 coverage gap; the merkle `incidence.target` defect.

## Part F — coherence (analysis, not deletion)

Per the brief, coherence.rs is **not** mechanically deleted. Which checks changed:

- The hash no longer path-prefix-reconstructs the parent (the
  `HashMap<PathKey,DeclId>` + prefix walks), so that traversal is gone from the
  hot path. The **repeated** canonical hash traverses the hierarchy **once**
  (link-derived), satisfying the "no double full-hierarchy traversal on the hot
  path" rule.
- `validate_hierarchy_index_coherence` (coherence.rs) and `validate_namespace_roots`
  now overlap on the parent/path agreement; the former stays as the **explicit
  opt-in diagnostic** and the corruption-detection surface (it also checks child
  backlinks, sibling cycles, record backlinks the boundary check does not). It is
  **not** an automatic hot-path tax — only `compile` runs the one-time boundary
  check. A future consolidation of the two path-agreement passes is a candidate,
  but each is exercised by distinct fault tests and was left intact to avoid a
  hash-order-adjacent change in this phase.

## Performance (Part H, measured; A = `3db8924`, host x86_64 windows, rustc 1.97.1)

| metric | A | A/A null | B | Δ |
|---|--:|--:|--:|--:|
| canonical hash, 1.5k decl | 818 µs | 820 µs | 643 µs | **−21 %** |
| canonical hash, 15k decl | 10.15 ms | — | 7.47 ms | **−26 %** |
| canonical hash, 160k incidence | 26.65 ms | — | 22.84 ms | **−14 %** |
| corpus load+lower+compile | 137.8 ms | — | 133.7 ms | −3 % (no regression) |

- **Hash bit-identical** (frozen v1 KATs, corpus digest, identity oracle).
- **Canonical hash ≤3 % regression: met** (−14…−26 %, i.e. faster — the repeated
  hash is index-free).
- **Compile ≤3 %: met** (−3 %; the added boundary validation is offset by the
  faster hash).
- **≥1 Index-based full-graph pass eliminated:** the `build_did_to_path(idx)` over
  the Index `HashMap` and the prefix-parent `HashMap<PathKey,DeclId>`
  reconstruction are gone from the hash.
- **Allocation not increased:** the hash allocates less (no `HashMap`).
- **CompiledProgram retained memory:** unchanged (idx kept for compat; removal
  would reduce it — documented).
- **Derived lookup:** explicit, caller-owned, not rebuilt per call.

## V&V

Full suite green: **112 lib + 203 integration + 7 doctests**; fmt + clippy
`-D warnings` + doctests clean; ir_bench smoke runs. Coverage of the flip is
exercised by the frozen KATs, the corpus digest, the identity oracle (incl. the
two redesigned tests), the derived-index round-trip, and the U2.1 namespace fault
suite (still green). The 6 required fault classes (wrong parent, missing child
backlink, sibling cycle, duplicate child, unreachable declaration, reordered
siblings) remain covered by the existing coherence + hierarchy fault tests.

## Return-report items

1. **Commit SHAs** — filled at push.
2. **Corrected permutation proof** — `dense_permutation_uses_genuinely_different_declids`:
   `e` gets a different `DeclId`, hash identical, both coherent.
3. **CompiledProgram.idx inventory** — table above; no production reader post-flip.
4. **Public path-query API** — `canonical_hash::build_path_index` → `DerivedPathIndex`.
5. **Removed Index/full-graph passes** — the hash's `build_did_to_path(idx)` +
   prefix-parent `HashMap` reconstruction.
6. **Hash bit-identity** — frozen v1 KATs + 96-corpus digest + identity oracle.
7. **Compile/hash/path benchmark** — hash −14…−26 %, compile −3 %, derived index
   round-trips 96/96.
8. **Memory/allocation** — hash allocates less; CompiledProgram memory unchanged
   (idx retained, removal documented).
9. **Tests/coverage** — 112 lib + 203 integration + 7 doctests green.
10. **Remaining dual state** — `CompiledProgram.idx` (non-authoritative, compat);
    removal planned for a breaking release once external consumers migrate to
    `build_path_index`.
11. **Own mistakes** — (a) began production edits before formalizing the `.tex`
    plan bundle (the Part B audit preceded them; the bundle was written alongside).
    (b) Two identity tests failed on the first flip run — correctly, they encoded
    the old Index authority; redesigned rather than forced.
12. **Next recommendation** — (i) `retire-compiled-program-idx`: migrate the
    test-only consumers to `build_path_index` + a test helper that returns the
    Index, then drop the field (memory win, single authority in storage too); or
    (ii) `consolidate-coherence-path-checks`: unify `validate_hierarchy_index_coherence`'s
    prefix-parent walk with `validate_namespace_roots` behind one diagnostic.

## Provenance

Branch `feat/core-hierarchy-authority-flip` @ base `3db8924`; rustc 1.97.1,
x86_64-pc-windows-msvc. Corpus 110 fixtures / 96 lowerable / max depth 5.
