# Large-hypergraph performance baseline (consolidated)

**Written:** 2026-08-30 04:23 CEST · **HEAD:** `75c6c7b` · **tree:** `7fd0a9c` · **Branch:** main (local == origin)
**Authorisation:** none — reports + existing benches + the reusable TSV→SVG helper only. No `hymeko_core/src`, `Cargo.toml`, dependency, `data/**`, or sibling-repo change.

Evidence classes: **[M]** measured this session · **[R]** reused from a cited prior
measured artifact · **[I]** inferred · **[H]** hypothesis · **[D]** deferred (not
run; reason stated). Per the campaign brief, existing benchmarks and profiling
artifacts are reused; **no grid was rerun.** Topology families are never averaged
into one number.

## 0. Method and honesty envelope

The stage-level performance of this platform is **already measured** in recent,
rigorous artifacts; this report *consolidates* them into the source→IR→view→op
pipeline view and answers the campaign's ten questions, adding only the one
deterministic measurement that was missing (core IR memory accounting). The
machine was **not exclusively quiet** during consolidation (a coverage build ran
earlier in the session), so every *timing* here is inherited from its original
quiet-run artifact **[R]**; the only value measured now is a `size_of`/`len`
memory calculation **[M]**, which is independent of machine load. The full
two-host GPU crossover grid is **[D] deferred** — see §9.

Reused artifacts: `reports/2026-08-25-published-baseline-benchmark.md` (§5–§13,
Criterion, quiet host), `reports/2026-08-30-graph-workload-vv-corrective.md` (§11–§12,
large topology families through the real parser), and the saved `target/criterion`
cells.

## 1. Correctness gate (before any timing) [R/M]

- **Parser → ModuleStore → lowering** succeeds on the 96-fixture core corpus and
  on the graph workload families up to the large band (Steiner m=9 = 130 305 inc,
  random-uniform = 200 000 inc, VLAN = 262 656 inc all lower; graph VV §11). **[R]**
- **Canonical coherence** passes; **repeated construction is bit-identical** and
  the canonical hash is stable (`validation::workload_coherence::*`,
  `repeated_workload_construction_is_bit_identical`,
  `workload_canonical_hash_is_stable_across_construction`). **[M this session:
  suite green, 251/1-known/0.]**
- **Formula-derived cardinalities** verified per family; serial vs parallel agree
  under the bitwise CSR contract (`csr-spmv-spmm-into`). **[R]**
- No malformed state is silently discarded (strict consumers report typed errors:
  `UIR-M2B-03`, `derived_views` rejections). **[M]**

## 2. Ingestion throughput by topology (parse → lowered) [R]

From graph VV §12 (release medians; resident = pre-parsed; the P-graph lowering
pipeline, i.e. parser→`Ir`→`meta_resolve`). Figure:
`reports/figures/2026-08-30-large-hypergraph-ingestion.svg`;
data: `reports/data/2026-08-30-large-hypergraph-ingestion.tsv`.

| family | incidences | resident µs | ns / incidence |
|---|---|---|---|
| Steiner STS15 | 105 | 21.2 | 202 |
| Steiner STS31 | 465 | 128.2 | 276 |
| Steiner STS127 | 7 997 | 3 238.6 | 405 |
| Mycielski iter5 | 4 724 | 2 225.1 | 471 |
| random-uniform | 20 000 | 8 893.3 | 445 |
| VLAN failure-domain | 2 112 | 1 279.9 | **606** |
| humanoid | 314 | 127.6 | 406 |
| chicken-flock | 2 542 | 780.4 | 307 |

Throughput is ≈ **linear in incidences** with a **topology-dependent constant**
of 202–606 ns/incidence. **VLAN (network) is the most incidence-expensive**
(606 ns/inc — dense failure-domain membership); Steiner (STS15) is cheapest
(202). Source generation and hashing are outside the timed region.

## 3. Canonical IR memory accounting [M this session]

`ir_bench::ir_redundancy` at 20 001 declarations / 5 000 arcs (`--test` pass;
deterministic, load-independent):

