# hymeko_core — architecture & performance audit (Stage 2)

2026-08-31 19:55 CEDT · read-only · branch `feat/core-generational-arenas` @ base
`67771c9`. **No production source was edited** (Stage 2 is read-only). Companion
artifacts: `docs/core-architecture-map.md`, `docs/core-simplification-candidates.md`
(+ `.csv`), `docs/core-component-diagram.mmd`.

## Method

Four independent read-only subsystem sweeps (ir_stream; ir+resolution;
tensor+wgpu boundary; module_store+writers+util+common), each grounded in
`file:line` and cross-checked against `reports/coverage/*.lcov` (`FNDA:0`) and a
repo-wide grep, plus objective scripted metrics (LOC, prod/test split,
brace-matched function sizes, param counts, dependency edges, risk-signal counts).

## Headline

The crate is **structurally healthy**: 63 files / 13,475 LOC, but a large
fraction is inline `#[cfg(test)]` (e.g. assembler.rs is 713 prod / 1799 test);
by complexity metrics only **3 functions exceed 150 LOC** and **2 exceed 7
params**; **1 `unsafe`** (sound); production panics are ~2. The leverage is in
**duplication and dead scaffolding**, not oversized functions. Two facts dominate
the findings:

1. **The tree is stored twice and hashed twice** — intrusive links vs the path
   `Index` — and `ir/coherence.rs` (358 LOC) exists only to prove they agree.
   This is the root of a cluster of downstream duplications (F).
2. **~700 LOC of provably-dead code** ships in-tree (`tensor/decomposition.rs`
   262, `module_store/module_view.rs` 45, plus scattered dead fns), and another
   279 LOC (`tensor/shared_state.rs`) is deprecated/quarantined.

## Metrics (production)

| axis | value |
|---|---|
| files / LOC | 63 / 13,475 (heavy inline tests) |
| largest **prod** file | `ir/canonical_hash.rs` 541 (hash-sensitive); then `assembler.rs` 713 |
| functions > 150 LOC | 3 (`validate_hierarchy_index_coherence` 216, `validate_sections` 178, `probe_all_ten` 152 = test) |
| functions > 7 params | 2 (`lower_arc`, `compute_degree_by_index`) |
| `unsafe` | 1 (`interner.rs:24`, sound) |
| production `panic!` | ~2 (`compute/mod.rs:111`, `decomposition.rs:23`) |
| module layering | clean, acyclic: common → {ir_stream, resolution} → ir → module_store → {writers, util}; tensor near-isolated |

## Top-10 refactor candidates (detail)

Each carries a **proposed approval token**; none is implemented here. See the CSV
for the machine-readable form.

### 1 — Delete `tensor/decomposition.rs` (G, 262 LOC, risk none)
- **Now:** hosvd / mode-k-unfold / truncated-SVD / spectral-entropy — a private
  module (`mod.rs:8`) with **zero references** anywhere (no caller, test, bench).
- **Owner:** none — pure deletion. Also removes the subsystem's only non-test
  `panic!`/`unreachable!`/full-matrix `.to_vec()`.
- **Removable:** 262 LOC. **Perf:** −262 LOC compile; no runtime change (dead).
- **Compat risk:** none (private ⇒ unreachable downstream). **Tests:** none.
  **Benchmark:** n/a. **Rollback:** `git revert`.
- **Token:** `APPROVED-CORE-EDIT: drop-dead-decomposition`.

### 2 — Unify `intern_pass.rs` owned/borrowed twin families (A, ~150 LOC, risk low)
- **Now:** 13 `lower_*` (`<'a>`) and 11 `lower_*_owned` (`<'static>`) are copied
  token-for-token; the borrowed output holds only `SymId` + interned strings, so
  the `'a` is **vacuous** — one family serves both entry points.
- **Owner:** `resolution::intern_pass` (one generic family). **Removable:** ~150.
- **Perf:** neutral (same work). **Compat risk:** low — `intern_ast` and
  `intern_ast_into_owned` must stay behaviourally identical. **Tests:** structural
  equality of both entry points on the fano corpus. **Benchmark:** intern-pass
  wall on the 96-fixture corpus (≤3%). **Rollback:** `git revert`.
- **Token:** `APPROVED-CORE-EDIT: unify-intern-passes`.

### 3 — Delete `module_store/module_view.rs` (G, 45 LOC, risk none)
- **Now:** `build_index_modules` is a byte-for-byte second copy of the inline
  index-build in `module_store.rs:224-231`; `validate_all_refs_modules` is never
  wired in. `FNDA:0`, no external consumer. A live **divergence trap** (two
  authoritative copies of the "root under `[]`, imports under alias prefix" rule).
