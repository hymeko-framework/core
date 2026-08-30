# Unified-IR platform-conformance gate

**Written:** 2026-08-30 04:11 CEST · **Base HEAD:** `cebc254` · **Branch:** main (local == origin)
**Authorisation:** none required and none used — work is confined to `hymeko_core/tests/**`, docs, reports, and plan artifacts. **No `hymeko_core/src`, no `Cargo.toml`, no dependency, no fixture under `data/**` was changed.** No `APPROVED-CORE-EDIT` token was granted or exercised.

Evidence classes are labelled throughout: **[M]** measured, **[S]** structurally verified (by a running test or by source inspection of an exact symbol), **[I]** inferred, **[H]** hypothesis, **[NI]** not implemented.

## 0. What this task did

Turned the architectural main line into a measurable V&V gate on the *existing*,
already-substantial unified-IR suite. It **did not** re-implement that suite: it
ran the baseline, audited the CBOR/`ir_stream`/CLI surfaces against the real
`cebc254` tree (never README claims), filled the three genuine gaps with
non-duplicative tests (product-CLI conformance, source-emission limitation), and
extended the durable requirement matrix. No production contract was implemented.

## 1. Baseline (unchanged tree) — recorded

| gate | result |
|---|---|
| `cargo fmt --all --check` | clean (0 hunks) **[M]** |
| `cargo clippy --workspace --all-targets -D warnings` | clean (exit 0) **[M]** |
| `cargo build` / `check` (all-features) | ok **[M]** |
| workspace test (all-features, no-fail-fast) | **523 passed / 1 failed / 1 ignored** **[M]** |
| — the 1 failure | `domain_transformations::…::test_load_meta_geometry` — the **documented 58/57 parser drift** (CORE.YAML `data/**`), pre-existing, left visible and unsilenced |
| doctests | core 1, hnn 0, wgpu 10 — all pass **[M]** |
| `ir_bench` targets | present (`ir_bench`, `ir_stream_bench`, `csr_bench`, `interning_bench`); smoke deferred to §7 |

Host (tools.yaml): Intel Core Ultra 9 275HX (24 logical), 63.4 GiB RAM, RTX 5080
Laptop 16 GiB, Windows 11. `cargo-llvm-cov 0.9.0` is installed (contra the stale
tools.yaml "not installed" note; the M2b coverage-baseline already used it).

## 2. The ten questions, answered

**Q1 — Which canonical-IR platform contracts already work?** **[S]** The
accessor layer (`UIR-ACC-*`, 12 rows PASS: checked-composite `node_for_decl`/
`edge_for_decl`, map-only `as_*`, total iteration, id-domain non-aliasing),
coherence detection (`UIR-COH-*`), canonical-hash stability (`UIR-HASH`/
`frozen_hash`, `UIR-IDENT`), hierarchy preservation, ordered-incidence and
sign/weight/base/value/annotation fidelity (`UIR-WL-*`, `verification/*`),
derived `HyperGraphView` construction and deterministic rebuild
(`validation/derived_views`), strict-consumer rejection of malformed IR
(`consumer_equivalence`, `derived_views`, `coherence`), real-corpus validation
(96 fixtures, `validation/corpus`), and the product CLI's three outcomes
(`UIR-CLI-01..03`, new).

**Q2 — Which are partial, absent, or defective?** **[S]** *Partial:* full source
emission (`UIR-EMIT-01` — skeleton only), the single-CLI invariant
(`UIR-CLI-04`). *Absent / NOT_IMPLEMENTED:* CBOR encode/decode/round-trip
(`UIR-CBOR-01..03`), lossless emit→reparse (`UIR-EMIT-02`), stable/generational
IDs (`UIR-M2C-01/03/04`), dynamic add/remove, transaction rollback, batch
atomicity, snapshot isolation, distributed convergence (`UIR-DYN-01..10`,
BLOCKED at M3/M4). *Defective (known, tracked):* `UIR-DEFECT-02` (merkle omits
incidence targets under adverse id order), `UIR-DEFECT-03` (serialized
`decl_hash` has no `algo_version`), `UIR-HASH-03` (`bases` covered by no identity
system) — all pre-existing, outside this task's authorised surface.

