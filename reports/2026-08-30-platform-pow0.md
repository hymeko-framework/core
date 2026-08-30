# Platform PoW-0 — source-to-CPU/GPU vertical slice

**Written:** 2026-08-30 09:11 CEST · **Base HEAD:** `0402d98` · **tree (pre-commit):** `d9050f4` · **Branch:** main (local == origin)
**Authorisation:** none — `hymeko_wgpu/tests/**`, a golden fixture outside `data/**`, docs, report, plan. **No `hymeko_core/src`, `Cargo.toml`, dependency, lockfile, `data/**`, or product binary; no `APPROVED-CORE-EDIT`.**

Evidence classes: **[M]** measured · **[S]** structurally verified (a running test / an exact symbol) · **[I]** inferred · **[H]** hypothesis · **[NI]** not implemented.

## 0. Result

**PoW-0 proves the static computational spine, end to end, on real hardware.** The
existing public path composes with no new abstraction:

```
HyMeKo source → RealParser → ModuleStore/lowering → canonical Ir
  → coherence → HyperGraphView → star-expansion CSR
  → CPU serial & Rayon SpMM → WGPU resident SpMM + activation → readback
  → numerical & structural equivalence
```

`hymeko_wgpu/tests/pow0_vertical_slice.rs` — **11 tests, all PASS [M]**, including
the GPU tests (they ran on the RTX 5080, not skipped).

## 1. Source fixture and demonstrated path [S]

Golden: `hymeko_wgpu/tests/data/pow0_service_network.hymeko` (auditable, not a
generator). A redundant service/network scenario: `sw_core`/`sw_edge` switches,
`host_a/b/c`, `svc_web/db/cache`; a **4-way** VLAN failure-domain hyperedge
(`+sw_core, −host_a, −host_b, −host_c`), a second overlapping failure domain
(`vlan_edge`, sharing `host_c`/`host_a`), a signed service-dependency edge
(`+svc_web, −svc_db, ~svc_cache` — the neutral advisory), and two placement edges.
Signs +/−/~, incidence weights, hierarchy under a `datacenter` scope.

Lowers to **decls 19, nodes 9, edges 5, arcs 5, incidences 14**; canonical hash
`00d4c01a…` (frozen). Star-expansion CSR: dim 14, nnz 20. Figure:
`reports/figures/2026-08-30-pow0-golden.svg`.

## 2. Test/coverage counts [M]

| suite | pass | fail | ignored |
|---|---|---|---|
| `pow0_vertical_slice` | **11** | 0 | 0 |
| workspace (all-features) | **538** | 1 | 1 |
| — the 1 fail | the documented `test_load_meta_geometry` 58/57 drift, left visible | | |
| doctests | core 1, wgpu 10 | 0 | 0 |

Gates: `fmt --check` clean, `clippy --workspace --all-features --all-targets -D
warnings` clean, `check`/`build` ok, bench smoke (`ir_bench --test`) ok.
**Production line coverage 85.19 %** (same denominator/skips as the M2b baseline;
+0.26 pp vs the conformance gate's 84.93 % — the integration test exercises more of
the hnn view/CSR and wgpu op production paths). **No production source changed, so
touched-production-line coverage is N/A.** Test-support coverage is not counted as
production. Named negative/fault tests: `pow0_missing_source_file_is_a_typed_error`,
`pow0_parser_rejects_malformed_source`; deeper malformed-IR/U3/U4/workgroup/invalid-
CSR/arena-provenance cases are cited from existing named tests (traceability), not
duplicated. No PoW test is `#[ignore]`; GPU tests skip only on a missing adapter
(not the case here).

## 3. CPU / Rayon / GPU correctness [M]

- **CPU:** `spmm_into_serial` ≡ `spmm_into_parallel` **bitwise** at fd 8/32/128
  (the row-disjoint, no-reassociation contract).
- **GPU:** one-encoder/one-submit resident SpMM→ReLU, intermediate `y` kept
  resident, read back through the tensor API; equals the CPU oracle within
  abs/rel 1e-5 with structural NaN/Inf agreement, at fd 8/32/128.
- **Structure:** the star CSR matches an **independent** oracle derived from the
  star definition (+: node→edge, −: edge→node, neutral: both) over the view's
  public incidence fields — not by calling the builder twice.

## 4. Performance / memory [R/I]

