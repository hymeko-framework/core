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

## Resolved pipeline findings (2026-09-23)

`APPROVED-CORE-EDIT: core-pipeline-encapsulation-cleanup` supersedes the
pipeline findings in this historical audit. Current owners are:

- `resolution/intern_pass.rs`: one owned AST traversal, used by `intern_ast`
  and `intern_ast_into`.
- `resolution/const_resolve.rs`: `ConstReferences::evaluate`, with dependency
  and evaluated-table lookup strategies; no table clone during substitution.
- `ModuleStore::compile`: module linking and per-module alias environments.
- `resolution/resolve.rs`: `ResolutionWalk` handles headers, annotations,
  bases and arc references in the same scope/order for validation and lowering.
- `ir/ir.rs`: `IrConstruction` exclusively borrows the arenas while building;
  checked operations commit links/records/bindings together.
  Runtime leaf-value batches use the IR's checked mutation operation.
- `ir/access.rs`: one bounded `ChildDecls`; Nagare's existing `DeclChildren`
  type name is an alias, and compatibility methods delegate.

Removed: the unused module-view and tensor-decomposition modules, inert program
cache and build-ID helper, duplicate intern/constant-evaluation/validation/child
iterator bodies, and uncalled forwarding entry points. The fault-injection
helper that depended on invalid public bindings was removed; tests now corrupt
records after valid construction. The public low-level arena fields remain a
compatibility surface for the existing Graph, Transformation and Nagare builders;
compiler construction does not expose them through its mutation capability.
The obsolete Merkle pass, its compiler call and the `doc_hash`, `decl_hash`,
`arc_hash` fields are removed, including their wire representation and C/C++
accessors. The DSL/description is authoritative; CBOR carries the current IR for
M2M exchange. Historical payload compatibility is not retained.
`semantic_model_digest` is the full-model identity owner; structural hashing
remains distinct and its algorithm is unchanged. `CompiledModel::semantic_digest`
retains derived contributions outside the serialized IR. Checked leaf-value
updates reuse paths, concrete-record contributions and the structural hash;
only edited declarations are re-encoded by the same full-digest implementation.
The final sorted contribution fold, immutable model copy and existing validation
remain linear; topology changes/recompilation build fresh derived state.
The remaining findings below
are historical unless explicitly re-verified; no performance claim is made here.

### Local follow-up, 2026-09-24

- Assembler test-runner follow-up (03:02): `AssemblerRunner` owns producer and
  consumer, real wire admission, resync checks, accepted-message and transaction
  execution. Unit/model/taxonomy modules share it; free test apply removed.
  One identical replay test removed; all its assertions survive. 36 tests pass,
  one taxonomy probe remains ignored; library/test Clippy passes. Production
  source prefix is byte-identical after newline normalization. Assembler test
  marks 607 -> 324; total checkout 459 / 9279 -> 448 / 8996. Source file itself
  grows by 49 lines for the runner and explicit ownership; no size/speed claim.

- Subsequent endian-reader cleanup: `ir_stream/reader.rs` now owns the private
  `LittleEndianReader` and `LittleEndianValue` contract. Envelope, section-table,
  typed section-body and snapshot-version reads share one generic copy operation.
  The old scalar/version readers and version-local closure are removed. Public
  API/wire representation unchanged; 54 targeted tests and library Clippy pass.
  Same-scope token result: 463 / 9599 -> 461 / 9546 marked runs / lines, including
  the new module; stored baseline untouched.
- Removed `ir/common.rs` and its two public free functions. Both integration-test
  consumers now call the existing `SignedRefR::target` / `sign` methods; the
  framework/Nagare source search found no external callers of the removed API.
- The seven assembler staging-limit branches share the private
  `TransactionAssembler::staging_limit_exceeded` method. Guard order, diagnostic
  text and retained committed/replay state are unchanged. Strengthened existing
  limit tests verify state cleanup and that rejected messages do not enter replay.
- Resolved incidence/access follow-up (2026-09-24 02:44 +02:00): the 14 shared
  `IncidenceIndex` / `IncidenceView` inherent methods now have one declaration
  template with explicit owned/borrowed result lifetimes. `IncidenceIndexData`
  remains the single numerical implementation. Four typed access families share
  iteration/count/containment declarations; typed lookups and backlink policies
  stay explicit. Assembler resync cleanup has one boundary in `apply`; reset
  reuses `abandon`. The staging-limit helper now only constructs the outcome.
  Verification: 37 unit + 34 integration tests pass; 2 benchmarks remain ignored;
  library Clippy passes. Same-scope token delta: 461 / 9546 -> 459 / 9279, or
  267 fewer marked lines. This measures authored source, not expanded machine code.
  Remaining assembler marks: 56 production getter lines + 607 test lines; no
  duplicate assembler state machine identified. Access marks fall 224 -> 113:
  typed lookups/backlink checks and total hierarchy accessors remain.
  The symbol parser does not expand private macros: 643 extracted names versus
  656 previously is an indexing limitation, not removal of 13 public APIs.
- Same-detector task delta: 462 / 9588 to 463 / 9599 marked runs / lines. The increase
  is entirely in integration-test matches after switching to method calls (+9 in
  `ir_transformation_tests.rs`, +2 in the unchanged `test_import_graphs.rs`). The
  deleted short duplicates were below the 60-token threshold. This is not a claimed
  drop in the machine metric. Core source-folder marked lines stay at 2059,
  including embedded tests. Public symbol count decreases from 658 to 656.

## Top-10 ranked refactor candidates

Ranked by (removable LOC + risk-reduction) × low-risk × test-guarded. **Do not
implement in this stage** — each needs its own approval token (proposed).

