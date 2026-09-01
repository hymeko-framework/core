# P1 duplication inventory — classification before consolidation

Created-at: 2026-09-01 19:05 CEST · STRATEGY-ID HYMEKO-PLATFORM-2026-09-01-v1 · ROADMAP-TASK P1

Categories (mission §2.2): **1** byte/semantic exact duplicate · **2** same algorithm, different adapter ·
**3** intentional same-run measurement control · **4** same family name, different math · **5**
single-consumer local helper · **6** dead code. **Only categories 1–2 are consolidated.**

| # | Candidate | Sites | Category | Action |
|---|-----------|-------|:--:|--------|
| 1 | Sparse-CSR fixture `Lcg`+`DegreeShape`+`SparseSpec` (MMIX LCG, presets small/medium/large, `build`/`dense_vector`/`dense_block`) | `hymeko_core/benches/sparse_fixture.rs`; `hymeko_wgpu/src/testing.rs` (2nd half) | **1** | **CONSOLIDATE** → `hymeko_core::tensor::sparse_workload`. Proven byte-identical: 136/136 non-comment lines, clean diff modulo doc-comments. |
| 1a | Duplicated fixture doc-comment header inside `wgpu/src/testing.rs` (two near-identical `=== Backend-neutral sparse-CSR fixture ===` blocks) | `hymeko_wgpu/src/testing.rs` | **1** | **REMOVE** (both blocks) with the moved code; the re-export needs no header. |
| 2 | CPU SpMM cells `spmm_into_serial` / `spmm_into_parallel` | `core/benches/csr_bench.rs` (`csr_spmm*`); `wgpu/benches/spmm_bench.rs` (`spmm_cpu` A/B) | **3** | **KEEP BOTH**. Both *call the same production kernel* (`TensorCsr::spmm_into_*`) — no algorithm is re-implemented. `csr_bench` is the standalone CPU baseline; `spmm_bench`'s `spmm_cpu` A/B is the **same-run GPU crossover reference** (measured next to the GPU cells on one host/thermal state, mission §6). Tagged `intentional measurement duplication`. |
| 3 | `Lcg(u64)` in `benches/ir_bench.rs:763` | core benches | **5** | **KEEP**. Distinct helper: `next()` returns `self.0 >> 33` (different extraction), drives `shuffled_decl_ids`/`mixed_validity_ids` (IR-decl shuffling, not CSR). Shares only the MMIX multiplier constant. §2.2: "don't merge RNGs on the `Lcg` name alone" — merging would change behaviour. |
| 4 | `Lcg(u64)` in `src/ir_stream/assembler.rs:2224` (`#[cfg(test)]`) | core src tests | **5** | **KEEP**. Test-local helper, single consumer. |
| 5 | Steiner / sunflower / complete-uniform generators | `hymeko_core/src/generators.rs` (`HypergraphDesign`) | **owner already single** | **NO-OP**. Core is the declared single home ("higher layers adapt rather than re-implement"). Graph reuses `hymeko_core::generators` (per direct-Ir report), does not re-implement. No cross-repo copy found to remove. |
| 6 | Graph sliding-window workload `build_window_ir` (arity-k hypergraph over `&Ir`) | `hymeko_graph` direct_ir bench | **4** | **KEEP**. Distinct mathematical construction (sliding window), not a Steiner/sunflower family; not a numeric CSR fixture. §2.2 forbids equating distinct constructions. |
| 7 | CPU reference oracles (`cpu_activation`, diamond oracle) | `wgpu/tests/activation.rs`, `fanout_tests` | **3** | **KEEP**. Written independently *on purpose* so a shared bug cannot hide behind both sides of a differential comparison (documented in `testing.rs` header). Unifying weakens every differential test. |
| 8 | wgpu GPU scaffolding `backend_or_skip`, `Fixture`, `encode_chain`, `chain_output` | `wgpu/src/testing.rs` (1st half) | **5** (single-crate) | **KEEP in place**. wgpu-specific (needs `WgpuBackend`, `GpuCsrMatrix`, scope types); cannot move to core (core has no GPU types) and is not duplicated elsewhere. |
| 9 | `pgip_io.rs` `next_id` / `next_uid` explicit counters | `hymeko_pgraph/src/pgip_io.rs:368,389` | — (lint, not dup) | **FIX** (§5) — `explicit_counter_loop` → `enumerate`, behaviour-neutral, separate graph branch. |

## Consolidation summary
- **Consolidated (cat 1–2): exactly one** — the sparse-CSR fixture (rows 1, 1a).
- **Intentional duplication kept (cat 3): rows 2, 7** — same-run GPU crossover CPU cell; independent
  differential oracles.
- **Distinct constructions / local helpers kept (cat 4–5): rows 3, 4, 6, 8.**
- **Already single-owner (no-op): row 5.**
- **Dead code (cat 6): none found.**

The mission's §2.2 "possible but not automatically removable" list (SpMM×2, multiple Lcg, edge→arc→ref
generators, core-vs-graph Steiner/sunflower, Mycielski/Steiner, sliding-window, WGPU CPU oracles) all
resolve to categories 3/4/5 — **none** are consolidated, each justified above. No `benchmark_support` /
`testing_framework` / workload crate is created (§4).
