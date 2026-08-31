# hymeko_core — simplification candidates

Read-only audit (Stage 2), branch `feat/core-generational-arenas` @ `67771c9`,
2026-08-31. Findings from four subsystem sweeps + objective metrics, each grounded
in `file:line`. **No production source was edited.** Classification per the audit
brief: A exact duplicate · B common mechanism / differing policy · C long
orchestration, splittable · D long hot kernel, keep whole · E needless public
surface · F derived-state duplication · G dead/legacy scaffolding · H not
decidable without measurement.

A generalization is proposed **only** when it meets all of: ≥2 real consumers,
identical pre/postconditions, identical output order, identifiable allocation
ownership, no floating-point-order change, no hot-loop dynamic dispatch.

## Top-10 ranked refactor candidates

Ranked by (removable LOC + risk-reduction) × low-risk × test-guarded. **Do not
implement in this stage** — each needs its own approval token (proposed).

| # | finding | cat | ~LOC | risk | token |
|---|---|---|--:|---|---|
| 1 | Delete `tensor/decomposition.rs` (whole module: hosvd/unfold/svd/entropy) — private, zero references repo-wide | G | 262 | none | `APPROVED-CORE-EDIT: drop-dead-decomposition` |
| 2 | Collapse `resolution/intern_pass.rs` owned/borrowed twin families (24 fns → 13) — the borrowed output lifetime is vacuous (payload all-owned) | A | ~150 | low | `APPROVED-CORE-EDIT: unify-intern-passes` |
| 3 | Delete `module_store/module_view.rs` — dead second impl of the index-build (divergence trap) + an unwired ref-validator | G | 45 | none | `APPROVED-CORE-EDIT: drop-module-view` |
| 4 | Retire `module_store` dead cluster: `program_cache`+`CompiledProgramCache`, `make_build_id`, `take_last_ir`, `writers/cbor_writer.rs::CborPayload` | G/F/E | ~74 | none | `APPROVED-CORE-EDIT: retire-module-store-scaffolding` |
| 5 | ir_stream section-entry layout: encode (`section.rs:246`) and decode (`:407`) hand-code the 32-byte offsets twice with bare literals — add `ENTRY_OFF_*` consts + a golden-entry image test | F | ~0 | low | `APPROVED-CORE-EDIT: section-entry-offset-consts` |
| 6 | Collapse the 7-fold `self.abandon(); ResyncRequired(StagingLimitExceeded{…})` in `assembler.rs:902-976` into one helper (state-machine copy-paste hazard) | B | ~15 | low | `APPROVED-CORE-EDIT: assembler-staging-guard-helper` |
| 7 | Consolidate 3 sibling-chain iterators — `DeclChildren` (panics, 2 callers), `ChildDecls` (total, 0 callers), coherence inline walk — onto the budgeted total form | B | ~15-30 | low | `APPROVED-CORE-EDIT: one-child-iterator` |
| 8 | Unify `const_resolve.rs` `eval_expr`/`eval_in_context` — identical arithmetic core (incl. div-by-zero), differ only in the `Ref` arm; pass a resolver closure | B | ~35 | low | `APPROVED-CORE-EDIT: unify-const-eval` |
| 9 | Split `module_store::compile` (96-line, 11-phase) into `load → build_index → lower_and_hash`; forces the per-AST `clone()` ownership question into the open | C | ~0 net | med | `APPROVED-CORE-EDIT: split-compile` |
| 10 | Remove `arc_hash` — a parallel-to-`decl_hash` derived array that is never computed or read (only `push(None)` + serde) | F/G | ~6 + wire | low | `APPROVED-CORE-EDIT: drop-arc-hash` (wire-versioned) |

### Strategic (highest architectural value, but gated on a decision or external check)

- **[F] Twin hierarchy + twin hash — the root cause.** The intrusive links
  (`DeclNode.parent/first_child/…`) and the path `Index` (`by_path`) encode the
  same tree independently; `compute_merkle_hashes` folds one, `canonical_program_hash`
  folds the other, and `ir/coherence.rs` (358 LOC) exists **only** to prove they
  agree (its own doc says "delete this module" once the index is derived). Executing
  user-decision **U2** (derive the index from the links) makes incoherence
  unrepresentable and lets the entire coherence module + one hash traversal + three
  back-pointer checks + the duplicated prefix-parent walk collapse. ~358+ LOC, but
  gated on U2 and guarded by the frozen canonical-hash KAT corpus. **This is the
  single biggest simplification in the crate.**
- **[G] `tensor/shared_state.rs` (279 LOC) + `ipc` feature + `iceoryx2` dep.**
  Deprecated, default-off, superseded by `ir_stream`, zero in-repo consumers.
  Deletion is blocked only by the external question: does any downstream `cdylib`
  binary consume the `ipc` symbols? Confirm, then delete in one change.

## Full findings by category

### G — dead / legacy scaffolding (highest confidence, pure subtraction)