| bucket | bytes | class |
|---|---|---|
| record tables (`DeclNode`+`NodeRec`+`EdgeRec`+`ArcRec`) | 2 920 096 | **canonical** |
| parallel maps (`decl_to_node/edge/arc` + hash slots) | 1 785 081 | **derived / cache** |
| derived ÷ records | **61.1 %** | overhead |

- **`Option<NodeId>` = 16 B** for an 8 B id — a 2× overhead per map slot (no niche
  today; `UIR-M2C-04` tracks that a generational key must not enlarge it). The
  derived maps are the memory duplication a snapshot need not carry on the wire.
- Logical bytes per declaration ≈ 2 920 096 ÷ 20 001 ≈ **146 B/decl canonical** +
  **89 B/decl derived**. Serialized JSON is ~5.86 MB at 20 000 incidences
  (`ir_serde`), i.e. the text/JSON encoding is ~20× the packed record tables —
  motivating the compact CBOR path (still unimplemented).
- `size_of::<T>()` is **not** presented as total object memory: heap capacities of
  the `Vec` arenas dominate and are counted via `len × size_of<record>`, with the
  `Option<Id>` overhead called out separately.

**Quadratic-expansion guard:** no clique/star expansion was materialised here;
high-cardinality clique expansion at the large band would exceed the 16 GiB cap
and must be a *checked projected skip* (graph VV records the projection), not an
OOM. **[R]**

## 4. Derived-view build & fundamental ops [R]

- `HyperGraphView::from_ir` (view construction) — published baseline §7.4; builds
  through the accessor layer; deterministic rebuild verified (`derived_views`).
- CPU CSR **SpMV** (published baseline §6.1): 0.53 µs (1 k nnz) → 5.23 ms (4.2 M
  nnz) serial; Rayon 44 µs → 537 µs at the top; **crossover at
  `PAR_SPMV_NNZ_THRESHOLD = 81 920`** (serial wins below, Rayon above, up to 9.7×
  at 4.2 M). SpMM at feature dims per §6.2.
- HNN message passing / convolutions with serial↔Rayon crossovers — §7.1–7.3.
- COO/CSR construction is reachable from the view; a **direct core IR→COO/CSR
  one-call** path is not exposed (PARTIAL, `UIR` Q4).

## 5. Parallel scaling [R]

Crossovers are the measured scaling signal: CSR SpMV serial→Rayon at ~82 k nnz;
HNN per §7.2. Below threshold the one-thread path wins (algorithmic simplicity
beats scheduling overhead); above it Rayon scales toward the core count (24
logical). The Rayon path is the *same* algorithm (row-disjoint), so the gain is
parallel, not algorithmic — the bitwise-identity contract guarantees it.

## 6. GPU decomposition [R]

Published baseline §8: **resident vs readback is the dominant effect** — a
resident chain avoids the staging→map→copy readback that otherwise dwarfs kernel
time; §8.1–8.3. CPU-vs-GPU at the shipped workloads §8.3; the workgroup rule
reproduces §8.4. Device memory from `ArenaStats` §10.2/§13. These are the deploy
costs; **transfer destroys the GPU advantage** for one-shot small ops and wins
only for resident multi-op chains.

## 7. Topology sensitivity [R]

ns/incidence: Steiner 202–405 (grows with block size), Mycielski 471
(triangle-free density), random-uniform 445, **VLAN 606 (network, densest
membership)**, robot 406, flock 307. The constant, not the exponent, is what the
topology changes — the pipeline stays linear.

## 8. The ten questions

1. **IR construction vs derived-view construction dominant?** **[R/I]** Neither
   dominates end-to-end; the **numerical operation** does (CSR SpMV 5.23 ms at
   4.2 M nnz vs ns-scale per accessor lookup and µs-scale ingestion at ≤10⁴ inc).
   Among build stages, ingestion (parse+lower) is ~445 ns/inc; view build is a
   single linear pass over the same incidences.
2. **When does parallel traversal pay?** **[R]** CSR SpMV above ~82 k nnz; below
   that serial wins. Traversal/gather thresholds track the same order.
