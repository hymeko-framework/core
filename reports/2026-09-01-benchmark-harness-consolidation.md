# P1 — benchmark-harness consolidation

**GOVERNING-STRATEGY:** docs/hymeko-platform-strategy.md · **STRATEGY-ID:** HYMEKO-PLATFORM-2026-09-01-v1
· **ROADMAP-TASK:** P1 / benchmark-harness-consolidation. 2026-09-01 19:50 CEST.
**APPROVED-CORE-EDIT:** benchmark-harness-consolidation (the core owner module + its `tensor/mod.rs`
export line only).

Consolidation / V&V / docs. No new benchmark framework, support crate, RNG, second `SparseSpec`, or
parallel graph model. Strategy ID **not** bumped. Two feature branches, **neither promoted to main** this
phase.

## Entry state (verified `git ls-remote`)
Core `origin/main = 2ebb833`; graph `origin/main = cbaadd4`. Both matched exactly. Fresh scratch worktrees
off those SHAs; the user dirty checkout untouched.

## 1. What was consolidated (the one proven exact duplicate)
The deterministic sparse-CSR fixture — `Lcg` (MMIX LCG) + `DegreeShape` + `SparseSpec`
(`small/medium/large`, `build`/`dense_vector`/`dense_block`) — existed **byte-for-byte** in
`hymeko_core/benches/sparse_fixture.rs` **and** `hymeko_wgpu/src/testing.rs` (136/136 non-comment lines,
diff-clean modulo doc comments). Moved to a single backend-neutral owner:
**`hymeko::tensor::sparse_workload`** (`#[doc(hidden)] pub`).

## 2. Owner choice — why a core-lib module, not a new framework (§4)
The dependency edge is `hymeko_wgpu → hymeko_core`. A `hymeko_core` bench cannot reference a symbol defined
in `hymeko_wgpu` without a dev-dependency cycle (core-bench → wgpu → core). The **only** location reachable
by both the core benches and wgpu's tests/benches/examples is `hymeko_core`'s own library API. So the owner
is a core-lib module — **no new crate, no new dependency**. Placed under `tensor/` next to `TensorCsr`,
**not** in `generators.rs`: it emits a numeric `TensorCsr<f32>`, it is not a canonical hypergraph family, so
it must not mix with the Steiner/sunflower generator semantics. `#[doc(hidden)]` keeps it off the documented
surface while remaining reachable by the separate bench/example/test crates; the surface is exactly the
three names both former copies exported.

## 3. Duplication inventory — before / after
Full classification: `docs/plans/2026-09-01-benchmark-harness-consolidation/duplication-inventory.md`.
**Consolidated (category 1): exactly one** — the sparse-CSR fixture. Everything else on the mission's §2.2
candidate list classified 3/4/5 and is **kept, justified**:

| kept | why |
|---|---|
| CPU SpMM in `csr_bench` **and** `spmm_bench` | intentional measurement duplication — both call the production `TensorCsr::spmm_into_{serial,parallel}` kernel; `csr_bench` = standalone CPU baseline, `spmm_bench spmm_cpu` A/B = **same-run GPU crossover** reference |
| `ir_bench` `Lcg` (`>>33`, IR-decl shuffle), `assembler` `Lcg` (`#[cfg(test)]`) | distinct single-consumer helpers; share only the MMIX constant. §2.2: don't merge RNGs on the name |
| graph `build_window_ir` sliding-window; core `generators.rs` Steiner/sunflower | distinct constructions / already single-owner (graph reuses `hymeko_core::generators`) |
| wgpu CPU reference oracles, `backend_or_skip`/`Fixture`/`encode_chain`/`chain_output` | independent differential oracles (kept on purpose); wgpu-specific scaffolding (needs GPU types) |

No `benchmark_support` / `testing_framework` / workload crate created.