**Q3 — Can real `.hymeko` pass parser → ModuleStore → lowering?** **[M]** Yes.
The 96-fixture corpus lowers and its accessors agree with direct field access
(`validation::corpus::accessors_agree_with_direct_field_access_on_the_whole_corpus`,
`v1_corpus_digest_is_unchanged`); the `hymeko` CLI compiles a source end-to-end
(`UIR-CLI-01`); the in-memory `MemProvider` path is exercised
(`minimal_tests::test_module_store`).

**Q4 — Which derived views build exclusively through public APIs?** **[S]**
`HyperGraphView::from_ir` / `try_from_ir` (hnn) builds from `&Ir` through public
accessors (`validation/derived_views`, 431 LOC). COO/CSR are reachable from the
view (`tensor_coo` doc-links `HyperGraphView::from_ir`), but the **direct**
IR→COO/CSR public path is view-mediated in hnn, not a core one-call API —
recorded PARTIAL for "IR-sourced COO/CSR".

**Q5 — Is view rebuild from the same IR deterministic?** **[M]** Yes.
`try_from_ir` and `from_ir` agree over the corpus; workload construction is
bit-identical on repeat (`workload_coherence::repeated_workload_construction_is_bit_identical`)
and its canonical hash is stable.

**Q6 — Real lossless CBOR codec, or only a schema?** **[S] Only a schema.**
`CborPayload` is `#[derive(Serialize, Deserialize)]` scaffolding
(`writers/cbor_writer.rs`, 18 lines). There is **no encoder, no decoder**, and
the struct is **constructed nowhere** in the workspace (grep across all three
crates: only its definition and one comment in `string_table.rs`). `ciborium
0.2.2` appears in `Cargo.lock` **only transitively via Criterion** (`cargo tree
-i ciborium` → `ciborium ← criterion [dev-dependencies]`), i.e. it is not a
production dependency and not even a direct dev dependency.

**Q7 — Does `CborPayload` duplicate canonical or derived state?** **[S] Yes.**
Field classes: `ir` = canonical; `interned_strings` = canonical companion but a
full copy of the string table; `index` and `canon_hash` = **derived/cache**,
recomputable from `ir`; `root_path`, `imports` = transport. Two of six fields are
recomputable caches and one duplicates the interner — so a snapshot as declared
carries the model plus two derived caches plus a string copy. A future decoder
must **recompute-and-verify** `canon_hash` and rebuild/verify `index`, not trust
them (`UIR-CBOR-04`). `root_path: PathBuf` is environment metadata (an absolute
host path), not portable canonical data (`UIR-CBOR-05`).

**Q8 — Is `ir_stream` separated from canonical serialization?** **[S] Yes,
cleanly.** `ir_stream/` (envelope/codec/section/assembler/version/limits) contains
**zero references to any IR entity id** (`DeclId`/`NodeId`/`EdgeId`/`HyperArcId`)
and does not import `ir::ir::Ir` — its sections are CSR offsets and feature arrays
(`UIR-M2C-02`, PASS). It is bounded (`DecodeLimits`/`AssemblerLimits`), validated
(`ValidatedEnvelope::decode`, `try_from_wire`, no transmute), and versioned
(`SnapshotVersion`), with 87 in-module tests and five `ir_stream_bench` cells. It
is transport framing, not an IR serializer, and it does not leak IDs into the
wire.

**Q9 — How many user-facing CLI entrypoints exist across the repositories?**
**[S]** Four binaries; **two are product CLIs**: `hymeko` (core — minimal:
compile + pretty-print, no subcommands) and `pgraph` (graph —
`read`/`transform`/`solve`/`generate`). `hymeko_pgraph_dump` (graph) is a debug
CLI; `parser/src/main.rs` is a `Hello, world!` stub. The one-`hymeko`-CLI
invariant is **not yet met** (`UIR-CLI-04` PARTIAL); consolidation plan in
`docs/unified-ir-single-cli.md`. Examples (core `ir_depth_probe`; hnn/wgpu
probes) are dev targets, not product CLIs.

**Q10 — Exact approval-bound production change to follow?** See §6.

## 3. Tests added (non-duplicative)

| file | tests | role |
|---|---|---|
| `tests/unified_ir/validation/product_cli.rs` (new) | 3 | product `hymeko` CLI: success / usage / missing-file, via `CARGO_BIN_EXE_hymeko_core` (no recursive cargo) |
| `tests/unified_ir/validation/consumer_equivalence.rs` (+1) | 1 | `writer_emits_the_declaration_skeleton_but_not_annotations_a_known_limitation` — honest emission-limitation |
| `tests/unified_ir/validation/mod.rs` (+1 line) | — | register `product_cli` |