| # | finding | cat | ~LOC | risk | token |
|---|---|---|--:|---|---|
| 1 | Unused tensor decomposition removed | resolved | - | verified | `core-pipeline-encapsulation-cleanup` |
| 2 | One owned intern traversal; duplicate bodies removed | resolved | - | verified | `core-pipeline-encapsulation-cleanup` |
| 3 | Unused module-view index path removed | resolved | - | verified | `core-pipeline-encapsulation-cleanup` |
| 4 | Inert module-store cache/build-ID scaffolding removed; CBOR payload is now live and is retained | G/F/E | historical | re-check | pipeline cleanup |
| 5 | ir_stream section-entry layout: encode (`section.rs:246`) and decode (`:407`) hand-code the 32-byte offsets twice with bare literals — add `ENTRY_OFF_*` consts + a golden-entry image test | F | ~0 | low | `APPROVED-CORE-EDIT: section-entry-offset-consts` |
| 6 | Assembler resync outcomes share one cleanup boundary | resolved | - | verified | user-approved follow-up 2026-09-24 |
| 7 | One bounded child iterator; compatibility entry points delegate | resolved | - | verified | `core-pipeline-encapsulation-cleanup` |
| 8 | One arithmetic evaluator with reference strategies | resolved | - | verified | `core-pipeline-encapsulation-cleanup` |
| 9 | Split `module_store::compile` (96-line, 11-phase) into `load → build_index → lower_and_hash`; forces the per-AST `clone()` ownership question into the open | C | ~0 net | med | `APPROVED-CORE-EDIT: split-compile` |
| 10 | Obsolete IR hash caches and their wire fields removed | resolved | - | verified | pipeline cleanup |

### Strategic (highest architectural value, but gated on a decision or external check)

- **[F] Hierarchy authority:** both current identity algorithms derive paths from
  the IR hierarchy. The compiler validates its resolution index at the lowering
  boundary. The obsolete cached Merkle pass has been deleted.
- **[G] `tensor/shared_state.rs` (279 LOC) + `ipc` feature + `iceoryx2` dep.**
  Deprecated, default-off, superseded by `ir_stream`, zero in-repo consumers.
  Deletion is blocked only by the external question: does any downstream `cdylib`
  binary consume the `ipc` symbols? Confirm, then delete in one change.

## Full findings by category

### G — dead / legacy scaffolding (highest confidence, pure subtraction)

- `tensor/shared_state.rs:1-279` — deprecated ipc scaffolding (gated deletion).
- `writers/cbor_writer.rs:9-18` `CborPayload` — never constructed (~18).
- `resolution/string_table.rs` `StringTable` — no reference in either workspace crate (~33; confirm no FFI consumer).

### A — exact duplicates

- `tensor/tensor_coo.rs:81-98` `new` vs `with_meta` — byte-identical (~9).
- `ir/common.rs` reference helpers: removed; all discovered callers use `SignedRefR` methods.
- ir_stream scalar readers: resolved by the shared generic `LittleEndianReader`.

### B — common mechanism, differing policy (unify only where the bar is met)

- assembler staging-limit outcome construction and resync cleanup: each has one owner.
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

## Keep-whole (do NOT split or unify)

FP-order / hash-order / cohesive kernels — any change gated on the frozen KAT
corpus or serial-vs-parallel regression tests:

- `canonical_hash.rs` `compute_subtree_hashes_with_kinds` (188), `prufer_from_forest` (312), `canonical_incidence_bytes` (445), and all header/tiebreak/skeleton helpers — hash byte-order is the contract.
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


## Completed WGPU and declaration traversal consolidation, 2026-09-24

- Eight operation builders now borrow one `OperationFactory` context with a single
  `OperationLimits`; uniform allocation/upload has one implementation.
- `OperandCheck` replaces six copies each of supplied-value and shape-error helpers;
  `GpuTensor::binding` replaces six free binding-entry helpers.
- `DeclarationFields` defines node/edge fields for both `AstInterner` and `ConstValues`.
  Each pass has one declaration walk; replaced free lowering/substitution helpers are gone.
- Verification: 11 resolution unit + 51 integration + 36 actual GPU tests passed;
  both libraries and their test targets pass Clippy with warnings denied.
- Same detector: 448/8996 -> 449/8543 marked runs/lines across 185 files. Lines fall
  by 453; contiguous run count increases by one. These are token-clone spans.
- Incidence index versus prepared-plan row kernels remain an open, separate finding.
  Existing GPU test scaffolding and benchmark fixtures also remain in the detector.


## Completed incidence transpose consolidation, 2026-09-24

The index/prepared-plan row-kernel finding above is now resolved. One private
`IncidenceIndexData::transpose_row` owns the ordered multiply/add fold. Direct,
prepared serial and Rayon paths use it through `IncidenceCoefficients`, implemented
by existing `IncidenceView` and `IncidenceOperatorPlan` owners. Serial output traversal
also has one implementation; prepared coefficients remain cached, topology borrowed.

26 correctness tests passed, including independent expected values, cancellation/FMA
controls, NaN/Inf handling, output tails and short-buffer behavior; Clippy passed.
The token scanner is unchanged at 449 runs / 8543 marked lines / 185 files. Neither
this production overlap nor the new test scenarios changes its marked spans:
incidence.rs remains 69; incidence_precompute_profile.rs remains 23. This is a
semantic consolidation below the detector's matching threshold, not a measured
source-size or performance reduction. Production source net +25 lines for the trait
and shared methods; test source net +157 lines for the runner and regression cases.
