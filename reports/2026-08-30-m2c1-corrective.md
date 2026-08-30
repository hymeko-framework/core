# M2c.1 corrective — representation-independent dense view construction

**Report date:** 2026-08-30 22:17 +02:00
**Change:** `fix(hnn): make dense view construction id-independent`
**Approval token:** `APPROVED-CORE-EDIT: dense-view-index-compute-degree` (corrective scope: `hymeko_hnn` production/tests/benches, PoW/integration tests, docs/reports/V&V only)
**Corrective base HEAD:** `30921cedae3b675e121721e9faeac3709ee63806`
**A/B baseline:** `fbba816` (parent of the M2c.1 line; pre-dense view)

This corrective repairs four defects in the M2c.1 commit `30921ce`, documented in
§18 of `reports/2026-08-30-dense-view-index-boundary.md` (additive correction) and
in the V&V rows `UIR-M2C1-04/06/08..12`.

---

## 1. What was wrong (reproduced, not asserted)

| # | Defect in `30921ce` | Reproduction |
|---|---|---|
| D1 | `dense_node_id(nid) = NodeId::new(nid.as_usize())` **fabricated** a canonical id by arithmetic; `dense_edge_id` likewise. | `dense_node_id_reads_provenance_not_arithmetic`: set provenance `[3,19,700]`, the fabricating impl returns `NodeId(0/1/2)` → fails against the old code, passes against the fix. |
| D2 | The builder indexed by canonical raw: `offsets[entry.e.raw()+1]`, `cursor[entry.n.raw()]`, `sort_by(e.raw(),n.raw())`, `from_u32(entry.n.raw() as u32)`. A representation leak (assumes `raw()==dense position`). | Mutant "builder uses raw index" is now **structurally impossible**: the CSR builders take `&[DenseInc<V>]` (dense-typed); `entry.e`/`entry.n` are dense newtypes with no `.raw()`. The 12 raw sites are gone. |
| D3 | The "≤3% runtime-neutral" claim rested on loop-shape reasoning with **no A/B run**. | Real A/B below: measured, not asserted. |
| D4 | "12.51 MiB saved" was **gross** (did not subtract the new provenance vectors). | Re-measured net: 9.92 MiB. |

Additionally, the workload fixture `HyperSpec::build()` previously minted canonical
ids to seed a view (false provenance). It now delegates to `from_ir(self.ir(), …)`.

## 2. New canonical↔dense architecture

`try_from_ir` (in `hymeko_hnn/src/traversal/hypergraphview.rs`):

1. **Provenance** — `node_canonical: Vec<NodeId>` / `edge_canonical: Vec<EdgeId>` collected from `ir.node_records()` / `ir.edge_records()`; dense position `i` owns `node_canonical[i]`. `dense_node_id`/`dense_edge_id` **read** these vectors.
2. **Checked maps** — transient `FxHashMap<NodeId, DenseNodeIndex>` / `FxHashMap<EdgeId, DenseEdgeIndex>` (rustc-hash — already an HNN dependency; **no new dep**), built from the same enumeration.
3. **Translate-before-build** — `translate_to_dense` maps every incidence canonical→dense **before** any sort/offset/cursor/fill, into `DenseInc<V>`. A lookup miss is `ViewBuildError::{NodeNotInView, EdgeNotInView}{raw}` — no panic, no silent drop, no `unwrap_or_default`.
4. **Dense-only CSR** — `sort_reduce_pairs` / `build_csr_edge_to_nodes` / `build_csr_node_to_edges` operate on `DenseInc<V>`; a canonical raw index cannot re-enter the builder.
5. **Transient maps** — the two `FxHashMap`s drop at end of construction; no resident reverse map is added (only the two provenance `Vec`s persist).

Correct under **non-contiguous, non-zero-based, permuted, or later generational**
canonical ids: `translate_maps_non_contiguous_ids_to_dense_positions` and
`permuted_provenance_keeps_semantics_and_real_ids` pin it.

## 3. Raw-index sites removed (exact)

Corrective diff `30921ce → HEAD` on the builder removed **12** canonical `.raw()`
sites: `par_sort/sort_unstable_by_key(e.raw(),n.raw())` (2 lines), dedup
`last.e.raw()==inc.e.raw()` / `last.n.raw()==inc.n.raw()` (2), `offsets[e.raw()+1]`
(1), `offsets[n.raw()+1]` (1), `cursor[n.raw()]` read+write (2),
`from_u32(n.raw())` / `from_u32(e.raw())` (2), and the two `entry.{n,e}.raw() <=
u32::MAX` overflow checks (2). File-level `.raw()` count fell **19 → 9**; the **9
residual are diagnostics/doc only** (5 in `ViewBuildError` Display, 2 in the
translate error *values* `NodeNotInView/EdgeNotInView{raw}`, 2 in doc comments).
**Zero** `.raw()` remain as an array index or sort key.

