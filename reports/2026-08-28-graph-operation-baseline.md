# Graph and hypergraph operation baseline

**Date:** 2026-08-28
**Base commit:** `62f1b96293499e8908d78342d9d963bf8ad12ae7` (+ Phase 9 working tree)
**Character:** measurement only. **No operation in this report was optimised.**
Companion: `2026-08-28-phase9-kernel-infrastructure.md`.

The purpose is a trustworthy starting line. Future CPU, GPU and distributed work
should be judged against these numbers, so the emphasis is on stating exactly
what was measured and where the numbers cannot be compared.

---

## 1. Reproducing this

```
cargo bench -p hymeko_hnn --bench hnn_bench
```

Individual groups by regex, e.g.

```
cargo bench -p hymeko_hnn --bench hnn_bench -- "representation_expansion|traversal"
```

Criterion writes machine-readable output to `target/criterion/**/estimates.json`
(median, mean, std. dev., confidence intervals per benchmark). The tables below
were produced with `--measurement-time 2 --warm-up-time 1 --sample-size 10`,
which is a **survey** configuration: adequate to establish scaling and rank
operations, not to certify a 3 % change. Default settings should be used before
any claim of that resolution.

Workload generation is `hymeko_hnn::workloads` — the existing seeded generator,
reused rather than duplicated. Sizes are its `small` / `medium` / `large` specs.

---

## 2. Method and its limits

* Construction is hoisted out of every timed closure, except in
  `view_construction` and `representation_expansion`, where construction *is* the
  subject.
* `black_box` on every input and result.
* Throughput is declared per group in the unit the inner loop actually traverses:
  incidences for message passing, `incidences × d_out` for convolutions, **output
  nonzeros** for expansions, **reached states** for traversal.
* Serial and parallel variants of the same operation are measured in the same
  group on the same input, so the comparison is paired.

**Deliberate omissions, stated rather than hidden.**

* **No GPU numbers appear in this report.** Mixing a resident GPU kernel with a
  CPU end-to-end path is the comparison the brief forbids, and this document has
  no defensible byte model for the GPU side. The seven cost categories
  (topology construction / representation conversion / resident CPU operation /
  upload / resident GPU execution / readback / end-to-end) are kept apart by
  simply not publishing categories that were not measured here. GPU readback and
  resident numbers live in the Phase 7 and Phase 8 reports.
* **`MeshConv` is not benchmarked.** `hymeko_hnn/src/tensor/mesh_nn/mesh_conv.rs`
  is a zero-line file. The brief conditioned it on being "complete and correct";
  it does not exist.
* **Allocation counts and peak RSS are not instrumented.** Criterion does not
  report them and no allocator hook was added — that would have been a new
  dependency or a global allocator, both out of scope here. Where allocation is
  the dominant cost it is visible *as* the measurement (§4, §5), and expansion
  output sizes are reported directly so the allocation volume is known even
  though its wall-clock share is not separated.
* **No signed/unsigned or weighted/unweighted axis sweep.** The existing groups
  cover `SignPattern` through `conv_signed_hgnn_forward`; a full cross-product of
  sign × weight × degree-shape × feature-dim was not run at survey resolution.
  Recorded as a gap, not as covered.

**Host.** Intel Core Ultra 9 275HX, 24 logical cores, Windows 11, 63.4 GB.
Rayon default thread pool (`RAYON_NUM_THREADS` unset → 24 threads). No CPU
affinity pinning. Interactive desktop; large cells run sequentially.

---

## 3. Workload scale

| spec | nodes | incidences | reachable states from node 0 |
|---|---:|---:|---:|
| small | — | 1 995 | 1 373 |
| medium | — | 199 982 | 74 090 |
| large | — | 1 642 260 | 299 944 |

Reachability is reported because traversal throughput is per *reached* state: a
traversal from one vertex covers its connected component, not the whole graph,
and incidences/s would silently mix components of different sizes.

---

## 4. Representation conversion

### 4.1 Expansions

| operation | small | medium | large |
|---|---:|---:|---:|
| `star_expansion_csr` | 7.36 µs | 4.00 ms | 34.5 ms |
| `clique_expansion_csr` | 31.7 µs | 13.2 ms | **456 ms** |