- **Owner:** none — deletion. **Removable:** 45. **Compat risk:** none.
  **Tests:** existing `compile()` corpus. **Rollback:** `git revert`.
- **Token:** `APPROVED-CORE-EDIT: drop-module-view`.

### 4 — Retire `module_store` dead scaffolding cluster (G/F/E, ~74 LOC, risk none)
- **Now:** `program_cache`+`CompiledProgramCache` (built empty, never read —
  would-be derived state duplicating `CompiledProgram`), `make_build_id` (never
  called), `take_last_ir` (pub, zero consumers), `writers/cbor_writer.rs::CborPayload`
  (never constructed). Capabilities advertised, wired to nothing.
- **Owner:** none — deletion / demotion. **Removable:** ~74. **Compat risk:** none
  in-repo (`take_last_ir` is pub — confirm no cdylib consumer, else `pub(crate)`).
  **Tests:** `compile()` idempotency (single-entry `last_compiled` is the live
  cache). **Token:** `APPROVED-CORE-EDIT: retire-module-store-scaffolding`.

### 5 — ir_stream section-entry offset consts (F, ~0 LOC, risk low)
- **Now:** the 32-byte entry layout is hand-coded twice — encode
  (`section.rs:246`, literals 0,2,8,16,24) and decode (`:407`) — while the
  envelope centralizes identical knowledge in `OFF_*` consts. Silent
  encode/decode drift is exactly the hazard this hostile-input codec was built to
  kill (`section.rs:5-18`).
- **Owner:** `ir_stream::section` (`ENTRY_OFF_*` consts). **Removable:** ~0 net
  (magic numbers → named consts). **Perf:** none. **Compat risk:** low (behaviour
  identical; consts just name existing offsets). **Tests:** a golden section-entry
  image test (mirror `envelope.rs:461`), which does not exist today. **Benchmark:**
  none. **Token:** `APPROVED-CORE-EDIT: section-entry-offset-consts`.

### 6 — Collapse 7-fold staging-limit guard (B, ~15 LOC, risk low)
- **Now:** `self.abandon(); return ResyncRequired(StagingLimitExceeded{what})` is
  copy-pasted at 7 sites (`assembler.rs:902-976`); one omitted `abandon()` is a
  state-machine-safety bug. Meets the B bar fully.
- **Owner:** `Assembler::staging_exceeded(&mut self, what) -> ApplyOutcome`.
  **Removable:** ~15. **Perf:** none. **Compat risk:** low. **Tests:** existing
  staging/superseded limit tests. **Token:** `APPROVED-CORE-EDIT: assembler-staging-guard-helper`.

### 7 — One child iterator (B, ~15-30 LOC, risk low)
- **Now:** three sibling-chain walkers — `DeclChildren` (ir.rs:447, panics on a
  dangling link, 2 callers), `ChildDecls` (access.rs, total/budgeted, **0
  callers**), and a coherence inline walk. The safe one is unused; the panicking
  one is live.
- **Owner:** the budgeted total iterator (make `DeclChildren` delegate, or drop
  it for `child_decls`). **Removable:** ~15-30. **Compat risk:** low (identical on
  well-formed IR; only malformed-input policy changes, toward safety). **Tests:** a
  malformed-sibling-cycle test proving the survivor does not panic. **Token:**
  `APPROVED-CORE-EDIT: one-child-iterator`.

### 8 — Unify const-eval twins (B, ~35 LOC, risk low)
- **Now:** `eval_expr` and `eval_in_context` (`const_resolve.rs:108/318`) share a
  byte-identical `Lit/Pi/Exp/Neg/Bin` core (incl. the div-by-zero branch) and
  differ only in the `Ref` arm. **f64 op order is identical** ⇒ no FP reordering.
- **Owner:** one evaluator taking a `Ref`-resolver closure. **Removable:** ~35.
  **Compat risk:** low. **Tests:** `const_resolve::tests` (arith / forward-ref /
  cycle / div0) already present. **Benchmark:** const-eval on a large const graph
  (≤3%). **Token:** `APPROVED-CORE-EDIT: unify-const-eval`.

### 9 — Split `module_store::compile` (C, ~0 net, risk medium)
- **Now:** 96 lines, ~11 sequential phases; phase boundaries are paid for with
  full-AST deep clones (`:197`, `:213`, "to avoid borrow fight").
- **Owner:** `load → build_index → lower_and_hash`. **Removable:** ~0 (reorg); may
  **cut the clones** if ownership is restructured (perf upside, measure).
  **Compat risk:** medium — the cross-profile `using` re-application (`:245-249`)
  is order-sensitive. **Tests:** `compile()` canonical-hash stability before/after.
  **Benchmark:** compile wall + peak RSS on the largest fixture (the clones are the
  H-item to measure). **Token:** `APPROVED-CORE-EDIT: split-compile`.