## 4. Provenance & error API

- `pub node_canonical: Vec<NodeId>`, `pub edge_canonical: Vec<EdgeId>` — dense position → canonical id. `node_decl`/`edge_decl` docs corrected to "dense position → declaration".
- `ViewBuildError::NodeNotInView { raw: usize }` / `EdgeNotInView { raw: usize }` — bounded numeric metadata only; `Display` arms added; propagated via `?`, never unwrapped.
- `DenseNodeIndex::from_u32` visibility **reviewed, kept `pub const`**: it is the sanctioned reconstruct-from-`u32` path, and the core boundary test (`test_dense_view_boundary.rs`, a legitimate cross-crate consumer) reconstructs dense coordinates with it; it carries a doc warning that the value is a view-local coordinate range-checked at construction.

## 5. Tests, mutants, coverage

**Tests (this build):** hnn lib **106 pass / 1 ignored**; core integration **260 pass / 1 known-fail / 1 ignored**; PoW-0 **11/11**; doctests hnn **2**, core **5**. The one known-fail is `test_load_meta_geometry` (58/57 geometry drift) — pre-existing, **left visible, not silenced**.

New tests: `dense_node_id_reads_provenance_not_arithmetic`,
`permuted_provenance_keeps_semantics_and_real_ids`,
`corrective_tests::{translate_reports_missing_node_mapping_typed_no_panic,
translate_reports_missing_edge_mapping_typed_no_panic,
translate_maps_non_contiguous_ids_to_dense_positions}`, and the ignored
`memory::large_view_net_memory_accounting`.

**Fault-injection mutants** (each caught by a named test, restored byte-identical):

1. fabricate `NodeId::new(dense)` in `dense_node_id` → **`dense_node_id_reads_provenance_not_arithmetic` FAILS**.
2. builder canonical raw index → **structurally prevented** (builders take `DenseInc<V>`; `entry.{e,n}` have no `.raw()`).
3. swap node/edge maps in `translate_to_dense` → **`translate_maps_non_contiguous_ids_to_dense_positions` FAILS**.
4. silent skip on a map miss (`if let Some(..)` instead of `?`) → **`translate_reports_missing_node_mapping_typed_no_panic` FAILS**.

**Coverage (after, workspace `llvm-cov`, all tests contributing):**
`hypergraphview.rs` **74.09% region / 72.19% line**; `dense_index.rs` **87.84% /
86.00%**; `workloads.rs` 21.48% / 27.27% (fixture generators mostly unexercised — the
changed `build`/`from_design` paths are covered via `sample_view`/memory test);
workspace **TOTAL 62.16% region / 64.06% line**. Baseline `fbba816` has no dense
module, so a same-file line-delta is undefined; the dense-specific coverage is
net-new (not fabricated against a nonexistent baseline).

## 6. CPU / Rayon / GPU correctness

PoW-0 pins it end to end: `pow0_cpu_spmm_serial_and_rayon_agree_at_multiple_feature_dims`
(serial == Rayon), `pow0_gpu_resident_spmm_activation_matches_cpu` and
`pow0_repeated_gpu_ops_reuse_buffers_without_changing_the_result` (GPU == CPU, buffer
reuse stable), and `pow0_canonical_identity_is_coherent_frozen_and_deterministic`
(frozen canonical hash `00d4c01a…` unchanged). Both incidence orientations agree
(`both_orientations_agree`) and rebuild is bit-identical
(`deterministic_rebuild_is_identical`).

## 7. Performance — real A/B + A/A null

Baseline `fbba816` built in a **separate detached worktree** (short path `D:/abwt` —
the deep scratchpad path broke blake3's MASM `A1009 line too long`; the work tree was
**never reset**). Timing = `HyperGraphView::from_ir` on the large spec (nodes=200 000,
edges=100 000, avg_cardinality=16, seed 0xBEEF, ≈1.60M incidences), 25 samples,
median+IQR, alternated A/B ×3 + A/A null:

| binary | median | IQR |
|---|---:|---|
| AFTER #1/#2/#3 | 102.51 / 102.24 / 102.15 ms | ±~1 ms |
| BASE (fbba816) #1/#2/#3 | 92.18 / 92.64 / 93.78 ms | ±~1 ms |
| A/A null (AFTER twice) | 104.27 / 102.48 ms (Δ≈1.8 ms) | noise floor |

