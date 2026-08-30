# Duplication, profile, and optimisation-decision audit

**Written:** 2026-08-30 · **Base:** `cebc254` · read-only across `hymeko_core`,
`hymeko_hnn`, `hymeko_wgpu` and (read-only) sibling repos. No source changed.
Companion to `reports/2026-08-30-unified-ir-platform-conformance.md` (Phases I–L).

## Phase I — duplication classification

Line similarity is not counted; every row states the semantic reason.

| # | site(s) | duplicated responsibility | freq | alloc/mem | latency | correctness risk | classification | proposed abstraction | CORE.YAML | test | falsifier |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | `hnn/traversal/graph_traversal.rs:24–30` | `self.nodes[n.0]`/`self.edges[e.0]` bit-marks | per node/edge in a traversal | traversal's own `Vec<bool>`; no alloc in loop | O(1) | none | **FALSE POSITIVE** — not the IR arena, it is the visited set | none | non-core | traversal tests | n/a |
| 2 | `hnn/workloads.rs:679–763` | direct `ir.decl_nodes[..]` / `decl_to_*` writes | per decl at build | builds the `Ir` | build-time | none | **INTENTIONAL SPECIALISATION** — IR *construction*; there is no public mutation API yet | future core `IrBuilder`/transaction (M2d) | non-core, constructs core `Ir` | `validation::workload_coherence` | a public mutation API lands |
| 3 | `core/ir/lower.rs:119–201` | `decl_to_node/edge/arc[..] = Some(..)` | per decl at lower | builds the `Ir` | lower-time | none | **INTENTIONAL** — the canonical constructor itself | already the seam | **LOCKED core** | corpus + workload | n/a |
| 4 | `core/tensor/representations/tensor_csr.rs` serial + Rayon `spmv/spmm` | two bodies for one numerical contract | per op | none extra | measured crossover (`PAR_SPMV_NNZ_THRESHOLD=81920`) | Rayon path is **bitwise-identical** by contract | **INTENTIONAL SPECIALISATION** — approved `csr-spmv-spmm-into`; already unified behind `*_into` | already `*_into` | **LOCKED core (approved)** | csr regression (serial vs parallel vs ref) | a >3% regression or any bitwise diff |
| 5 | `core/writers/cbor_writer.rs` `CborPayload{ir,index,interned_strings,canon_hash,imports}` | embeds derived (`index`,`canon_hash`) + a string-table copy beside canonical `ir` | **0 — never constructed** | 0 runtime | 0 | none today | **ARCHITECTURAL DUPLICATION (latent, unwired)** | codec that recomputes/verifies `index`+`canon_hash` on decode | **LOCKED core** (+ Cargo.toml dep) | `cbor_roundtrip` (future) | measured on a wired codec |
| 6 | 13× `collect::<Vec<_>>` in core/hnn | collect-then-use | setup, not hot loop | one-shot allocations at view build | negligible | none | mostly **FALSE POSITIVE** — build-time, not per-incidence | none | mixed | existing | n/a |

**No TRUE DUPLICATION and no PERFORMANCE DUPLICATION with measurable runtime cost
was found in editable (non-core) production code.** The M2b migration already
routed the core consumers (`ir_writer`, `pretty_print`) and the derived-view
builder (`HyperGraphView::from_ir`, which uses the map-only `as_node` accessor)
through the accessor layer; the CSR serial/parallel split is a measured, approved
specialisation, not duplication.

## Phase J — profile (from existing artifacts; no grid rerun)

Reused: `reports/2026-08-25-published-baseline-benchmark.md`, the saved
`target/criterion/*` cells (`ir_construction`, `ir_canonical_hash`, `ir_clone`,
`ir_serde`, `csr_spmv`, `csr_spmm`, `conv_*`), and `ir_stream_bench`.

Absolute costs (medians; the point is absolute, not %):

| stage | scale | cost | note |
|---|---|---|---|
| accessor vs direct decl lookup | n≤10⁴ | sub-ns–ns per lookup | `m2a_decl_lookup` — the checked accessor's guard is a few ns; not a bottleneck |
| CSR SpMV serial | 1k→4.2M nnz | 0.53 µs → 5.23 ms | `csr_bench` (`8b9d82d`) |
| CSR SpMV Rayon | 262k→4.2M nnz | 44 µs → 537 µs | crossover at `PAR_SPMV_NNZ_THRESHOLD=81 920`; up to 9.7× at 4.2M |
| canonical hash / clone | per saved cells | (criterion-stored) | whole-graph passes; O(decls+incidences) |

