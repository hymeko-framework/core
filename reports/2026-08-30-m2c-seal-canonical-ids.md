# M2c — seal canonical HyMeKo entity IDs

**Written:** 2026-08-30 14:28 CEST · **Base HEAD:** `b85f42a` · **Branch:** main (local == origin)
**Authorisation:** `APPROVED-CORE-EDIT: seal-canonical-entity-ids` (token quoted verbatim in the plan and this report; see the commit footer).

Evidence: **[M]** measured · **[S]** structurally verified (a running test / an exact symbol).

## 1. Summary

Made representation-dependent ID construction explicit and compiler-visible
**before** the physical representation changes. The `Id<T>` tuple field is now
`pub(crate)`, the implicit `usize ↔ Id` conversions are removed, and `new`/`raw`
are documented as the sole explicit construction/extraction seam. **No generational
IDs, no storage-layout change, no numeric-representation change, no mutation /
transaction / snapshot / CBOR / ir_stream / CLI change.** Runtime values, serde
schema, and performance are unchanged.

## 2. The seal (2 production files)

| file | change |
|---|---|
| `hymeko_core/src/common/ids.rs` | `Id<T>(pub usize, …)` → `Id<T>(pub(crate) usize, …)`; `new`/`raw` documented as the explicit representation-dependent seam; three `compile_fail` doctests + one compiling twin added |
| `hymeko_core/src/common/ids_impl.rs` | removed `impl From<usize> for Id<T>` and `impl From<Id<T>> for usize` (the implicit conversions). **Kept** `Serialize`/`Deserialize` (bare index), `Copy`/`Ord`/`Hash`/`Debug` |

After the seal **[S]**:
- external crates cannot read or construct the tuple field (`pub(crate)`);
- `DeclId::from(7usize)`, `id.into()`, `usize::from(id)` no longer compile;
- `DeclId::new`, `raw`, and domain-typed `EdgeId::new(d.raw())` remain (explicit,
  greppable, documented);
- cross-domain construction stays a type error (phantom tags — `DeclId` ≠ `NodeId`);
- the serde wire encoding is unchanged (a bare integer).

## 3. Compiler-identified migration (this workspace only)

`pub(crate)` keeps **all 44 core-internal `.0` sites** working unchanged. The
compiler flagged the external sites; migration was driven positionally from the
exact `E0616`/`E0308` spans (`.0` → `.raw()`), converging in 3 passes.

| crate | files migrated | sites |
|---|---|---|
| `hymeko_hnn/src` | 10 (traversal/hypergraphview, graph_traversal, workloads; tensor/{message_passing, common_traversal, tensor, mesh_nn, conv/hgnn, conv/signed_hgnn, representations/tensor_csr_representations}) | ~50 `.0`→`.raw()` |
| `hymeko_core` tests/benches/examples | 26 | 184+ `.0`→`.raw()` |
| `test_ids.rs` | 1 | `test_from_usize` → `test_explicit_construction_seam` (the two removed `From` uses) |
| **`hymeko_wgpu/src`** | **0** | wgpu references no IR ids — prediction confirmed |

**Authorised-surface verification [M]:** `hymeko_core/src` changed = **only
`ids.rs` + `ids_impl.rs`**; `hymeko_wgpu/src` untouched; no `Cargo.toml`/`Cargo.lock`
change. Sibling repos (`graph`, `network`, `transformation`, `hotaru`) pin
`hymeko_core` at older revisions and are **not** edited — insulated until they bump
their pin.

## 4. Tests & gates [M]

- `compile_fail` doctests: **3 pass** (`.0`, `7usize.into()`, `id.into()` each fail
  to compile) + the compiling twin (`new`/`raw`, `EdgeId::new(d.raw())`) passes.
- Serde schema-compat: `basic::test_ids::test_serde_roundtrip` still asserts
  `DeclId::new(123)` → `"123"` (unchanged).
- Full workspace: **542 passed / 1 failed / 1 ignored** — the 1 failure is the
  documented `test_load_meta_geometry` 58/57 drift (left visible/unsilenced). All
  migrated `.0`→`.raw()` sites yield identical values, so every existing test still
  passes.
- `fmt --check` clean; `clippy --workspace --all-features --all-targets -D warnings`
  clean; `check`/`build --all-targets` clean.
- **Production line coverage 85.19 %** (same denominator/skips as the M2b/PoW-0
  baseline — unchanged; the seal is visibility + doc + an already-covered inline
  accessor, and the removed `From` impls were exercised only by the replaced test).

## 5. Behaviour / representation / performance — preserved [S]

- **Representation:** `Id<T>` is still `#[repr(C)]` `(usize, PhantomData<T>)`; no
  numeric change, no generation counter, no slot reuse.
- **Behaviour:** `raw()` is `#[inline(always)]` returning `self.0` — identical
  codegen to the former `.0`; every migrated site is value-identical.
- **Serde:** bare-index encoding unchanged (tested).

## 6. Requirement matrix

`UIR-M2C-01` ("sealing the ID field converts silent mis-indexing into a compile
error") moves **NOT_IMPLEMENTED → PASS**; new `UIR-M2C-05/06/07` (no implicit
conversion; serde schema-compat; representation/perf-preserving) are PASS.
`UIR-M2C-03` (Python-held ID lifetime) and `UIR-M2C-04` (generational key ≤
`Option<Id>` size) stay **NOT_IMPLEMENTED** — those are the *next* phases
(`canonical-generational-arenas`), not this token.

## 7. What this phase deliberately did NOT do

No generational IDs, no arena/storage-layout change, no numeric-representation
change, no generation counters, no slot reuse/deletion, no dependency/manifest/
lockfile change, no parser/lowering/mutation/transaction/CBOR/snapshot/ir_stream/
CLI change, no sibling-repo edit, no compatibility façade.

## 8. Files touched

Production: `hymeko_core/src/common/ids.rs`, `hymeko_core/src/common/ids_impl.rs`
(the seal); `hymeko_hnn/src/**` (10 compiler-identified migration files). Tests/
benches/examples: 27 `hymeko_core` files + `hymeko_wgpu/tests/pow0_vertical_slice.rs`
(`.0`→`.raw()`). Docs: `docs/plans/2026-08-30-m2c-seal-canonical-ids/*`,
`docs/verification/unified-ir/traceability.csv` (M2C rows), this report. No
`Cargo.toml`/`Cargo.lock`/`data/**`; the three IFAC artifacts untouched.

## HARD STOP

The token authorised the public-contract sealing only. The next phase
(`APPROVED-CORE-EDIT: canonical-generational-arenas`) is not begun.