PoW-0 is a **correctness** proof on a deliberately tiny golden (dim 14); its own
timings are not a reportable benchmark. The platform's large-scale performance and
memory are the separate consolidated baseline
(`reports/2026-08-30-large-hypergraph-performance.md`): ingestion ~445 ns/incidence,
CSR SpMV Rayon crossover at 81 920 nnz, canonical vs derived memory 61.1 % overhead,
resident-vs-readback the dominant GPU effect. No new benchmark target was registered
(the existing end-to-end cells already exist; Phase E adds none).

## 5. Two-host result [D]

**Deferred.** The decisive subset was not re-run on Amaterasu in this session; the
local RTX 5080 result stands and the two-host GPU crossover remains the
provisional/bracketed result of the published baseline. No remote result is
invented. (Amaterasu correctness cross-check of the pure-CPU spine is
straightforward future work — the tree hash to transfer is `d9050f4`.)

## 6. CLI-host verdict [S] — the earlier single-CLI plan is INVALID

Dependency audit (Phase F): `hymeko_pgraph` (graph) **depends on** `hymeko_core`;
core does **not** depend on graph (one-way `graph → core`). Putting the `hymeko`
façade in the **core** package and linking graph would create **`core → graph →
core`**, a Cargo cycle. **Corrected:** the single façade must live in the
already-downstream **`hymeko_pgraph` package** (promote its `pgraph` bin to
`hymeko` with subcommands; remove the core binary and the parser stub afterward).
`docs/unified-ir-single-cli.md` is corrected additively; `UIR-CLI-04` stays
PARTIAL. No CLI was implemented or added.

## 7. PoW-0 limitations (honest)

- **[NI]** Proves nothing dynamic: no mutation, transactions, CBOR, versioning,
  replay, or distributed convergence — those are PoW-1 (BLOCKED).
- **[S]** Source emission (`IrWriter`) remains skeleton-only; PoW-0 does not
  round-trip source (it goes source→Ir→compute, not Ir→source→Ir).
- **[S]** The golden is small by design (auditability); scale is covered by the
  separate large-graph baseline, not PoW-0.
- **[D]** Two-host GPU crossover not re-measured here.
- Clique expansion was not exercised (the scenario's star expansion is the
  non-trivial one; `clique_expansion_csr` is cited as available, applied only where
  its projected semantics hold).

## 8. PoW-1 (dynamic) — design only, BLOCKED

`docs/platform-proof-of-work.md` specifies the 10 dynamic scenarios and **six
separate approval scopes in dependency order**, each with files/API/invariants/
compat/rollback/tests/coverage/budget/falsifier, reusing the documented
transaction + `ir_stream` mechanisms (no new delta vocabulary). Traceability rows
`POW1-01..07` are BLOCKED, never counted as implemented.

## 9. Requirement satisfaction (traceability.csv)

Added `POW0-01..09` (all PASS) and `POW1-01..07` (all BLOCKED). Matrix-wide: **56
PASS · 3 PARTIAL · 3 FAIL (known defects) · 13 NOT_IMPLEMENTED · 17 BLOCKED**.
Evidence coverage: every PoW-0 row cites a named passing test; every PoW-1 row
names its blocking token. Implemented-normative satisfaction for PoW-0 = 9/9 PASS;
end-state satisfaction counts the 7 PoW-1 rows as future.

## 10. First recommended PoW-1 approval token

**`APPROVED-CORE-EDIT: canonical-cbor-codec`** — the smallest independently-valuable
step (needs neither stable IDs nor transactions), it removes the measured ~20×
JSON-vs-packed snapshot overhead and the `CborPayload` derived-state duplication,
and it is the on-ramp for the snapshot side of PoW-1. The full six-token order is in
`docs/platform-proof-of-work.md`.

## Files

`hymeko_wgpu/tests/pow0_vertical_slice.rs`,
`hymeko_wgpu/tests/data/pow0_service_network.hymeko`,
`docs/platform-proof-of-work.md`, `docs/unified-ir-single-cli.md` (correction),
`docs/verification/unified-ir/traceability.csv` (+16 rows),
`docs/plans/2026-08-30-platform-pow0/*`, this report,
`reports/data/2026-08-30-pow0-golden-cardinalities.tsv`,
`reports/figures/2026-08-30-pow0-golden.svg`. No `src`/manifest/dependency/`data/**`
change; the three IFAC artifacts are untouched.

## HARD STOP

No stable IDs, arenas, mutation, transactions, snapshots, CBOR, `ir_stream` apply,
or CLI consolidation implemented. PoW-1 awaits the listed approvals.