## 4. LOC removed / retained
`refactor` commit: +288 / −342 (net **−54**). One full duplicate copy eliminated: `benches/sparse_fixture.rs`
(171 lines) deleted; the `wgpu/src/testing.rs` sparse block (~163 lines) removed and replaced by a 2-line
`pub use` re-export; the owner module is 268 lines (verbatim generator + a `#[cfg(test)]` unit-test module +
ownership docs). Net **one generator copy (~163 lines) removed** from the tree.

## 5. Frozen fixture hashes (blake3, domain `HYMEKO-P1-SPARSE-KAT-v1`)
| preset | rows | cols | nnz | digest |
|---|--:|--:|--:|---|
| small | 1000 | 1000 | 7 967 | `d29a53e9…a818ce` |
| medium | 50000 | 50000 | 799 877 | `311abed6…f766ef6` |
| large | 200000 | 200000 | 5 442 303 | `7d764dea…259c883` |

`tests/sparse_workload_kat.rs` was committed **first** against the OLD `benches/sparse_fixture.rs` (via
`#[path]`), then rebound to the owner module with the **same** constants — a green run after the move is a
bit-identity proof. Digest covers rows/cols/full row_ptr/col_ind/val-bits/dense_vector/dense_block(8,32,128).

## 6. Benchmark target / name changes
**None.** `csr_bench` groups (`csr_spmv`, `csr_spmm`, `csr_spmv_paths`, `csr_spmm_paths`, `csr_spmv_alloc`)
and wgpu `spmm`/`chain`/`arena` are unchanged; `hymeko_wgpu::testing::{Lcg,DegreeShape,SparseSpec}` is now a
re-export, so every `use` path and every reproduction command is byte-identical. All benches compile
(`cargo bench --workspace --no-run`).

## 7. Tests / doctests / coverage
- Core workspace: **538 passed / 0 failed / 4 ignored** (all-features) — +6 vs P0's 532 (3 KAT + 3 owner
  unit tests). Core doctests green. wgpu: 168 tests pass on the real RTX 5080 (CPU/GPU SpMM correctness).
- Graph workspace: **444 / 0 / 6** — +1 vs P0's 443 (the pgip id-invariant test). Clippy green.
- New production lines (the owner module) are covered by the owner unit tests (well-formedness, determinism,
  irregular-shape) **and** the integration KAT (byte identity). `cargo llvm-cov` unchanged elsewhere (only
  relocated code); not re-run since no other production line changed.

## 8. A/B performance & memory
Generator source relocated **verbatim** ⇒ identical codegen; fixtures **bit-identical** (KAT). No kernel
touched. Release rlib sizes (`target/release/deps`), the real and expected change (the fixture leaves the
bench crate + the wgpu rlib and lives once in the core rlib):

| rlib | before | after | Δ |
|---|--:|--:|--:|
| `libhymeko.rlib` | 3 872 274 | 3 904 954 | **+32 680 (+0.84%)** |
| `libhymeko_wgpu.rlib` | 1 586 558 | 1 525 616 | **−60 942 (−3.84%)** |
| both | 5 458 832 | 5 430 570 | **−28 262 net** |

`build()` is unchanged source (same `Vec::with_capacity`, `sort_unstable`, `dedup`) ⇒ **no new per-call or
per-incidence allocation**, retained bytes identical. Fixture-build/kernel wall time is not asserted from
single-shot timers on this ~8–14% ambient host (§3 forbids); bit-identity + identical codegen + the size
table are the load-bearing no-regression evidence. Peak RSS for the `large` KAT cell (`dense_block(128)` ≈
25.6M f32 ≈ 102 MiB transient) ≪ 16 GiB.