### 10 — Remove `arc_hash` (F/G, ~6 LOC + wire, risk low)
- **Now:** a parallel-to-`decl_hash` array **never computed or read** — only
  `push(None)` in builders and threaded through the manual serde (`ir.rs:652`).
- **Owner:** delete field + wire slot. **Removable:** ~6 + a wire field. **Compat
  risk:** low but **it is a wire change** — must be version-tagged (the ir_stream
  version machinery exists). **Tests:** serde roundtrip golden (flag the wire
  break). **Token:** `APPROVED-CORE-EDIT: drop-arc-hash`.

### Strategic (above the top-10, gated)
- **[F] Twin hierarchy + twin hash (root cause).** Execute U2 (derive index from
  links) ⇒ delete `coherence.rs` (358) + one hash walk + three back-pointer checks
  + the duplicated prefix-parent walk. Biggest simplification in the crate; gated
  on the U2 decision; guarded by the frozen canonical-hash + merkle KAT corpus.
  **Token:** `APPROVED-CORE-EDIT: u2-derive-index-from-links`.
- **[G] `shared_state.rs` + `ipc` feature + `iceoryx2` (279 LOC).** Delete once an
  external check confirms no downstream `cdylib` consumes the `ipc` symbols.

## Long functions that must NOT be split (keep-whole)

- `canonical_hash.rs` `compute_subtree_hashes_with_kinds` (188), `prufer_from_forest`
  (312), `canonical_incidence_bytes` (445) + helpers — **hash byte-order is the
  contract**; the per-node `sort`, min-heap leaf order, and LE encodings are load-
  bearing.
- `hash_pass.rs` `compute_merkle_hashes` — merkle fold order.
- `tensor_csr.rs` `row_dot` (SpMV), `row_axpy` (SpMM), `finalize_coalesced`;
  `tensor_val.rs` `extract_ref_weight_scalar`; the two parallel-dispatch thresholds
  — **FP accumulation order** is the contract (serial≡parallel≡dense).
- `generators.rs` `SteinerSearch` / `bose_triples` / `combinations` / `binom` —
  cohesive combinatorial kernels.
- `ir_stream` `WireError::fmt` (a display table), the `apply_inner→…→commit`
  cluster (already correctly decomposed), `ir_writer::write_children` (strict error
  semantics are the point).
- `validate_sections` / `validate_hierarchy_index_coherence` **may** be split into
  per-check helpers **only if call order is preserved** (the first-firing error
  variant / report order is contractually tested).

## Performance notes (no measurement performed this stage — read-only)

No benchmarks were run (Stage 2 forbids production edits; the perf-relevant items
are all H — they need a bench that would accompany the actual refactor):
- `assembler.rs:979` per-section `body.to_vec()` (`Vec<Vec<u8>>` staging) — bench
  a flat staging arena before restructuring.
- `module_store::compile` per-AST `clone()` — bench compile wall + peak RSS.
- allocation-hot files (objective): `intern_pass` (34 sites), `generators` (23),
  `canonical_hash` (22), `resolve` (18) — candidates *if* a profile flags them,
  not before.

## Test gaps (pre-requisites for the refactors)

- No `spmv`/`spmm`/`finalize_coalesced` regression tests despite the tensor
  approval header requiring serial-vs-parallel + vs-dense — **write before touching
  any keep-whole tensor kernel.**
- `SectionKind`/`ElementKind` lack `try_from_wire(to_wire)==id` round-trip tests.
- Section-entry lacks a golden-image test.

## Own mistakes / retracted hypotheses (this session)

- **Initially read "ir_stream is a ~5000-LOC monster."** Wrong: it is ~1900 prod
  / ~3300 test; assembler.rs is 713 prod, not 2512. The module-size alarm was a
  raw-LOC artifact — corrected against the `#[cfg(test)]` split before ranking.
- **Considered `compute_degree_by_index` (8 params) a config-struct candidate.**
  The tensor sweep showed it is a correct generalization (enum matched once above
  the loop, monomorphised projection, no hot-loop dispatch, FP-order asserted) —
  dropped from the candidate list; kept whole.
- **`build_skeleton_forest` vs coherence prefix-parent walk** looked like a clean
  B dedup; flagged **high-risk** instead because the canonical_hash side is
  hash-determining — the shared helper must be proven bit-identical against the KAT
  or it is H, not B.

## Provenance

Branch `feat/core-generational-arenas` @ `67771c9`; rustc 1.97.1, x86_64-pc-windows-msvc.
Findings from 4 subagent sweeps (ir_stream, ir+resolution, tensor+wgpu,
module_store+writers+util+common) + scripted metrics; every finding grounded in
`file:line`. No source edited.