Core `mod` suite: **247 → 251 passed**, 1 failed (known drift), 0 ignored. No
existing test changed. No fake PASS was written for a NOT_IMPLEMENTED contract; no
test-only IR/transaction/delta framework was introduced. The single `#[ignore]`
discipline was not needed here — every added test is a real PASS or an honest
limitation characterisation with a live reproducer.

## 4. Requirement matrix delta

Extended `docs/verification/unified-ir/{requirements.md,traceability.csv}`
additively with 11 rows (families `UIR-CLI`, `UIR-CBOR`, `UIR-EMIT`). Matrix-wide
after: **47 PASS · 3 PARTIAL · 3 FAIL (known defects) · 13 NOT_IMPLEMENTED · 10
BLOCKED**. The task's requested vocabulary maps to the matrix's:
PASS=PASS/VERIFIED, PARTIAL=PARTIAL, FAIL=known-defect, NOT_IMPLEMENTED=PLANNED,
BLOCKED=BLOCKED.

- Evidence coverage (rows with a live test or an explicit inspection finding):
  every new row is backed by a named test or a cited symbol/grep.
- Implemented-normative satisfaction (PASS ÷ (PASS+PARTIAL+FAIL of *implemented*
  contracts)): unchanged for the pre-existing families; the new CLI family is 3/4
  PASS with the 4th an honest PARTIAL invariant.
- End-state normative satisfaction counts NOT_IMPLEMENTED/BLOCKED against the
  eventual platform: 23 of the ~76 rows are future work, openly marked.

## 5. Coverage

Command matches the M2b baseline denominator exactly:
`cargo llvm-cov --workspace --all-features --lcov -- --skip test_load_meta_geometry
--skip test_coo_construction_throughput`. **Production source was not changed, so
production touched-line coverage is N/A** (§ per the task). The added tests are a
CLI **subprocess** exercise (`main.rs` runs out-of-process and is therefore *not*
instrumented — `main.rs` coverage is unchanged) and a writer path already at
97.50 % (`ir_writer.rs`, M2b). **Measured [M]:** production line coverage
**84.93 %** (9475/11156), function 82.66 %, vs the M2b "after" baseline of
84.65 % — unchanged-to-marginally-higher (+0.28 pp, the writer-limitation test
touching a slightly different `IrWriter` branch), exactly as expected when no
production source changes. Artifact: `reports/coverage/platform-conformance.lcov`.

## 6. Proposed scope: `APPROVED-CORE-EDIT: canonical-cbor-codec` (NOT granted here)

- **Files:** `hymeko_core/src/writers/cbor_writer.rs` (add `encode`/`decode`),
  `hymeko_core/Cargo.toml` (promote `ciborium` from transitive-dev to a direct
  production dependency — a pinned-dependency event), a new
  `hymeko_core/tests/unified_ir/validation/cbor_roundtrip.rs`.
- **API:** `fn encode(ir, index, strings, imports) -> Vec<u8>`; `fn decode(bytes,
  &DecodeLimits) -> Result<CborPayload, CborError>` that (a) bounds every length
  before allocation, (b) rebuilds `index` and recomputes `canon_hash` from the
  decoded `ir` and rejects on mismatch, (c) stores a workspace-relative
  `root_path`. Reuse `ir_stream::DecodeLimits`; do not invent a second limit type.
- **Limits/schema:** add an explicit schema/version field (there is none today);
  cap decl/arc/section counts and total bytes.
- **Tests:** `cbor_decode(cbor_encode(x))` ≡ `parse(emit_hymeko(x))` on the corpus
  and the workload scales; fault-injection on truncated/oversized/hash-mismatched
  payloads; the `#[cfg(test)]`-free public API only.
- **Memory budget:** decode peak < 2× the decoded `Ir`'s heap; no per-incidence
  temporary allocation.
- **Rollback:** the codec is additive; `git revert` the commit. `CborPayload`'s
  shape is preserved for compatibility (plus the new version field).
- **Compatibility:** the JSON `SnapshotDto` view is unaffected; `ir_stream` frames
  the CBOR payload but is not modified.

## 7. Performance / memory

