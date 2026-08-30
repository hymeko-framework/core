# Unified-IR — canonical entity-ID sealing (M2c, full spec)

**Written:** 2026-08-30 14:40 CEST · **Base HEAD:** `34f0b5b` (the field-seal commit) · **Branch:** main
**Authorisation:** `APPROVED-CORE-EDIT: seal-canonical-entity-ids` (in the plan, this report, and the commit footer).

This report covers the **complete** sealing task. The field-privacy + implicit-
conversion removal + migration landed in `34f0b5b`; this follow-up adds the
representation-claim fix, the SymID-gate analysis, the cross-domain compile-fail
proof, the size invariant, the PoW dependency-order corrections, and this report.
Evidence: **[M]** measured · **[S]** structurally verified.

## 1. Exact public API — before / after

| item | before | after |
|---|---|---|
| `Id<T>` tuple field | `pub usize` | **`pub(crate) usize`** — inaccessible outside `hymeko_core` |
| `impl From<usize> for Id<T>` | present | **removed** |
| `impl From<Id<T>> for usize` | present | **removed** |
| `Id::new(usize)` | `pub const` | `pub const` — **retained** as the explicit, documented reconstruction seam (see §6) |
| `Id::raw() -> usize` | `pub const` | `pub const` — retained (observation seam) |
| `Id::NONE`, `is_none`, `is_some`, `Copy`/`Ord`/`Hash`/`Debug`, serde | unchanged | **unchanged** |
| memory-representation attribute | `#[repr(C)] // Stable layout for serde and FFI` | **`#[repr(transparent)]`** + a doc stating `usize` is the in-process representation, **not** a cross-target wire ABI |

## 2. Migrated call-site categories [M]

Migration was compiler-driven from the exact `E0616`/`E0308` spans (`.0` →
`.raw()`), not blind search-and-replace.

| category | sites | resolution |
|---|---|---|
| representation-owner internal (core `src`) | 44 `.0` | unchanged — `pub(crate)` keeps them compiling |
| derived-view / workload construction (`hymeko_hnn/src`, 10 files) | ~50 `.0` + 16 `::new` | `.0`→`.raw()`; `::new` **retained** (see §6 conflict) |
| formatting / logging | (in hnn Debug impls) | `.raw()` |
| tests / benches / examples (27 files) | 184+ `.0` | `.0`→`.raw()` |
| implicit-conversion test (`test_ids`) | 2 | `test_from_usize` → `test_explicit_construction_seam` |
| `hymeko_wgpu/src` | **0** | references no IR ids |

**Authorised-surface check [M]:** `hymeko_core/src` changed = **only
`common/ids.rs` + `common/ids_impl.rs`**; `hymeko_wgpu/src` untouched; no
`Cargo.toml`/`Cargo.lock`/`data/**` change. Sibling repos (`graph`, `network`,
`transformation`, `hotaru`) pin core at `15223bc` (predates this seal) and are
**not** edited — read-only-verified insulated.

## 3. SymID gate — no conflict [S]

`SymId = Id<SymTag>` is sealed identically. **No external SymId construction
contract is broken:** SymIds are minted only by the `Interner` (`intern` /
`get_id` return `SymId`); there is **no** `SymId::new` / `SymId::from` /
tuple-construction anywhere in the workspace. Allocation/content semantics
(interner string table) are unchanged; **no generations added**; the ID framework
is **not** split or duplicated. `UIR-M2C-08` = PASS.

## 4. Compile-fail proof [M]

Four permanent `compile_fail` doctests on `Id` (each with a compiling twin, so a
failure cannot pass for an unrelated reason):

| required external failure | doctest | result |
|---|---|---|
| tuple-field access `d.0` | ids.rs:50 | rejects ✓ |
| implicit `From<usize>` (`7usize.into()`) | ids.rs:56 | rejects ✓ |
| implicit `Id -> usize` (`d.into()`) | ids.rs:61 | rejects ✓ |
| cross-domain `EdgeId::from(node.raw())` | ids.rs:68 | rejects ✓ |
| **compiling twin** (`new`/`raw`, `EdgeId::new(n.raw())`) | ids.rs:79 | compiles + passes ✓ |

The doctests were verified non-vacuous (the twin proves the import path is
correct; each `compile_fail` rejects for the sealing diagnostic).

**Deviation from Phase D #2 (arbitrary `DeclId::new(7)` compile failure) —
reported honestly.** `new()` is **retained public**, so `DeclId::new(7)` still
compiles. Reason: the top-level GOAL explicitly permits *"intentional raw/wire
reconstruction, if retained, explicit and documented"*, and the `hymeko_hnn`
**derived-view builder** (`edge_span(EdgeId::new(e))`, 5 sites) and **workload
generator** (builds whole `Ir`s, 10 sites) are representation owners for the
derived/generated id-spaces that must construct typed ids from indices. Making
`new()` `pub(crate)` cannot be resolved within the authorised surface: it forbids
new core construction API and builder relocation, and Phase B forbids a public
`from_raw` added "merely to restore every removed call site" — which is exactly
what those 16 sites are. So the delivered guarantee is Phase H's precise one —
*ordinary external Rust code cannot accidentally depend on the tuple
representation or implicit integer conversion* — and arbitrary-`new` sealing is
**deferred** to the `canonical-generational-arenas` phase, which can introduce
typed derived-id construction and retire the raw-index `::new` in hnn. This is a
retained-`new` design, not an oversight; it is documented in `ids.rs` and here.

## 5. Semantic regression — unchanged [M]