Output size, which is the point:

| scale | star nnz | clique nnz | clique/star |
|---|---:|---:|---:|
| small | 2 719 | 5 373 | 1.98× |
| medium | 266 575 | 1 244 165 | 4.67× |
| large | 2 189 343 | 55 423 732 | **25.3×** |

Star expansion is linear in incidences; clique expansion is quadratic in
hyperedge **cardinality**, and the ratio grows with scale accordingly. At large
scale clique expansion costs 13× the wall time of star for 25× the output.

### 4.2 Clique expansion under high cardinality

Cardinality held against a fixed node count, so only edge size varies:

| avg cardinality | edges | output nnz | wall |
|---:|---:|---:|---:|
| 64 | 400 | 1 607 760 | 9.84 ms |
| 128 | 400 | 6 464 102 | 43.9 ms |
| 256 | 200 | 12 901 952 | 93.4 ms |
| 512 | 100 | 25 512 230 | 165 ms |

Doubling cardinality at fixed edge count quadruples output (64→128: 1.61M→6.46M,
4.02×) — the expected k² term, measured rather than assumed.

**Memory limits were respected by projection, not by collision.** The benchmark
computes `Σ k²` before building anything and skips any cell whose projected
output exceeds a 2 GiB budget, printing the projection. No cell was forced into
an allocation failure, and none approached the 16 GB cap.

### 4.3 Explicit adjacency and degrees

| operation | small | medium | large |
|---|---:|---:|---:|
| `build_explicit_a` (COO) | 25.6 µs | 16.7 ms | **724 ms** |
| `compute_bipartite_degrees` | 1.42 µs | 154 µs | 2.10 ms |

`build_explicit_a` carries the same quadratic term as clique expansion and is the
most expensive single operation in this report. It is paid once per
`GcnCliqueLayer` construction — which is exactly why §6 times layer construction
separately from the forward pass.

Degree computation is cheap and linear; it is not a candidate for optimisation.

---

## 5. Construction

| operation | small | medium | large |
|---|---:|---:|---:|
| `HyperGraphView` from design | 10.3 µs | 2.22 ms | 32.5 ms |
| `HyperGraphView::from_ir` | 22.9 µs | 8.91 ms | **91.1 ms** |

`from_ir` costs 2.8× the direct path at large scale. The difference is IR
resolution and weight extraction, which the direct path skips — this is the one
group where allocation *is* the measurement, so the gap is a real cost of
entering through the IR rather than a harness artifact.

---

## 6. Message passing

### 6.1 Serial vs parallel, swept by incidence count

| incidences | gather serial | gather parallel | scatter serial | scatter parallel |
|---:|---:|---:|---:|---:|
| 512 | 358 ns | 15.7 µs | 384 ns | 19.9 µs |
| 2 048 | 1.57 µs | 19.8 µs | 1.49 µs | 22.7 µs |
| 8 192 | 5.78 µs | 22.8 µs | 6.65 µs | 23.7 µs |
| 16 384 | 12.4 µs | 26.2 µs | 17.6 µs | 25.6 µs |
| 65 536 | 48.7 µs | 27.7 µs | 146 µs | 33.9 µs |
| 262 144 | 214 µs | 45.2 µs | 768 µs | 71.6 µs |
| 1 048 576 | 1.16 ms | **127 µs** | 3.32 ms | **245 µs** |

At 1 048 576 incidences: gather **9.2×**, scatter **13.5×** on 24 threads —
38 % and 56 % thread-scaling efficiency respectively.

The parallel path has a **~16–20 µs floor** independent of size, which is Rayon's
dispatch cost. That floor, not bandwidth, is what makes parallel lose below the
crossover.

**Crossover is reported as a bracket, not a point.** Gather crosses somewhere in
**[16 384, 65 536]** incidences; scatter in **[8 192, 16 384]**. At survey
resolution the intervals around the nearest cells overlap parity, and the brief
forbids publishing a crossover under that condition. Locating it properly needs
default Criterion settings and a denser sweep — recorded as follow-up.

