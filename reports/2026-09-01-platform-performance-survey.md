# Platform performance survey (core) + MCU architecture

**APPROVED-CORE-EDIT: platform-performance-survey** · 2026-09-01 12:35 CEDT
Branch `feat/platform-performance-mcu-plan` @ base `fb6b783`. Measurement + design only; core `main`,
the WebXR branch, and both graph branches **not merged**. Production source is byte-identical (only
benchmark/docs/plan artefacts added, plus the graph direct_ir bench extended in its own repo).

## 1. Method
Unified measurement contract (`docs/plans/.../metric-definition.md`): phases cold / prepared / resident /
end-to-end never cross-compared; one typed time unit per table; A/A null for drift; interleaved A/B.
Per CLAUDE.md, trustworthy prior reports are **cited**, not re-measured from scratch; new measurement
targets headline verification + gaps.

## 2. Host / environment
Win11 (26200), 24 logical cores, RTX 5080 Laptop; rustc 1.97.1, release. Ambient CPU 8.7–14.4%
(borderline the <10% quiet target) → fine deltas are **PROVISIONAL**; the incidence-operator cell ran
at 8.7% (quiet). Second host Amaterasu reachable (Ubuntu 24.04, 16 CPUs) but **no Rust toolchain** →
two-host comparison **NOT performed** (no invented numbers).

## 3. Benchmark inventory (reuse, not duplicate)
Full inventory: `docs/plans/2026-09-01-platform-performance-mcu/benchmark-inventory.md`. Reused as-is:
`ir_bench`, `ir_stream_bench`, `csr_bench`, `interning_bench` (criterion); the incidence `#[ignore]`
operator bench; the webxr bench; wgpu `spmm/chain/arena`. **Duplications flagged** (consolidation
targets, not implemented): `SparseSpec`/`Lcg` in `benches/sparse_fixture.rs` **and** `wgpu/src/testing.rs`
(byte-identical); CPU SpMM in `csr_bench` **and** `spmm_bench` A/B; three generators of the same
edge→arc→ref topology; cross-repo Steiner/sunflower in graph tests **and** `hymeko_core::generators`.

## 4. Core base-operation performance
Cited from prior trustworthy reports (unified method verified where re-run):
- CPU CSR SpMV serial 0.53 µs (1k nnz) → 5.23 ms (4.2M nnz); Rayon at the top ~9.7×; **serial→Rayon
  crossover at `PAR_SPMV_NNZ_THRESHOLD = 81 920` nnz** (published baseline). SpMM per §6.2 there.
- Resident incidence throughput 202–606 ns/incidence, topology-dependent (VLAN worst) — large-hypergraph.
- Canonical memory ~146 B/decl canonical + 89 B/decl derived; serialized JSON ~5.86 MB at 20k incidences.
- `ir_bench` groups (build/clone/serde-JSON/canonical_program_hash/lookup/hierarchy) and `interning_bench`
  (parse+intern) exist and are runnable; not re-timed this session (criterion suites, covered by the
  published baseline). **Gaps with no timing bench:** derived path index, namespace/coherence validation,
  delta-apply/materialization.

## 5. Incidence / vector operator performance (measured, quiet host 8.7%)
`raw/incidence_operator.tsv`, nnz = 1e6:

| workload | index build | B/inc | Bx | Bᵀy | BᵀBx | **Bᵀy/Bx** | clique build | clique B/inc |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| arity 5 (200k arcs) | 20.4 ms | 14.8 | 5.9 ms | 28.9 ms | 35.3 ms | **4.9×** | 30.0 ms (1.5×) | 18.0 (1.2×) |
| arity 20 (50k arcs) | 15.6 ms | 12.4 | 5.2 ms | 17.6 ms | 22.8 ms | **3.4×** | 57.6 ms (3.7×) | 76.2 (6.1×) |

**Retracted (§13):** the prior corrective report's **"Bᵀy ~10× Bx"** (Bᵀy 168–243 ms) is **NOT
reproduced** on a quiet host. Quiet-host Bᵀy is 17–29 ms, **3.4–4.9× Bx** — the prior absolute Bᵀy was
inflated by host contention (that report explicitly flagged its host as not-quiet and advised this
re-run). The arc-scatter penalty is real but modest. Profiled mechanism (not optimised this phase):
`Bᵀy` resolves each incidence's sign/weight by chasing the scattered `ArcRec.refs` (node-major
indirection); `Bx` streams arc-major. `BᵀBx ≈ Bᵀy + Bx` (composition), confirmed. Clique/2-section is a
control only: 1.2–6.1× the index memory (arity-dependent), never the standard path.

## 6. WebXR validator scaling (measured)
`raw/webxr_validate.tsv`: validate 0.01 ms (100 ent) → 31.2 ms (1e5 ent), ~0.31 µs/entity, flat per-entity
(linear). Consistent with the prior 32 ms/1e5 after the bucketed-linearization; the "522 ms" is the
pre-optimization git-historical baseline (one classification pass + reusable buffer replaced the 7-scan +
per-call HashSet). No SceneDto built for the measurement.