**Dominant end-to-end cost is the numerical operation (CSR SpMV/SpMM), not IR
construction or lookup** — the latter are ns-scale per element, the former
ms-scale at 10⁶ nnz. Optimising the accessor layer would be optimising a
non-bottleneck (the §3-forbidden "defensive optimization"). This is confirmed by
absolute magnitude, not inferred.

## Phase K — optimisation decision

**No source optimisation is implemented this phase.** None met the full bar
(outside protected paths *and* proved high-value *and* removes real duplicated
work/allocation *and* fully A/B-verifiable):

- The editable (hnn/wgpu) production code has no measurable duplicated work to
  remove (Phase I rows 1,2,6 are false-positive/intentional).
- The one architectural duplication (row 5, `CborPayload`) is **unwired** (zero
  runtime cost) and lives in **locked core** — it is a *codec-design* constraint,
  not a present optimisation.
- The CSR parallelism (row 4) is already approved, unified, and measured.

Per the task, a would-be optimisation inside protected core is delivered as a
**patch plan + approval-token proposal, not an edit**:

> **`APPROVED-CORE-EDIT: canonical-cbor-codec`** — smallest surface:
> `writers/cbor_writer.rs` (+`encode`/`decode`), `Cargo.toml` (promote `ciborium`
> from transitive-dev to direct prod), new `tests/.../cbor_roundtrip.rs`. Contract:
> decode bounds-before-alloc, recomputes `canon_hash` and rebuilds `index` and
> rejects on mismatch, stores workspace-relative `root_path`, adds a schema-version
> field. Memory gate: decode peak < 2× decoded `Ir` heap; no per-incidence temp.
> Rollback: one revert. Full scope in the conformance report §6.

## Phase L — reuse & complexity verification

Trivially satisfied: **0 production lines changed**; no new abstraction, factory,
façade, `*_v2`/`*_new`, or helper crate; no library functionality moved into a
CLI; only the existing product CLI remains user-facing; no `Box<dyn Trait>` added
to any numerical loop; no enum dispatch duplicating generic dispatch. The three
new tests are validation-layer additions that call public APIs (and the CLI as a
subprocess); they add no production duplication.

### Intentional duplication that remains, and why

- **CSR serial vs Rayon** (`tensor_csr.rs`): merging them would destroy the
  measured crossover control and the bitwise-serial guarantee. Kept separate by
  the `csr-spmv-spmm-into` contract.
- **Workload/lowering direct arena writes** (`workloads.rs`, `lower.rs`): IR
  *construction* legitimately writes the arena; there is no public mutation API to
  route through yet (that is M2d). Merging into a premature builder now would be
  the single-implementation abstraction the gate forbids.

---

## Addendum (2026-08-30) — scope correction: this audit was core-workspace-only

The Phase I conclusion above — *"No TRUE DUPLICATION and no PERFORMANCE DUPLICATION
with measurable runtime cost was found in editable (non-core) production code"* — is
**correct but scope-limited, and must not be read as a framework-wide claim.** It
audited only the **core workspace** (`hymeko_core`, `hymeko_hnn`, `hymeko_wgpu`). It did
**not** cover the canonical sibling repositories `hymeko/graph` (`hymeko_graph`,
`hymeko_pgraph`) or `hymeko/tensor` (`hymeko_tensor`, `hymeko_clifford`, `hymeko_spline`,
`hymeko_numeric_vv`), nor any Nagare instance.

The cross-repository I0 audit
(`reports/2026-08-30-hnn-nagare-tensor-integration-audit.md`, 2026-08-30) covers that
wider scope and finds duplication **does** exist across the framework — concentrated in
Nagare, not in the core workspace:

- **≈ 17 289 LOC of exact / stale-vendor copies** in the Nagare instances
  (`nagare_github/vendor/hymeko_graph` 15 421 LOC + `vendor/hymeko_clifford` 776 LOC +
  the zero-commit non-git `nagare` skeleton 1 092 LOC).
- A handful of **B/C overlaps**: `hymeko_graph::hyper_operator` vs
  `hymeko_hnn::message_passing` (same signed-incidence math, different contract —
  differential-test-gated); `compute_bipartite_degrees` vs the core degree seam; a
  Chebyshev recurrence duplicated within the tensor workspace; a fixture-scoped LCG in
  three crates.
- A **gap** (not a duplication): no canonical dense `matmul`/`linear_forward` owner
  exists; hnn hand-rolls `X·Θ` 4× internally and Nagare once.

**The core-workspace verdict of this document stands unchanged** — it never claimed
cross-repo scope. This addendum records that the "no duplication" statement is bounded
to `core/{hymeko_core,hymeko_hnn,hymeko_wgpu}`, and points to the I0 audit and
`docs/hymeko-kernel-ownership.md` / `docs/hymeko-kernel-migration.md` for the framework
picture and the consolidation plan.