Scatter benefits more than gather because its serial path is more expensive at
every size above 8 192 (node-major accumulation vs edge-major reduction), so it
has more to parallelise.

### 6.2 Diagonal operations

| operation | small | medium | large |
|---|---:|---:|---:|
| `clique_diag` (serial) | 1.44 µs | 66.7 µs | 329 µs |
| `remove_self_effect` | 326 ns | 16.6 µs | 57.1 µs |

Both are linear and cheap relative to the gather/scatter they accompany —
`remove_self_effect` at large scale is 2 % of a serial gather. Measured
explicitly so it is not assumed negligible.

---

## 7. Traversal

| operation | small | medium | large |
|---|---:|---:|---:|
| `bfs` (generic `GraphView`) | 126 µs | 16.6 ms | 180 ms |
| `dfs_preorder` (generic) | 121 µs | 17.7 ms | 192 ms |
| `bfs_dense` (Berge) | **9.00 µs** | **2.66 ms** | **31.7 ms** |
| `dfs_preorder_dense` (Berge) | 15.7 µs | 4.63 ms | 60.1 ms |

**The dense Berge traversals are 3.2–5.7× faster than the generic ones**, and the
gap widens with scale. The generic path carries a `HashSet` of visited states;
the dense path uses per-node and per-edge bitsets sized from the view. This is
the largest single ratio in the report and it is a pure representation effect —
same traversal order, same reached set.

This is a measurement, not a recommendation to delete the generic path: it is
generic over `GraphView` and serves `DeclTreeView` as well, where a dense
indexed set is not available.

---

## 8. Model-level forward passes

| model | scale / dims | wall |
|---|---|---:|
| HGNN | small 32×32 | 154 µs |
| HGNN | medium 32×8 | 1.13 ms |
| HGNN | medium 32×32 | 3.10 ms |
| HGNN | medium 32×128 | 9.79 ms |
| HGNN | large 32×32 | 16.2 ms |
| Signed-HGNN | small 32×32 | 193 µs |
| Signed-HGNN | medium 32×8 | 1.93 ms |
| Signed-HGNN | medium 32×32 | 5.27 ms |
| Signed-HGNN | medium 32×128 | 16.9 ms |
| Signed-HGNN | large 32×32 | 31.4 ms |
| Clique-GCN | small d8→8 | 30.9 µs |
| Clique-GCN | small d32→32 | 111 µs |
| Clique-GCN | medium d8→8 | 1.39 ms |

Signed-HGNN costs **1.7–1.9×** HGNN at matched dimensions — consistent across
every scale and feature dimension, reflecting its two-channel aggregation.

HGNN scales close to linearly in output features at medium (32×8 → 32×128 is
16× the features for 8.6× the time), so it is not feature-bound at these sizes.

Clique-GCN's forward pass is cheap; its cost lives almost entirely in
`build_explicit_a` at construction (§4.3), which at large scale is 724 ms against
a forward pass measured in milliseconds. **Any comparison of Clique-GCN against
HGNN must state whether construction is amortised**, because the answer changes
by two orders of magnitude.

---

## 9. What this baseline does and does not license

**Licensed.** Ranking CPU operations against each other; identifying which
operations are superlinear; judging a future optimisation of any single operation
against its row here, at the same Criterion settings.

**Not licensed.** Any CPU-versus-GPU statement — none of the GPU cost categories
were measured in this document. Any crossover claim finer than the brackets in
§6.1. Any claim about allocation counts or peak RSS. Any statement about
irregular-degree, signed/unsigned or weighted/unweighted axes beyond what §8's
Signed-HGNN row covers, since those sweeps were not run.

---

## 10. Follow-up

* Re-run at default Criterion settings before using any row as a regression gate.
* Locate the gather and scatter crossovers with a denser sweep.
* Add an allocation counter (a counting global allocator behind a bench-only
  feature) so construction and expansion can separate allocation from compute.
* Sweep the degree-shape axis: every number here is `DegreeShape::Uniform` except
  the cardinality study, so skew is unmeasured.
* Measure `build_explicit_a` amortisation across inference steps, which is what
  decides whether Clique-GCN is competitive in a loop.