## 9. Graph lint & dump/PGIP equivalence
Full detail: graph `reports/2026-09-01-p1-pgraph-lint-gate.md` (branch `feat/p1-lint-cleanup`). The two
`explicit_counter_loop` lints in `pgip_io.rs` (`write_pgip` id loops) rewritten to
`(1i64..).zip(…)` — 1-based contiguous ids preserved (byte-identical). **P0 undercount surfaced:** clearing
the lib lints unmasked a third pre-existing lint (unused imports in `hymeko_pgraph_dump.rs`, masked because
`-D warnings` aborted at the lib) — also cleared. All 3 pre-existing, behaviour-neutral. `.pgip` round-trip
+ ABB-bake tests green; a new `materials_and_units_get_contiguous_1_based_ids` test pins MIN(id)=1 /
MAX(id)=COUNT; M7 mutation `(1i64..)→(0i64..)` caught. Graph clippy `-D warnings` now **green**.

## 10. Intentional duplication (declared, not removed)
`csr_bench`/`spmm_bench` CPU SpMM (standalone baseline vs same-run GPU crossover); wgpu differential CPU
oracles; `ir_bench`/`assembler` `Lcg`; graph sliding-window vs core Steiner/sunflower. Each is category 3/4/5
(§2.2) — justified in the inventory. No §6.5 anti-pattern introduced.

## 11. Own mistakes / retracted assumptions
- **Mutation-restore footgun:** after proving M7, `git checkout -- pgip_io.rs` reverted my *uncommitted*
  graph fix (base was HEAD=cbaadd4). Detected immediately (grep showed the counter back), re-applied,
  committed. The M7 result itself is valid (it mutated the applied fix before the checkout). Lesson: commit
  the fix before running a checkout-restore mutation, or restore from a copy.
- **P0 lint undercount (2 → 3):** the true pre-existing `hymeko_pgraph` lint set was larger than P0's 2,
  because `-D warnings` masks later targets. Surfaced and cleared, not hidden.
- **`items_after_test_module`:** the first wgpu re-export placement (after the `#[cfg(test)] mod tests`)
  tripped clippy; reordered so the test module is last. fmt also rewrapped one unit-test `assert!`.
- No architecture claim changed; the strategy §11 retractions stand.

## 12. Fault-injection / mutation evidence (no mutant committed; git-clean restore each)
Core owner: **M1** RNG multiplier, **M2** high-bit shift, **M3** irregular-bucket boundary, **M4** `dedup`
removal, **M5** `dense_block` seed XOR, **M6** rename owner symbol → **all caught** (M1–M5 by the KAT
digest / unit tests; M6 by a wgpu compile failure, proving the re-export binding is load-bearing — a stale
local copy would still compile). Graph: **M7** 0-based id caught by the new invariant test. Every source
restored git-clean; no mutant committed.

## Commits (feature branches; NOT promoted)
Core `feat/benchmark-harness-consolidation`: `8404f87` plan · `45dfe23` freeze KAT · `562a018` consolidate
(token) · this report. Graph `feat/p1-lint-cleanup`: `e952805` id fix · `4911553` unused-import · `c35b9f2`
verification doc.

## P2 entry conditions (`incidence-operator-precompute`)
Enter only with core+graph mains still at `2ebb833` / `cbaadd4` (P1 is not promoted; a promotion decision is
separate) or at whatever they are promoted to, and the strategy on main. P2 targets the strategy §11 #1
bottleneck: a derived, provenance-tracked sign+weight operator-side cache to cut the measured Bᵀy arc-scatter
(quiet-host 3.4–4.9× Bx) toward 1×. It is a **derived-index** change (lossless, rebuildable) over the
canonical `Ir` + `IncidenceIndex`, opt-in and measured before defaulting; it must not alter canonical bytes,
the wire, or IDs, and rests on the P1-consolidated `sparse_workload` fixture for its CPU baselines. Do **not**
begin it now.

## Provenance / HARD STOP
rustc 1.97.1, x86_64-pc-windows-msvc, release. RTX 5080 present (wgpu GPU tests ran). Single host. CORE.YAML
unchanged. Nagare and siblings untouched; the user dirty checkout untouched. **HARD STOP** after pushing the
two feature branches — no promotion, P2 not started.