- Full workspace: **542 passed / 1 failed (the documented `test_load_meta_geometry`
  58/57 drift, visible) / 1 ignored**; every migrated `.0`→`.raw()` site is
  value-identical.
- PoW-0: `pow0_vertical_slice` 11/11 pass — same canonical hash `00d4c01a…`, same
  source→IR→view→CSR→CPU/GPU results.
- serde ID known-answer: `test_serde_roundtrip` still asserts `DeclId::new(123)` →
  `"123"` (bare-integer schema unchanged).
- NONE / invalid / valid lookup, hierarchy traversal, canonical v1 frozen vectors,
  the 96-fixture corpus digest, malformed-state error variants — all covered by
  the unchanged unified_ir suite (green).
- `fmt --check` clean; `clippy --workspace --all-features --all-targets -D
  warnings` clean; `check`/`build --all-targets` clean; doctests green.

## 6. The retained reconstruction seam (`new`/`raw`)

Per Phase B, the explicit boundary is documented, typed, and grep-able:

- `Id::new(index)` — reconstruct a **domain-typed** id from a raw index at a
  representation / deserialization / derived-view boundary. A raw index is **not**
  a validated handle; validation happens on lookup against the owning `Ir`. It is
  deliberately loud: cross-domain reuse must name both `raw()` and the target
  domain (`EdgeId::new(node.raw())`), and the implicit cross-domain path
  (`EdgeId::from(...)`) is a compile error.
- `Id::raw()` — observation/serialization extraction; not a stable wire ABI.
- No public `from_raw` was added.

A serialized ID is not automatically a live handle: serde can decode any integer,
so the id type is **not "unforgeable"** — the precise guarantee is the accidental-
dependence one in §4.

## 7. Performance & memory [M/S]

Runtime-neutral by construction: `raw()` is `#[inline(always)]` returning
`self.0` — identical codegen to the former `.0`; every migrated site is
value-identical; `#[repr(transparent)]` guarantees the same layout as `usize`.

| metric | value |
|---|---|
| `size_of::<DeclId/NodeId/EdgeId/SymId>()` | `size_of::<usize>()` (8 B) — unchanged, asserted by `test_sealing_is_size_neutral` |
| `size_of::<Option<Id>>()` | 16 B (no niche; `usize::MAX` is the live `NONE`) — **unchanged**; reducing it is the generational phase, not this one |
| production line coverage | **85.19 %** (unchanged; the seal adds no production logic — visibility + doc + an already-covered inline accessor) |

A full A/B micro-benchmark was not run because the change has **identical codegen**
(inline `raw()` == field read) and no new allocation; the size and coverage
invariants above establish the zero-cost claim structurally. The future 2×
`Option<Id>` reduction is **not** claimed here — it belongs to
`canonical-generational-arenas`.

## 8. PoW documentation corrections (Phase G)

`docs/platform-proof-of-work.md` is corrected additively: the dynamic-PoW
dependency order (seal → generational-arenas → transactions → versioned-snapshots
→ CBOR → ir_stream-apply → versioned-view-refresh → single-CLI); CBOR is
independently useful but not the first dynamic dependency; the ~20× is
JSON-vs-packed-storage, not a CBOR speedup; the first CBOR oracle is
`semantic_fingerprint(decode(encode(ir))) == semantic_fingerprint(ir)` (not
`parse(emit_hymeko)` while `IrWriter` is skeleton-only); transformation's
`emit_hymeko` is the complete candidate canonical writer (do not build a second);
`root_path` is provenance metadata, not canonical identity.

## 9. Files & line deltas

Production: `common/ids.rs` (+doc, `pub(crate)`, `repr(transparent)`,
4 compile_fail doctests + twin), `common/ids_impl.rs` (−2 `From` impls); the
10 `hymeko_hnn/src` migration files (landed in `34f0b5b`). Tests: `basic/test_ids.rs`
(+size invariant, seam test); 27 core test/bench/example files migrated (`34f0b5b`).
Docs: this report, `docs/platform-proof-of-work.md` (corrections),
`docs/verification/unified-ir/traceability.csv` (UIR-M2C-01 → PASS; +M2C-05..10),
`docs/plans/2026-08-30-m2c-seal-canonical-ids/*`. No `Cargo.toml`/`Cargo.lock`/
`data/**`; the three IFAC artifacts untouched.

## 10. Mistakes & rejected assumptions

- **Mistake (first pass, `34f0b5b`):** kept the false `#[repr(C)] // Stable layout
  for serde and FFI` comment and did not analyse the SymID gate or the Phase D #2
  `new()` question. Corrected here (`repr(transparent)`, SymID gate, retained-`new`
  conflict reported).
- **Rejected assumption:** that `new()` could be made `pub(crate)` within the
  authorised surface — refuted by the hnn derived-view/workload representation
  owners (16 sites) and the no-new-core-API / no-`from_raw` constraints.
- **Rejected assumption:** that the id type is "unforgeable" — serde can decode any
  integer; the precise guarantee is accidental-dependence only.

## 11. Remaining blockers for generational arenas

- Retire the raw-index `Id::new` in the hnn derived-view/workload layer by giving
  core a typed derived-id construction path, so `new()` can then go `pub(crate)`.
- `Option<Id>` is 16 B (no niche) — the generational key must add a niche or a
  compact layout to hit the 8 B target (`UIR-M2C-04`).
- Python-held ID validity lifetime (`UIR-M2C-03`) still undefined.

## HARD STOP

No generational arenas, storage-layout, mutation, transaction, snapshot, CBOR,
ir_stream, or CLI work. The next token is
`APPROVED-CORE-EDIT: canonical-generational-arenas`.