The existing `ir_bench` already covers construction, canonical hash, clone/serde,
and accessor-vs-direct lookup at scale; `ir_stream_bench` covers wire
validate/assemble/reject. Per the task, new cells are added only for genuinely
missing pipeline stages (source parse+lower on real fixtures; `IrWriter`
emission) — see §8 for what shipped. No single-shot timer is reported as a
benchmark; medians/IQR only. Ambient load was not exclusively controlled during
this session (a background coverage build ran); any crossover/performance figure
here is therefore labelled **provisional [H]** and not used for a ranking claim.

## 8. Files, and what actually shipped

(Filled at commit — see the commit message and `git show --stat`. All additive:
`tests/unified_ir/validation/product_cli.rs`, a +1 test in
`consumer_equivalence.rs`, a +1 line in `validation/mod.rs`, the requirement
matrix rows, `docs/unified-ir-single-cli.md`, this report, the coverage lcov, and
the plan bundle. No `src`, `Cargo.toml`, dependency, or `data/**` change.)

## 9. Defects & limitations found

- **[S]** `IrWriter` is skeleton-only (annotations/values/bases/incidences not
  emitted; arcs stub to a comment) → emit→reparse is not lossless in core today.
- **[S]** No CBOR codec; `CborPayload` duplicates derived state and leaks a host
  path.
- **[S]** Two product CLIs + a debug CLI + a stub `main` — the single-CLI
  invariant is unmet.
- **[S]** Pre-existing tracked defects `UIR-DEFECT-02/03`, `UIR-HASH-03` remain
  (outside authorised surface).
- The known `test_load_meta_geometry` 58/57 drift stays failing and visible.

## 10. Mistakes made during this phase

- Initially built the `hymeko` binary with `--bin hymeko` (the *lib* is named
  `hymeko`; the **bin** is `hymeko_core`). Corrected to `CARGO_BIN_EXE_hymeko_core`
  before writing the test.
- First aggregate of the background test output read the wrong file (the echo
  stream, not the filtered results) and under-counted; recomputed from the
  filtered log.

## 11. No-duplication evidence

The existing `tests/unified_ir` suite (~4200 LOC) already covers accessors,
coherence, hash, hierarchy, identity, derived views, and workload determinism; it
was searched before adding anything. The three new tests fill families with **no**
prior rows (grep confirmed no CLI, CBOR, or emission-completeness rows). The
matrix was **extended**, not forked. No graph-repo Steiner/Mycielski/… generator
was copied.

## HARD STOP

No M2c/M2d/M1, no stable-ID/mutation/transaction/CBOR *implementation*, no new
CLI/subcommand, no stream-to-IR application, no consumer-repo migration, no
dependency change, no backend optimisation. Next production step: §6.

## 12. Phases H–L (architecture refinement, duplication audit, profile, optimisation)

- **H (architecture):** the 9-layer ownership model, the abstraction gate, and the
  serialization/transport, parallel, single-CLI and forbidden-dependency
  boundaries are added to `docs/unified-ir-architecture.md`.
- **I (duplication):** `docs/unified-ir-duplication-and-profile-audit.md` classifies
  every candidate. **No TRUE/PERFORMANCE duplication with measurable runtime cost
  exists in editable (non-core) production code:** `HyperGraphView::from_ir` already
  uses the accessor layer; the hnn "arena" hits are a traversal bitset (false
  positive) and IR *construction* (intentional, no public mutation API yet); the
  CSR serial/Rayon split is the approved, bitwise-identical `csr-spmv-spmm-into`
  specialisation. The one architectural duplication (`CborPayload` embedding
  derived state) is unwired (zero runtime cost) and in locked core.
- **J (profile, from existing artifacts):** absolute costs show the dominant
  end-to-end cost is the numerical op (CSR SpMV 5.23 ms serial / 0.54 ms Rayon at
  4.2 M nnz), not IR construction/lookup (ns-scale). Optimising the accessor layer
  would optimise a non-bottleneck.
- **K (decision):** **no optimisation implemented** — none met the
  proved+authorised+high-value bar without touching locked core. The core-internal
  opportunity is delivered as the `APPROVED-CORE-EDIT: canonical-cbor-codec` patch
  plan (§6), not an edit.
- **L:** 0 production lines changed; no new abstraction/façade/entrypoint; only the
  existing product CLI is user-facing.