## 7. ir_stream / CBOR results (audit + decision)
ir_stream (`src/ir_stream/`) is the real wire format: 144-byte envelope (BLAKE3 digest, LE, schema
version), 32-byte SectionEntry table (offset+length, skippable Unknown sections), Begin/Chunk/Commit/Abort
assembler. `ir_stream_bench` times validate/section/reject/assembler; runnable, not re-timed this session.
**CBOR is dormant**: `CborPayload` is an unwired full-copy serde struct; no CBOR codec is a real
dependency. **Decision (§12 in the MCU doc): the sectioned snapshot is the single MCU wire format; CBOR is
not adopted** (it would duplicate the schema for a lossy full copy). Falsifier stated there.

## 8. Tensor CPU / Rayon / WGPU
Cited from the 2026-08-25 published baseline (not re-run; RTX 5080 present but GPU cells are large criterion
runs and the baseline is trustworthy): CPU SpMV/SpMM serial vs Rayon crossovers (SpMV at 81 920 nnz); GPU
resident SpMM, upload/readback isolation, activation, chain, arena. Not folded into the core/graph baseline
per §7 (GPU numbers kept separate for the MCU plan, which is CPU/flash-bound).

## 9. Allocation / memory / RSS
Incidence index 12.4–14.8 B/incidence retained (real 8-byte HyperArcId). Canonical ~146 B/decl. webxr
validate: warm query allocation-free (bucket pass + one reusable buffer). Peak RSS ≪ 16 GiB for all
1e6-incidence / 1e5-entity cells in one process.

## 10. Two-host
NOT performed — Amaterasu has no Rust toolchain (see §2). Single-host only; fine deltas PROVISIONAL.

## 11. Ranked bottleneck list
1. **Bᵀy arc-scatter** — *mechanism proven* (node-major indirection resolving sign/weight from scattered
   refs). *Expected gain*: a co-located sign+weight operator-side cache would cut the 3.4–4.9× toward 1×.
   *Impl risk*: low (a derived cache). *Semantic risk*: low (derived, lossless). *Next token*:
   `incidence-operator-precompute`.
2. **Fat canonical structure vs flat CSR on traversal** — *proven* (graph report: direct ≈13× a flat u32
   star for warm reachability). *Gain*: large for query-heavy workloads. *Impl risk*: medium (a flat
   target cache betrays zero-copy if defaulted). *Semantic risk*: medium (losslessness/coherence). *Next
   token*: `graph-flat-target-cache` (opt-in only).
3. **serde/JSON path not MCU-portable** — *proven* (usize IDs + tagged enums). *Gain*: enables the MCU
   wire. *Impl risk*: low (the ir_stream fixed-width path already exists). *Semantic risk*: low. *Next
   token*: `mcu-c-wire-view-pow` (the C11 layout probe).
4. **Benchmark-harness duplication** (SparseSpec×2, SpMM×2, 3 topology generators, cross-repo Steiner/
   sunflower). *Gain*: measurement trust + less drift. *Impl risk*: low. *Semantic risk*: none. *Next
   token*: `bench-harness-consolidation`.
5. **Timing gaps** (derived path index, namespace/coherence validation, delta-apply). *Gain*: closes the
   survey. *Impl risk*: low. *Next token*: fold into `bench-harness-consolidation`.

## 12. Retracted hypotheses / measurement errors
- Bᵀy ~10× Bx → **retracted**; quiet-host is 3.4–4.9× (§5).
- The 522 ms webxr figure is a pre-optimization historical baseline, not the current path (§6); current
  is 31 ms/1e5.
- "CBOR-direction work" implied a real CBOR path → **there is none** (dormant unwired full-copy struct).

## 13. MCU / C-C++ / CBOR decisions
Full plan: `docs/mcu-canonical-ir-architecture.md`. One semantic Ir; profiles MCU-RO / MCU-DYNAMIC /
MCU-HOSTED; C wire view = the existing ir_stream envelope+section format (no new protocol); C++ = a thin
RAII/span wrapper over the same C view; ID/numeric contract = fixed-width u32/u64, i8 signs, explicit
f32/f64, typed rejection, no silent meaning change; CBOR **not adopted** (sectioned snapshot is the single
format). First implementation PoW (planned, not built): a host-compiled C11 layout probe reading the same
frozen KAT bytes on both sides under ASan/UBSan.

## 14. Worktree / remote
Worktree `/d/hnnwork/core-perfmcu` clean at the feature tip; branch off `fb6b783`; no foreign files
touched; Amaterasu SSH config/keys untouched. Remote push + SHA in the return summary. **HARD STOP.**

## Provenance
rustc 1.97.1, x86_64-pc-windows-msvc. Single-host, ambient 8.7–14.4% (PROVISIONAL where noted).