**Result: construction ≈ +10.1% vs fbba816 — exceeds the 3% gate.** Isolated to the
new correctness cost: 2 provenance `collect()`s, 2 `FxHashMap` builds (300 k inserts),
`translate_to_dense` (≈1.60M checked lookups). Hot paths (traversal, SpMM/degree
kernels) consume the **same** dense arrays and are unchanged (PoW-0 parity holds).
**Provisional:** ambient CPU ≈45.7% during the A/B (machine not quiet); the median
gap (≈9.4 ms) is ~5× the A/A noise (≈1.8 ms), so the *sign* is trustworthy, the
*magnitude* carries host-load uncertainty. The baseline predates the dense module, so
this measures M2c.1 **+** corrective vs the pre-dense view, not the corrective alone.
The ≤3% neutrality claim is **withdrawn**; the ~10% one-time construction cost is the
accepted price of representation-independent provenance + checked translation.

## 8. Memory — gross vs net

Measured on this build (`large_view_net_memory_accounting`, real allocator
capacities, 1 599 950 incidences):

| quantity | bytes | MiB |
|---|---:|---:|
| gross saving (dense incidence arrays u64→u32, ×2 orientations) | 12 799 600 | 12.21 |
| **new** provenance (`node_canonical`+`edge_canonical`) | 2 400 000 | 2.29 |
| **NET saving** | 10 399 600 | **9.92** |

Formula `8·inc − 8·(nodes+edges)` = 10 399 600 B — exact. Peak RSS of the run stays
far under the 16 GB cap (a single ~1.6M-incidence view; incidence arrays ≈12.2 MB).

## 9. Files touched

Production (HNN, lockdown none): `hymeko_hnn/src/traversal/hypergraphview.rs`,
`hymeko_hnn/src/workloads.rs`. Tests: `hymeko_core/tests/test_tensor_representations/
test_dense_view_boundary.rs` (+ corrective tests + memory module). Docs/V&V: this
report, §18 addendum in `reports/2026-08-30-dense-view-index-boundary.md`,
`docs/verification/unified-ir/traceability.csv` (+5 rows, 2 corrected), plan bundle
`docs/plans/2026-08-30-m2c1-corrective/`. **No** `hymeko_core/src` change, **no**
`Cargo.toml`/`Cargo.lock`, **no** dependency change. `deliverables/`,
`reports/2026-08-26-ifac-geoff-chase-materials.md`, `tools/build_ifac_materials.ps1`
untouched. Temporary `examples/ab_construct.rs` and the `D:/abwt` worktree removed.

## 10. Compatibility impact

`HyperGraphView` gains two `pub` fields (`node_canonical`, `edge_canonical`);
`ViewBuildError` gains two variants. `dense_node_id`/`dense_edge_id` change from
arithmetic to a provenance read (same signature, corrected semantics). No public
signature of the core seam changed; `compute_degree_by_index` is used **unchanged**.

## 11. Own errors, rejected hypotheses, unproven claims

- **My prior M2c.1 report was wrong on three counts** (fabricated `dense_node_id`, canonical-raw construction sold as "intentional", ≤3%/12.51 MiB unmeasured). Corrected here and in §18 additively, not by rewriting history.
- **Rejected:** replacing the `FxHashMap` with a `Vec`-indexed-by-`raw()` translation table — cheaper, but it re-introduces the `raw()==dense` assumption the corrective exists to kill (breaks under generational ids). Kept the hash map as the representation-independent choice; its cost is the measured ~10%.
- **Unproven / provisional:** the A/B magnitude (machine at ~45.7%); the *sign* of the regression is solid (~5× the A/A null), the exact percent is not. Baseline `fbba816` predates the dense module, so the A/B bundles M2c.1 with the corrective.

## 12. Readiness for `APPROVED-CORE-EDIT: canonical-generational-arenas`

**Ready.** The view no longer assumes anything about canonical id layout: provenance
is stored, translation is a checked map, a miss is a typed error. When canonical
`Id`s become generational, `node_records()`/`edge_records()` keep yielding
`(Id, &Rec)` and construction is unaffected; `UIR-DYN-04` (stale-id rejection) is the
one thing still requiring the generational key itself. `Id::new` visibility is
**unchanged** — the fixture's synthetic provenance in `from_design` is documented
benchmark-only and does not depend on widening `Id::new`.