- `tensor/decomposition.rs:1-263` — whole module, private, zero refs (~262 LOC).
- `module_store/module_view.rs:1-45` — dead index-build twin + unwired validator (~45).
- `tensor/shared_state.rs:1-279` — deprecated ipc scaffolding (gated deletion).
- `module_store/module_store.rs:61-86` `program_cache`+`CompiledProgramCache` — never read/written (~8).
- `module_store/module_store.rs:313-327` `make_build_id` — never called (~15).
- `writers/cbor_writer.rs:9-18` `CborPayload` — never constructed (~18).
- `util/pretty_print.rs:34-39` `fmt_hash_opt` — never called, dup of inline (~6).
- `ir/hash.rs:50` `hash_decl` (~18), `hash.rs:11` `path_bytes` (~9) — no consumer.
- `resolution/resolve.rs:224` `resolve_arc_anno` (~8), `resolve.rs:26` `Index::iter` (~3) — unused.
- `resolution/intern_pass.rs:173` `intern_ast_into` (borrowed) — no consumer (~3).
- `resolution/string_table.rs` `StringTable` — no reference in either workspace crate (~33; confirm no FFI consumer).

### A — exact duplicates

- `intern_pass.rs` owned/borrowed families (~150, top-2).
- `tensor/tensor_coo.rs:81-98` `new` vs `with_meta` — byte-identical (~9).
- `ir/common.rs:4-18` `ref_target`/`ref_sign` duplicate `SignedRefR::{target,sign}` (ir.rs:525) (~15).
- ir_stream `rd_u64` — one LE reader in ≥3 spellings (`section.rs:534`, inline in `envelope`, `version`) (~5-15).

### B — common mechanism, differing policy (unify only where the bar is met)

- `const_resolve` eval twins (~35, meets bar).
- assembler 7-fold staging guard (~15, meets bar).
- 3 sibling iterators (~15-30, meets bar).
- `canonical_hash::build_skeleton_forest` vs coherence prefix-parent walk (~10, **hash-sensitive — verify against KAT, else H**).
- `util/pretty_print.rs` `fmt_path`/`fmt_decl` shared path-join step only (~4).
- **Do NOT merge:** ir_stream discriminant tables (4 enums — policies differ: some error on unknown, `SectionKind` maps ≥MAX→Unknown). Instead **add** the missing `try_from_wire(to_wire)` round-trip tests for `SectionKind`/`ElementKind`.
- **Do NOT merge:** `ir::try_from_wire` kind/backlink vs coherence kind/record — different return contracts (`IrDecodeError` vs `Finding`).

### C — long orchestration, splittable (order-preserving)

- `module_store::compile` (96 lines, #9 above).
- `ir_stream/section.rs:355` `validate_sections` (177 lines) — extract `validate_entry`; **guard order is contractual** (first-firing error variant is tested).
- `ir/coherence.rs:143` `validate_hierarchy_index_coherence` (216 lines) — 6 independent checks; **preserve call order** (report order is contractual).

### E — needless public surface

- `assembler.rs:231` `MessageIdentity` — public + re-exported, no external consumer → `pub(crate)`.
- `module_store::take_last_ir` — pub, zero consumers.
- `ir/access.rs` — ~10 zero-consumer accessor methods (~60 LOC speculative boundary surface; low priority).
- `util/pretty_print.rs:87` bypasses the sealed accessor (raw `did.0`) while `ir_writer` uses the total accessor — **consistency fix**, not LOC.
- `ModuleKey(pub PathBuf)` / `PathKey(pub Vec<SymId>)` expose raw repr, contradicting the `Id.0→pub(crate)` sealing policy — low-value note.

### H — measurement required (do not act blind)

- `assembler.rs:979` per-section `body.to_vec()` (`Vec<Vec<u8>>` staging) — architecturally required (borrows transport buffer); flat-arena payoff is a bench question.
- `module_store::compile` per-AST `clone()` (`:197,213`) — borrow-checker workaround; cost depends on AST size × compile frequency.
- `interner.rs:22` the sole `unsafe` (`Box<str>`→`&'static`) — sound but unpinned; add a Miri run over `intern` growth.
- `resolve.rs:401` `validate_items` re-walk — may duplicate lowering's ref-resolution; depends on pipeline ordering.

## Keep-whole (do NOT split or unify)

FP-order / hash-order / cohesive kernels — any change gated on the frozen KAT
corpus or serial-vs-parallel regression tests:

- `canonical_hash.rs` `compute_subtree_hashes_with_kinds` (188), `prufer_from_forest` (312), `canonical_incidence_bytes` (445), and all header/tiebreak/skeleton helpers — hash byte-order is the contract.
- `hash_pass.rs:9` `compute_merkle_hashes` — merkle fold order.
- `tensor_csr.rs` `row_dot` (SpMV), `row_axpy` (SpMM), `finalize_coalesced`; `tensor_val.rs` `extract_ref_weight_scalar`; the two parallel-dispatch thresholds — FP accumulation order load-bearing.
- `generators.rs` `SteinerSearch`, `bose_triples`, `combinations`, `binom` — cohesive combinatorial kernels.
- `ir_stream` `WireError::fmt` (a display table), the `apply_inner→dispatch→begin/chunk/stage/commit` cluster (already correctly decomposed), `ir_writer::write_children` (strict error semantics are the point).
- `ir/ir.rs` `dispatch` module — already the single source of truth for the 62-bit slot; do not fork.

## Test gaps surfaced (pre-req for the above)

- No `spmv`/`spmm`/`finalize_coalesced` regression tests despite the approval
  header requiring serial-vs-parallel + vs-dense — **write before touching any
  keep-whole tensor kernel.**
- `SectionKind`/`ElementKind` lack `try_from_wire(to_wire)==id` round-trip tests
  (sibling `MessageType` has one).
- Section-entry has no golden-image test (envelope does).