3. **Memory-bandwidth-bound ops?** **[I from R]** CSR SpMV/SpMM at high nnz (the
   Rayon speedup saturating at ~9.7× on 24 threads indicates bandwidth, not
   compute, is the ceiling).
4. **Allocation-bound?** **[R]** The `*_into` allocation-free CSR paths exist
   precisely because the convenience paths allocated; §6.3. IR construction is
   arena-`push`, amortised.
5. **When does resident GPU win?** **[R]** Resident multi-op chains (§8.1) where
   the upload is amortised across many kernels.
6. **When does transfer destroy it?** **[R]** One-shot small ops with readback —
   staging→map→copy exceeds kernel time (§8.1).
7. **Topology stressing memory most?** **[R/M]** VLAN (network) per-incidence
   (606 ns/inc, densest membership); at fixed incidences the canonical memory is
   topology-independent (record tables scale with decls+arcs, §3).
8. **Which duplication has measurable runtime cost?** **[M/I]** *None in editable
   production code* (conformance §12 / duplication audit): `from_ir` uses
   accessors, CSR serial/Rayon is the approved specialisation. The **derived
   parallel-map memory** (61.1 % overhead, `Option<NodeId>`=16 B) is a *memory*
   duplication, not a runtime one, and is intrinsic to the fast-jump design.
9. **Next highest-value optimisation?** **[I]** The **canonical CBOR codec**
   (`APPROVED-CORE-EDIT: canonical-cbor-codec`): the JSON encoding is ~20× the
   packed record tables, so a compact lossless binary snapshot is the largest
   single memory/transfer win — and it removes the string/derived duplication the
   payload currently declares.
10. **Is HyMeKo a high-performance parallel hypergraph platform today, and with
    what limits?** **[M/R, honest]** **Partially.** *Yes:* real `.hymeko` lowers
    at ~2–5 ns/incidence-per-… (445 ns/inc through P-graph lowering), views build
    deterministically through public accessors, CPU CSR scales to 9.7× on 24
    threads with a measured crossover, and a resident WGPU path exists. *Limits:*
    no CBOR codec (JSON-only snapshots, 20× larger); source emission is
    skeleton-only (no lossless text round-trip); no stable IDs / mutation /
    transactions / incremental view refresh (all BLOCKED at M3/M4); the single-CLI
    invariant is unmet; the GPU crossover is **provisional/bracketed**, not a
    settled per-host verdict.

## 9. Deferred, with reasons (not fabricated)

- **[D] Two-host GPU crossover grid (Phase S).** Amaterasu cross-check not run in
  this consolidation session; a settled crossover requires an exclusively quiet
  host and an alternating-order null (CLAUDE.md §3), which this session could not
  guarantee. The local GPU numbers are reused from the quiet §8/§11 published
  baseline and are labelled **provisional/bracketed** there. No remote result is
  invented.
- **[D] Full topology-family × scale-band Criterion grid at 10⁵–10⁶.** The large
  band *lowers correctly* (§1) and ns/incidence is scale-stable; re-timing the
  full grid would rerun measured work the brief forbids. The large-band *timings*
  beyond the §12 survey are not re-extracted here.

## 10. Rejected hypotheses

- "Accessor-layer guard cost is a bottleneck" — **rejected [M/R]**: it is ns-scale
  vs ms-scale numerical ops; optimising it is the §3-forbidden defensive
  optimization.
- "A view-builder refactor would speed the pipeline" — **rejected**: `from_ir`
  already uses accessors; no duplicated runtime work exists to remove.

## Files

`reports/2026-08-30-large-hypergraph-performance.md` (this),
`reports/data/2026-08-30-large-hypergraph-ingestion.tsv`,
`reports/data/2026-08-30-ir-memory-accounting.tsv`,
`reports/figures/2026-08-30-large-hypergraph-ingestion.svg`. No source, manifest,
dependency, or fixture change. The three IFAC artifacts are untouched.

## HARD STOP

No further optimisation is begun from this report. The next optimisation must be
separately authorised and must cite the measured bottleneck it addresses
(candidate: `APPROVED-CORE-EDIT: canonical-cbor-codec`, per §8 Q9).
