# CPU and memory optimisation of `hymeko_core` + `hymeko_hnn`

**Date:** 2026-08-24
**Plan:** `docs/plans/2026-08-24-cpu-optimization/`
**Backend proposal:** `docs/backend-abstraction.md`
**Git SHA at start:** `45b0512` (working tree clean)
**Approval tokens used:** `APPROVED-CORE-EDIT: csr-spmv-spmm-into` (2026-08-24, user chat)

---

## 1. Summary

Reduced CPU time in the sparse-tensor and hypergraph message-passing hot paths
by **1.2×–25×** depending on kernel and scale, with **bitwise-identical**
numerical results and no public API change. The largest wins are in the
convolution forward passes (17–25× at scale) and CSR SpMM (up to 13×).

Two governance gaps were closed first: this workspace had no `CORE.YAML` /
`tools.yaml` (a §1/§11 halt condition), and the CSR work required an explicit
core-edit approval. Both were resolved with the user before any code changed.

Everything claimed below is measured. Where a claim could not be separated from
measurement noise, that is stated rather than glossed.

---

## 2. What was optimised

### 2.1 `hymeko_core/src/tensor/representations/tensor_csr.rs` (core, under token)

| Change | Mechanism |
|---|---|
| Added `spmv_into` / `spmm_into` (+ `_serial` / `_parallel` variants) | Allocation-free; caller owns the output buffer |
| `spmv` / `spmm` now delegate | Signatures unchanged; one allocation each, as before |
| Rayon partitioning by **output row** (`par_chunks_mut`) | Rows are disjoint → no atomics, no locks, no reassociation |
| Rewrote the SpMM inner loop to zip two equal-length slices | The old flat-index form (`y_flat[y_off + f] += …`) could not be proven in-bounds, so LLVM kept a bounds check per element and did not vectorise |
| Per-row zeroing inside `row_axpy` | Removes a separate clearing pass over the whole output; the row is already in cache |
| Chunk = `num_rows / (threads × 8)` | Several tasks per worker so work-stealing can rebalance irregular rows |

Both dispatch thresholds are **measured**, not assumed — see §6.

### 2.2 `hymeko_hnn/src/tensor/message_passing.rs`

- **Scatter and `clique_diag` moved to the node-first incidence CSR.** The
  edge-major form read-modify-writes a shared `y[v]`, which would need atomics
  or per-thread partials to parallelise. Walking `node_offsets` / `flat_node_*`
  makes every node's output independently computable — no atomics, no locks, no
  reduction buffers.
- **Serial / parallel / dispatching entry points** for gather and scatter.
- **Diagonal removal fused into the scatter** in `implicit_clique_step`,
  removing a second full pass over `y`.
- **Edge-major kept as the small-input path** (see §7 — this was a measured
  correction, not the original design).
- `build_explicit_a`: hoisted a per-hyperedge `Vec` allocation out of the loop.
- Removed a byte-identical duplicate of `inc_scalar_signed` (§6.1).

### 2.3 `conv/hgnn.rs` and `conv/signed_hgnn.rs`

The dominant defect. Both forward passes ran the entire gather → scale →
scatter pipeline **once per output feature**, allocating three temporaries
(HGNN) or five (Signed-HGNN) on every iteration — `3·d_out` / `5·d_out`
allocations and `d_out` full traversals of the incidence structure.

- **Feature tiling**: one traversal serves a tile of features, with the feature
  loop innermost over contiguous equal-length slices. Allocation is now `O(1)`
  in `d_out` (2 buffers for HGNN, 4 for Signed-HGNN).
- **Scratch bounded** by `FEATURE_TILE_BUDGET` (~4 MiB of `f32`) so the
  memory-for-traversals trade degrades gracefully instead of scaling with
  `edges × features`.
- **Dense `X·Θ` loop order** changed from `i→j→k` to `i→k→j`, so `Θ` is walked
  contiguously in the innermost loop. Each output still accumulates over `k` in
  `0..d_in` order, so the result is unchanged bitwise.
- **Scatter half on the node-first CSR**, parallel over nodes.
- Steps 4 and 5 fused (normalisation + activation applied while the row is hot).

### 2.4 `traversal/hypergraphview.rs`

`build_csr_node_to_edges` replaced an `O(nnz log nnz)` comparison sort with an
`O(nnz + num_nodes)` **counting sort**, and no longer needs `&mut` access to
`pairs`. Valid because counting sort is stable and the input is already
`(edge, node)`-ordered and deduplicated, so each node's bucket comes out in
increasing edge order — the same unique `(node, edge)` ordering the comparison
sort produced.

### 2.5 `conv/gcn_clique.rs`

`forward_relu` writes through `spmm_into` into the caller's buffer instead of
allocating an `n × d_out` `Vec` to copy out of, and applies ReLU in place. Same
`i→k→j` matmul reordering.

---

## 3. Why the numbers do not change

The parallel and restructured paths are **bitwise identical** to the originals,
not merely within tolerance. That is a stronger claim than usual, and it is what
made the parallel paths safe to enable by default.

`HyperGraphView` materialises the same incidence multiset twice — once ordered
by `(edge, node)`, once by `(node, edge)`. The old edge-major scatter visited
`e = 0..m` in order, so node `v` accumulated its contributions in strictly
increasing edge order. Walking `v`'s slice of the node-first array visits
exactly those incidences, also in increasing edge order. **The summation
sequence per output element is identical**, so partitioning over nodes cannot
reassociate anything — no two threads ever touch the same accumulator.

The same argument covers CSR SpMV/SpMM (partitioned by output row) and the
feature-tiled convolutions (tiling does not reorder the `k` or `p` loops).

Two places where this required care, and where the code deliberately avoids the
"obvious" simplification:

- `x_e * inv_de[e] * w_e[e]` is **not** folded into `x_e * (inv_de[e] * w_e[e])`.
  Floating-point multiplication is not associative; precomputing the product
  would perturb the result.
- The diagonal-removal fusion computes `acc - diag[v]·x[v]`, which is exactly
  what `y[v] = acc; y[v] -= diag[v]·x[v]` computed, in the same order.

This is **asserted by test**, not assumed — see §4.

---

## 4. Test results

```
cargo test --workspace --release
```

| Layer | Result |
|---|---|
| Unit (`hymeko_core` lib) | **17 / 17 pass** |
| Integration (`tests/mod.rs`) | **138 total — 137 pass, 1 known fixture drift** |

**Known drift, unchanged and untouched:**
`domain_transformations::parse_files::test_load_meta_geometry` —
`assertion left == right failed: meta kinematics node count drifted; left: 58, right: 57`.
Pre-existing, caused by upstream parser drift, documented in `README.md`, and
**not** a regression from this work. It was not "fixed" by editing the fixture
or the assertion.

**31 tests added** (integration count 107 → 138), all passing:

| File | Tests | What they pin |
|---|---:|---|
| `test_csr_kernels.rs` | 11 | SpMV/SpMM vs a dense reference; serial≡parallel **bitwise**; poisoned-buffer overwrite contract; empty / singleton / all-empty-rows / `feature_dim = 0`; dimension-mismatch panics |
| `test_message_passing_parallel.rs` | 8 | Node-major scatter and `clique_diag` vs verbatim **edge-major reference oracles**, bitwise; fused vs unfused clique step; `use_abs = false` sign preservation; empty / edgeless / singleton |
| `test_conv_forward.rs` (conv) | 4 | HGNN and Signed-HGNN forward vs verbatim **pre-optimisation reference oracles**, bitwise, incl. the neutral sign arm |
| `test_conv_forward.rs` (gcn) | 4 | `GcnCliqueLayer` — previously untested: weight-homogeneity, zero-weight identity, oversized-buffer contract, trait/inherent agreement |
| `test_view_construction.rs` | 4 | Counting-sort ordering; both orientations hold the same multiset (bitwise weights); offset well-formedness; empty IR |

The assertions are **bitwise** (`f32::to_bits`) wherever the claim is order
preservation. A tolerance assertion would pass even if a rewrite reassociated
the sums, which is precisely the defect worth catching.

The reference oracles are verbatim transcriptions of the *pre-change*
algorithms, kept in the test tree on purpose: comparing the new code against
itself would prove nothing.

### Static gates

- `cargo fmt --all -- --check` — **pass**
- `cargo clippy --workspace --all-targets -- -D warnings` — **pass**

---

## 5. Benchmark results (before → after)

Criterion, medians with confidence intervals, deterministic seeded workloads,
default release profile, default Rayon thread count (24). Full tables
regenerable at any time with `scripts/dev/criterion_report.ps1`.

### 5.1 CSR SpMV / SpMM

| Benchmark | before | after | speedup | after throughput |
|---|---:|---:|---:|---:|
| `spmv/small` (8 k nnz) | 3.291 µs | 2.696 µs | 1.22× | 2.96 Gelem/s |
| `spmv/medium` (800 k) | 416.4 µs | 93.49 µs | **4.45×** | 8.56 Gelem/s |
| `spmv/large` (4 M) | 6.310 ms | 694.7 µs | **9.08×** | 7.83 Gelem/s |
| `spmm/small/fd1` | 12.007 µs | 8.660 µs | 1.39× | 920 Melem/s |
| `spmm/small/fd8` | 30.427 µs | 12.295 µs | **2.47×** | 5.18 Gelem/s |
| `spmm/small/fd32` | 62.683 µs | 28.942 µs | **2.17×** | 8.81 Gelem/s |
| `spmm/small/fd128` | 123.81 µs | 98.514 µs | 1.26× | 10.35 Gelem/s |
| `spmm/medium/fd1` | 1.388 ms | 140.34 µs | **9.89×** | 5.70 Gelem/s |
| `spmm/medium/fd8` | 4.444 ms | 671.22 µs | **6.62×** | 9.53 Gelem/s |
| `spmm/medium/fd32` | 8.444 ms | 1.426 ms | **5.92×** | 17.94 Gelem/s |
| `spmm/medium/fd128` | 38.786 ms | 4.164 ms | **9.32×** | 24.59 Gelem/s |
| `spmm/large/fd1` | 12.431 ms | 996.89 µs | **12.47×** | 5.46 Gelem/s |
| `spmm/large/fd32` | 105.19 ms | 7.949 ms | **13.23×** | 21.91 Gelem/s |

### 5.2 Hypergraph message passing

| Benchmark | before | after | speedup | after throughput |
|---|---:|---:|---:|---:|
| `mp_gather/small` | 885.3 ns | 947.4 ns | 0.93× | 2.11 Gelem/s |
| `mp_gather/medium` | 114.98 µs | 35.628 µs | **3.23×** | 5.61 Gelem/s |
| `mp_gather/large` | 1.853 ms | 201.80 µs | **9.18×** | 8.14 Gelem/s |
| `mp_scatter/small` | 1.016 µs | 998.3 ns | 1.02× | 2.00 Gelem/s |
| `mp_scatter/medium` | 128.09 µs | 54.524 µs | **2.35×** | 3.67 Gelem/s |
| `mp_scatter/large` | 1.939 ms | 253.51 µs | **7.65×** | 6.48 Gelem/s |
| `mp_clique_diag/small` | 909.2 ns | 988.7 ns | 0.92× | 2.02 Gelem/s |
| `mp_clique_diag/medium` | 126.17 µs | 73.558 µs | **1.72×** | 2.72 Gelem/s |
| `mp_clique_diag/large` | 1.861 ms | 297.89 µs | **6.25×** | 5.51 Gelem/s |
| `mp_implicit_clique_step/small` | 2.129 µs | 1.981 µs | 1.07× | 2.01 Gelem/s |
| `mp_implicit_clique_step/medium` | 286.35 µs | 98.047 µs | **2.92×** | 4.08 Gelem/s |
| `mp_implicit_clique_step/large` | 3.983 ms | 515.55 µs | **7.73×** | 6.37 Gelem/s |

### 5.3 Convolutions — the largest wins

| Benchmark | before | after | speedup |
|---|---:|---:|---:|
| `hgnn/small 32×32` | 541.17 µs | 164.57 µs | **3.29×** |
| `hgnn/medium 32×8` | 7.655 ms | 1.140 ms | **6.72×** |
| `hgnn/medium 32×32` | 39.010 ms | 3.069 ms | **12.71×** |
| `hgnn/medium 32×128` | 185.69 ms | 8.946 ms | **20.76×** |
| `hgnn/large 32×32` | 272.55 ms | 15.455 ms | **17.64×** |
| `signed_hgnn/small 32×32` | 640.45 µs | 200.46 µs | **3.19×** |
| `signed_hgnn/medium 32×8` | 22.545 ms | 1.880 ms | **11.99×** |
| `signed_hgnn/medium 32×32` | 99.729 ms | 5.148 ms | **19.37×** |
| `signed_hgnn/medium 32×128` | 427.33 ms | 16.660 ms | **25.65×** |
| `signed_hgnn/large 32×32` | 671.57 ms | 28.904 ms | **23.23×** |

The speedup **grows with `d_out`** (6.7× → 12.7× → 20.8× at medium scale as
`d_out` goes 8 → 32 → 128). That is the signature the plan predicted for
hypothesis H4 (per-feature re-traversal), and it is why the `d_out` sweep was
designed into the benchmark rather than a single dimension being measured.

### 5.4 Construction

| Benchmark | before | after | delta |
|---|---:|---:|---:|
| `from_design/small` | 8.646 µs | 9.006 µs | +4.2 % |
| `from_design/medium` | 2.379 ms | 2.416 ms | +1.6 % |
| `from_design/large` | 30.178 ms | 29.673 ms | −1.7 % |
| `from_ir/small` | 81.930 µs | 62.598 µs | **1.31×** |
| `from_ir/medium` | 16.126 ms | 14.837 ms | 1.09× |
| `from_ir/large` | 141.04 ms | 142.90 ms | +1.3 % |

The counting sort wins where the replaced sort ran serially and is a wash at
large scale — because above 8192 elements the old code used
`par_sort_unstable_by_key`, i.e. all 24 cores, against a single-threaded
counting sort. Kept anyway: simpler, `O(nnz + n)`, and drops the `&mut`
requirement. The +1.3 % at large is marginally outside the before-CI and is
reported as a small real cost, not as noise.

### 5.5 Allocation cost, isolated

Reported separately so "allocation removed" is never mistaken for "kernel got
faster" — a confound the plan flagged in advance.

| Scale | `spmv` (allocating) | `spmv_into` (hoisted buffer) | saved |
|---|---:|---:|---:|
| small | 2.857 µs | 2.789 µs | 2.4 % |
| medium | 88.952 µs | 63.755 µs | **28.3 %** |
| large | 700.23 µs | 583.45 µs | **16.7 %** |

---

## 6. Threshold calibration (measured, not guessed)

The first draft of every dispatch threshold was a placeholder. The sweeps showed
**all of them were wrong**, in the same direction — too eager to parallelise.
This section is the evidence.

### 6.1 SpMV — crossover ≈ 80 000 nnz

| nnz | serial | parallel | winner |
|---:|---:|---:|---|
| 1 024 | 429 ns | 18.43 µs | serial 43× |
| 16 384 | 5.199 µs | 22.11 µs | serial 4.3× |
| 65 536 | 21.30 µs | 24.41 µs | serial 1.15× |
| 262 144 | 108.28 µs | 33.63 µs | **parallel 3.2×** |
| 4 194 304 | 4.675 ms | 487.36 µs | **parallel 9.6×** |

Rayon's fork/join floor is ~18.4 µs. `PAR_SPMV_NNZ_THRESHOLD = 81_920`
(the placeholder 65 536 was measurably too low — parallel loses by 15 % there).

### 6.2 SpMM — crossover ≈ 262 000 MACs

| feature_dim | work | serial | parallel |
|---:|---:|---:|---:|
| 1 | 16 k | 17.603 µs | 26.274 µs |
| 8 | 131 k | 20.469 µs | 24.572 µs |
| 32 | 524 k | 47.923 µs | 27.355 µs |
| 128 | 2.1 M | 170.52 µs | 37.516 µs |

**SpMM needs a threshold 3× higher than SpMV.** Its inner loop runs over two
contiguous equal-length slices and vectorises, reaching ~11 Gelem/s serially
against SpMV's ~3 — so more work is needed to amortise the same fork/join cost.
A single shared constant would misdispatch one of them.
`PAR_SPMM_WORK_THRESHOLD = 262_144`.

### 6.3 Message passing — crossover ≈ 25–45 k incidences

| incidences | gather serial | gather par | scatter serial | scatter par |
|---:|---:|---:|---:|---:|
| 512 | 223.4 ns | 14.916 µs | 249.3 ns | 17.768 µs |
| 8 192 | 3.382 µs | 21.700 µs | 4.204 µs | 21.931 µs |
| 16 384 | 6.733 µs | 22.445 µs | 8.659 µs | 23.195 µs |
| 65 536 | 30.300 µs | 25.820 µs | 113.30 µs | 35.187 µs |
| 1 048 576 | 730.17 µs | 118.96 µs | — | — |

Gather and scatter do **not** share a crossover (≈45 k vs ≈25 k): serial
node-major scatter reads `x_edges` randomly and falls off a cache cliff between
16 k and 65 k (1.89 Gelem/s → 578 Melem/s). One constant is kept at roughly the
geometric mean, `PAR_INCIDENCE_THRESHOLD = 32_768`, because the regret either
side is small (~1.4× on gather in 33–45 k, ~1.2× on scatter in 25–33 k) and a
second knob is not worth that. The placeholder 16 384 was 3.3× too eager.

### 6.4 A second threshold regime for blocked kernels

`PAR_BLOCKED_WORK_THRESHOLD = 262_144` applies **only** to the dense `X·Θ`
transform, which walks two contiguous rows. It deliberately does *not* apply to
the gather/scatter tiles: those loop over `width` contiguous features but reach
each incidence through a *random* `v·d_out + f0` slice, measured at ~2.5 Gelem/s
serially, so they use `PAR_INCIDENCE_THRESHOLD`. An intermediate draft applied
the blocked constant to the tiles as well; see §7 for why that could not be
cleanly resolved by measurement.

---

## 7. Regressions found, and what was done about them

Per CLAUDE.md §3 the default attribution for a regression is "a bug was
introduced". Three cells breached the 10 % gate; **all three were my bugs**, and
all are now cleared.

| Cell | Peak regression | Root cause | Resolution | Final |
|---|---:|---|---|---:|
| `mp_scatter/small` | +13.8 % | Node-major does 2 random loads per incidence (`edge_weight[e]`, `x_edges[e]`) where edge-major hoists `x_edges[e]` per edge and does 1 random write. Node-major wins **only** via parallelism; run serially it is strictly more work. | Dispatch inputs below the threshold to `scatter_nodes_from_edges_edge_major` (bitwise identical) | **−1.7 %** |
| `mp_implicit_clique_step/small` | +15.4 % | Same, inherited through the fused scatter | Below threshold, use edge-major scatter + separate `remove_self_effect`; fusion only pays once the traversal is node-major | **−6.9 %** |
| `mp_clique_diag/small` | +29.6 % | **Double zeroing.** `clique_diag` allocated with `vec![F::zero(); n]` then called `clique_diag_into`, whose fully-overwritten postcondition obliges its edge-major branch to clear the buffer *again* | Split the accumulating kernel from the clearing contract; `clique_diag` dispatches directly. Plus `#[inline]` on the accumulator | **+8.7 %** (under gate) |

`mp_clique_diag/small` remains +8.7 % against baseline. It is under the gate and
accepted: the residual is the dispatch branch plus `&mut [F]` aliasing that the
original's local `Vec` did not have, and it buys the 6.25× large-scale win.

`mp_gather/small` sits at −7 % (0.93×), also under the gate. Gather's traversal
was not restructured at all; the delta is code-layout and a dispatch branch.

### Measurement-integrity caveat (important)

`conv_hgnn_forward/serial/small/32×32` measured **135.9 → 147.9 → 164.6 µs**
across three runs in this session. Runs 1 and 3 used the **same dispatch path**
(parallel tiles) — so a 21 % spread appeared on *identical code*. Sustained
benchmarking on a laptop CPU drifts (thermal, and allocator state).

Consequences, stated plainly:

- The conv-tile threshold choice (§6.4) is justified by **mechanism**, not by a
  clean measurement separation — that cell's serial-vs-parallel difference is
  below this session's noise floor. A contiguous A/B on a cold machine would be
  needed to settle it.
- Small-scale cells generally carry a noise band of this order. Single-digit
  percentages on `small` rows should not be over-read.
- The **before** baselines were taken first, on a cooler machine, and the
  **after** measurements later on a warmer one. That biases the reported
  speedups **downward**, so the large-scale claims (17–25×) are conservative,
  not inflated. They are also far outside any plausible drift band.

---

## 8. Bottlenecks identified

Ranked by measured cost.

1. **Per-output-feature re-traversal in the convolutions** — dominant. At
   `d_out = 32` the HGNN forward cost 272 ms where a single gather+scatter over
   the same structure costs 3.8 ms, i.e. ~70×, with per-incidence throughput
   5–10× worse than the raw kernels. **Fixed** (§2.3).

2. **`extract_ref_weight_scalar` allocates a `Vec` per incidence** — the
   dominant remaining cost in `from_ir`, and **not fixed** (see §11). Isolated
   with a discriminating benchmark that builds structurally identical IRs
   differing only in `weights: Some(..)` vs `None`:

   | scale | `weights_some` | `weights_none` | allocation share |
   |---|---:|---:|---:|
   | small | 68.22 µs | 17.17 µs | **75 %** |
   | medium | 17.55 ms | 10.69 ms | 39 % |
   | large | 169.4 ms | 109.1 ms | 36 % |

   At large scale that single `Vec::new()` costs ~60 ms of 169 ms.

3. **Output allocation in `spmv`/`spmm`** — 17–28 % at medium/large (§5.5).
   Fixed by the `_into` API; the allocating wrappers remain for compatibility.

4. **Bounds-check-blocked vectorisation in the SpMM inner loop** — the flat
   index form could not be proven in-bounds. Fixed; visible as the serial-path
   gain at `spmm/small/fd8` (2.47×, below any parallel threshold).

5. **Serial-only CSR and message passing** — fixed, with measured thresholds.

6. **`from_ir` is the slowest operation in the workspace per element**
   (11.6 Melem/s vs 5–8 Gelem/s for the message-passing kernels). Partially
   addressed (counting sort); the remainder is item 2 plus the triple-array
   allocations noted in §11.

---

## 9. Memory

### 9.1 Allocations removed (exact, per call)

These are counted from the code, not estimated. This is where the memory work
actually happened — the transient allocation *rate* in the hot paths, not the
resident footprint.

| Call | before | after |
|---|---:|---:|
| `HgnnLayer::forward_pass` | `3·d_out + 1` | **2** |
| — at `d_out = 128` | 385 | **2** |
| `SignedHgnnLayer::forward_pass` | `5·d_out + 2` | **4** |
| — at `d_out = 128` | 642 | **4** |
| `spmv_into` / `spmm_into` | 1 (output) | **0** |
| `build_explicit_a` | 1 per hyperedge | **1 total** |
| `GcnCliqueLayer::forward_relu` | 2 (`h` + spmm result) | **1** (`h`) |
| `scatter_nodes_from_edges` | 0 (+1 zeroing pass) | 0 (no zeroing pass) |

The convolution figures are the substantive ones: at `d_out = 128` the
Signed-HGNN forward went from **642 allocations per forward pass to 4**.

### 9.2 Peak working set (measured)

Windows has no RSS cgroup gate, so the §4 16 GB cap is observed and asserted,
not kernel-enforced. Sampled via `scripts/dev/measure_peak_memory.ps1`.

| Target | Scope | Peak working set | Peak paged | vs 16 GB cap |
|---|---|---:|---:|---|
| `interning_bench` | full | **72.39 MiB** | 75.04 MiB | ok |
| `csr_bench` | large cells | **122.60 MiB** | 149.05 MiB | ok |
| `hnn_bench` | conv large cells | **416.62 MiB** | 428.95 MiB | ok |

`interning_bench` is the **untouched** frontend path and the only target
comparable to the pre-existing ~69 MiB figure on this host: it measures
72.39 MiB, i.e. unchanged within run-to-run variation. That is the control.

`csr_bench` and `hnn_bench` have **no "before" peak-WS counterpart** — the
benchmark harness that generates these workloads did not exist prior to this
change, so there is nothing to compare against. Stating that rather than
inventing a baseline. Both are far under the plan's 1 GiB budget.

### 9.3 The one place memory increased

The feature-blocked convolutions hold `num_edges × tile` intermediates where the
old code held `num_edges`. This is a deliberate memory-for-traversals trade and
is bounded by `FEATURE_TILE_BUDGET` at ~4 MiB of `f32`: at 100 k edges the tile
clamps to 10 features rather than growing with `d_out`. On a graph large enough
that even one feature exceeds the budget, the tile floors at 1 and behaviour
degrades to the old per-feature traversal instead of allocating without limit.

---

## 10. Thread scaling

`RAYON_NUM_THREADS` swept per process invocation (Rayon's pool is process-wide
and initialised once, so sweeping inside one process would silently measure the
first pool size for every cell). Driver: `scripts/dev/bench_thread_sweep.ps1`.

| threads | `spmv/large` | `spmm/large fd32` | `mp_scatter/large` | `hgnn/large 32×32` |
|---:|---:|---:|---:|---:|
| 1 | 5.851 ms | 64.271 ms | 2.741 ms | 130.69 ms |
| 4 | 1.986 ms (2.95×) | 19.491 ms (3.30×) | 798.5 µs (3.43×) | 33.786 ms (3.87×) |
| 8 | 1.149 ms (5.09×) | 11.072 ms (5.81×) | 577.2 µs (4.75×) | 21.151 ms (6.18×) |
| 16 | 818.8 µs (7.15×) | 8.545 ms (7.52×) | 374.4 µs (7.32×) | 16.509 ms (7.92×) |
| 24 | 650.7 µs (8.99×) | 7.852 ms (8.18×) | 292.5 µs (9.37×) | 15.185 ms (8.61×) |

**Scaling efficiency** at 24 threads is 34–39 %, at 16 threads 45–50 %. Expected
for sparse, randomly-accessed, memory-bandwidth-bound kernels, and this host's
24 logical cores are a hybrid P/E-core mix, so linear scaling was never
available. Returns flatten past 16 threads.

### 10.1 Algorithmic gain vs parallel gain, separated

The `t1` column is the *new* code on one thread, so comparing it to the serial
baseline separates the two sources of speedup. This is the most diagnostic table
in the report.

| kernel | baseline (serial) | t1 (new, 1 thread) | algorithmic | × parallel (24t) | total |
|---|---:|---:|---:|---:|---:|
| `conv_hgnn/large` | 272.55 ms | 130.69 ms | **2.09×** | 8.61× | 17.64× |
| `csr_spmm/large fd32` | 105.19 ms | 64.271 ms | **1.64×** | 8.18× | 13.23× |
| `csr_spmv/large` | 6.310 ms | 5.851 ms | 1.08× | 8.99× | 9.08× |
| `mp_scatter/large` | 1.939 ms | 2.741 ms | **0.71×** | 9.37× | 7.65× |

Two things worth reading off it:

- **`csr_spmm` gained 1.64× before any thread was added** — that is the
  bounds-check removal and vectorisation of the inner loop, independent of
  Rayon.
- **`mp_scatter` at one thread is 41 % *slower* than the original.** The
  node-major traversal is genuinely more work per incidence; its 7.65× win is
  parallelism overcoming that penalty. This is the quantified justification for
  keeping edge-major as the small-input path (§7) — without the fallback, every
  sub-threshold caller would have paid that 41 % for nothing.

### 10.2 `target-cpu=native`

| Benchmark | default | `-C target-cpu=native` | delta |
|---|---:|---:|---:|
| `csr_spmm/large/fd32` | 7.949 ms | 6.600 ms | **−17.0 %** |
| `csr_spmv/large` | 694.7 µs | 650.1 µs | −6.4 % |
| `conv_hgnn/large 32×32` | 15.455 ms | 15.467 ms | +0.1 % |
| `mp_scatter/large` | 253.5 µs | 262.4 µs | +3.5 % |

The gain concentrates entirely in the kernel with the contiguous vectorisable
inner loop. **Not recommended as a library default** — it produces a binary that
will not run on older CPUs, for a benefit confined to one kernel. Recommended as
a documented opt-in for deployment builds via `.cargo/config.toml`.

`[profile.release] lto = "thin", codegen-units = 1` was **not measured** and is
therefore **not recommended**; the plan listed it as optional and the wall-clock
budget went to the threshold sweeps instead, which changed real dispatch
decisions. Flagged as an open item rather than silently dropped.

### 10.3 Unattributed observation

`interning_bench/full_frontend_pipeline` measures **91.06 ms** against the
~107 ms stated in the task baseline. That path (parser → `intern_ast`) is
**untouched** by this change — nothing in the diff reaches it. The difference is
not attributable to this work and was not investigated. Recorded so it is not
later mistaken for a result of this change.

---

## 11. Next CPU opportunities

Ordered by measured expected value.

1. ~~**Remove the per-incidence allocation in `extract_ref_weight_scalar`**~~
   — **DONE**, 2026-08-24, under `APPROVED-CORE-EDIT: extract-ref-weight-noalloc`.
   See the **Addendum** at the end of this report: 1 642 260 allocations removed
   per large `from_ir` call, weighted construction **1.57–3.34×** faster.

   **Correction to the sketch originally written here.** This item proposed
   `if count == 0 { F::one() } else { sum }`, on the reasoning that "for one
   element the sum *is* that element". **That is wrong**, and implementing it as
   sketched would have introduced a silent numerical regression: the old code
   returns a single value *verbatim*, whereas `F::zero() + x` is not bitwise `x`
   when `x = -0.0` — it yields `+0.0`, flipping the sign bit. The shipped
   implementation therefore keeps a separate `first` value for the one-element
   case. The error was caught by writing the regression tests against the old
   implementation *before* touching it, which is the whole reason that ordering
   is worth the trouble.

2. **`sort_reduce_pairs` allocates a second full array.** For the large workload
   that is ~39 MB copied to deduplicate. In-place `dedup_by` with the same
   aggregation would avoid it. Non-core (`hymeko_hnn`), so unblocked.

3. **`collect_triples` weight cloning.** Each incidence's weight is cloned twice
   more when building the two CSRs. For `V = f32` that is free, but for a
   non-`Copy` `IncVal` it is three copies per incidence.

4. **Parallelise the counting-sort scatter** in `build_csr_node_to_edges`, or
   restore a parallel sort above a measured size — the counting sort gave up the
   24-core parallelism the old code had at large scale (§5.4).

5. **`clique_expansion_csr` / `populate_clique_csr`** allocate a `Vec` per
   hyperedge in the hot loop — the same defect already fixed in
   `build_explicit_a`. **Deliberately not changed**: neither function has any
   test coverage, and modifying untested code was out of scope for the five
   stated target areas. Add coverage first.

6. **`GcnCliqueLayer::from_view_with_init`** sorts a `Vec<(usize, usize, F)>` of
   clique entries and builds the CSR by hand; it could use the counting-sort
   approach and `TensorCsrBuilder::finalize_coalesced`.

7. **`f32` SIMD width.** The SpMM inner loop now vectorises, but nothing pins
   the tile width to the machine vector width. Explicit 8- or 16-wide blocking
   might beat the auto-vectoriser on `feature_dim` values that are not multiples
   of the register width.

---

## 12. Regressions, trade-offs, and waivers

**Trade-offs accepted:**

- **Convolution scratch memory increased.** The feature-blocked forward holds
  `num_edges × tile` intermediates where the old code held `num_edges`. Bounded
  at ~4 MiB of `f32` by `FEATURE_TILE_BUDGET`; on very large graphs the tile
  shrinks toward 1 and behaviour degrades to the old per-feature traversal
  rather than allocating without limit. This is a deliberate
  memory-for-traversals trade and is the one place memory went **up**.
- **Three public entry points per kernel** (`_serial`, `_parallel`, dispatching).
  Justified: it is what makes serial≡parallel testable, and it is the seam the
  backend abstraction reuses (§13). Not a Cartesian-product API — the axes are
  traversal strategy, not orthogonal feature flags.
- **`from_ir/large` +1.3 %** (§5.4).
- **`mp_clique_diag/small` +8.7 %**, `mp_gather/small` −7 % (§7).

**Waivers (CLAUDE.md §6.3 / §9):**

- `plan.pdf` **not rendered** — no TeX engine on this host (`pdflatex`,
  `lualatex`, `xelatex`, `tectonic`, `latexmk`, MiKTeX all absent). `plan.tex`
  and `plan.tikz` are authored as compilable sources. Recorded in
  `tools.yaml: docs.latex.status = absent`.
- `plan.mmd` **not rendered** — no `node` / `npx`. Source is authored and renders
  inline on GitHub.
- **`samply` substituted for `cargo flamegraph`** — user-approved 2026-08-24;
  `cargo flamegraph` has no working backend on Windows. A samply capture of
  `from_ir/large` was attempted and **timed out under Criterion**; the bottleneck
  was instead isolated with the discriminating benchmark in §8.2, which is
  stronger evidence than a sampled profile for this particular question (it
  isolates one field, not one symbol).
- **Two `#[allow(clippy::too_many_arguments)]`** added in `signed_hgnn.rs`
  (`gather_tile`, `scatter_tile`), each with an inline reason: two output
  channels plus a tile window; a config struct threaded through the hot loop
  would cost more than it documents.
- **No `unsafe` was added anywhere.**
- **16 GB RSS cap** (§4) is observed and asserted on Windows, not
  kernel-enforced — there is no `systemd-run --user -p MemoryMax=` equivalent.

**No §6.5 anti-patterns introduced.** One was *removed*: a byte-identical
duplicate of `inc_scalar_signed` (one copy in `message_passing.rs`, one in
`common_traversal.rs`) was unified behind a single `project_incidence`.

---

## 13. GPU / FPGA backend interface

Full proposal in **`docs/backend-abstraction.md`**. Summary:

- **Three layers.** Kernel **traits with static/generic dispatch** for the CPU
  tiers (`CpuSerialBackend`, `CpuRayonBackend`) — monomorphised, fully
  inlinable, **zero** cost in the hot loop. An **enum facade** (`Device`) at the
  outermost call site for runtime device selection — one predictable branch per
  *invocation*, which is free against a 105 ms SpMM. **Feature gates** around
  the device modules for toolchain isolation.
- **`Box<dyn Backend>` is rejected** — not for the virtual call, but because it
  erases the `F: Real` element type and forfeits the mixed precision an FPGA
  (fixed-point) or CUDA (fp16/bf16) backend would want.
- **The rule to enforce:** the enum never appears below the kernel entry point.
  A `match device` inside a row loop is the anti-pattern the layering exists to
  prevent — at ~1 ns/element, a non-inlinable indirection *is* the work.
- **The `*_into` signatures added here are the enabling change**: explicit
  caller-owned output, no hidden allocation, no ownership transfer — exactly
  what a device backend needs to write into pinned or device-resident memory.
  Layer 1 is a mechanical extraction from code that already exists and is tested.
- **Flagged for later:** kernel launches are async (add
  `spmm_into_async -> Completion` beside the sync form, do not change it);
  matrices need a `PreparedMatrix` residency handle or upload will dominate; and
  **the bitwise-identity guarantee will not survive on device** — segmented
  reductions reassociate. The test suite must gain a *tolerance* tier for device
  backends **beside** the existing bitwise tier, never by weakening it.
- **Do not build it yet.** Introducing the traits before a second real
  implementation exists is speculative indirection.

---

## 14. File inventory and provenance

### 14.1 Modified

| File | Δ lines | Core? |
|---|---:|---|
| `hymeko_core/src/tensor/representations/tensor_csr.rs` | +326 | **yes** — under `csr-spmv-spmm-into` |
| `hymeko_core/Cargo.toml` | +7 | **yes** — bench target, same token |
| `hymeko_hnn/src/tensor/message_passing.rs` | +550 | no |
| `hymeko_hnn/src/tensor/conv/signed_hgnn.rs` | +344 | no |
| `hymeko_hnn/src/tensor/conv/hgnn.rs` | +290 | no |
| `hymeko_hnn/src/traversal/hypergraphview.rs` | +132 | no |
| `hymeko_hnn/src/tensor/common_traversal.rs` | +107 | no |
| `hymeko_hnn/src/tensor/conv/gcn_clique.rs` | +98 | no |
| `hymeko_hnn/src/tensor/common.rs` | +79 | no |
| `hymeko_hnn/Cargo.toml` | +8 | no |
| `hymeko_hnn/src/lib.rs` | +5 | no |
| `hymeko_core/tests/test_tensor_representations/mod.rs` | +4 | no (allowlisted) |

**Formatting-only** (touched by `cargo fmt --all`, no semantic change —
declared per §3): `hymeko_hnn/src/tensor/mesh_nn/mod.rs`,
`hymeko_hnn/src/tensor/mod.rs`, `hymeko_hnn/src/tensor/tensor.rs`,
`hymeko_hnn/src/tensor/representations/tensor_csr_representations.rs`,
`hymeko_hnn/src/traversal/{decltreeview,graph_traversal,graphview,mod}.rs`.
Verified as import reordering, argument wrapping and module-declaration
ordering only (`git diff --ignore-all-space`).

### 14.2 Created

| File | Lines | Purpose |
|---|---:|---|
| `CORE.YAML` | 202 | Workspace lockdown manifest (was absent — §1/§11 halt) |
| `tools.yaml` | 177 | Toolchain declaration, incl. the samply substitution |
| `hymeko_hnn/src/workloads.rs` | 612 | Deterministic shared workloads + `from_design` adapter |
| `hymeko_hnn/benches/hnn_bench.rs` | 491 | Message-passing, conv, construction benchmarks |
| `hymeko_core/benches/csr_bench.rs` | 243 | SpMV / SpMM benchmarks + threshold sweeps |
| `hymeko_core/tests/.../test_conv_forward.rs` | 660 | HGNN / Signed-HGNN / GCN regression oracles |
| `hymeko_core/tests/.../test_csr_kernels.rs` | 479 | CSR correctness and contracts |
| `hymeko_core/tests/.../test_message_passing_parallel.rs` | 477 | Edge-major reference oracles |
| `hymeko_core/tests/.../test_view_construction.rs` | 270 | Counting-sort ordering guarantees |
| `docs/backend-abstraction.md` | 231 | GPU/FPGA interface proposal |
| `docs/plans/2026-08-24-cpu-optimization/plan.{tex,tikz,mmd}` | 508 | Plan bundle (PDF waived, §12) |
| `scripts/dev/criterion_report.ps1` | 167 | Criterion JSON → comparable tables |
| `scripts/dev/measure_peak_memory.ps1` | 151 | Windows peak-WS harness |
| `scripts/dev/bench_thread_sweep.ps1` | 84 | Per-process `RAYON_NUM_THREADS` sweep |

### 14.3 Dependencies

- **Added:** `criterion 0.8.2` as a **dev-dependency of `hymeko_hnn`** only
  (already a dev-dependency of `hymeko_core`). No runtime dependency added,
  removed, or version-changed in either crate.
- `nalgebra` re-pinned in the new `CORE.YAML` to `0.34.1`, the version this
  workspace has always built against. This **records** existing reality rather
  than changing it — the stale monorepo pin (`=0.33.0`) was what tripped
  `on_dependency_drift`.

### 14.4 Experiment provenance

- **Git SHA:** `45b0512` (`main`). Working tree dirty by design — this change.
  No commit was made; the user did not request one.
- **Host:** Intel Core Ultra 9 275HX, 24 logical cores; 63.4 GiB RAM;
  NVIDIA RTX 5080 Laptop 16 GiB (unused — no GPU work this round).
  OS Windows 11 Home 10.0.26200.
- **Toolchain:** cargo 1.97.1, rustc 1.97.1 (8bab26f4f 2026-07-14),
  clippy 0.1.97, rustfmt 1.9.0-stable, criterion 0.8.2, samply 0.13.1.
- **Seeds:** all workloads deterministic from fixed LCG seeds declared in
  `hymeko_hnn/src/workloads.rs` (`SparseSpec` `0x5EED_000{1,2,3}` / `0x5EED_7777`;
  `HyperSpec` `0x11AB_000{1,2,3}` / `0x11AB_7777`). No system entropy anywhere.
- **Criterion baselines on disk:** `target/criterion/**/{before,after,t1,t4,t8,t16,t24,native}/`.
  Regenerate any table with `scripts/dev/criterion_report.ps1 -Baseline before -Compare after`.
- **Measurement caveat:** see §7 — sustained benchmarking on this laptop drifts;
  small-scale cells carry a ~20 % run-to-run band, and `before` was measured on a
  cooler machine than `after`, which biases the reported speedups downward.

---

# Addendum — `extract-ref-weight-noalloc` (2026-08-24, follow-up)

**Approval token:** `APPROVED-CORE-EDIT: extract-ref-weight-noalloc`

This closes §11 item 1, the largest remaining CPU win identified by the main
round, which was blocked at the time for lack of a core-edit approval.

## A.1 Algorithmic change

`hymeko_core/src/tensor/tensor_val.rs :: extract_ref_weight_scalar`.

The old implementation collected every numeric weight into a temporary
`Vec<F>`, then asked it three questions: *is it empty*, *does it hold exactly
one element*, and *what is its left-to-right sum*. `Vec::new()` allocates on
first push, so on the `from_ir` path that was **one heap allocation per
incidence**.

All three questions are answerable from running state, so the `Vec` is not
needed:

| old | new |
|---|---|
| `nums: Vec<F>`, one push per value | `count: usize`, `first: F`, `sum: F` |
| `nums.is_empty()` | `count == 0` |
| `nums.len() == 1` → `nums[0]` | `count == 1` → `first` |
| `nums.into_iter().fold(zero, +)` | `sum`, accumulated in the same pass |

Pure strength reduction: same values, same order, no allocation. Complexity is
unchanged at `O(n)` in the number of weight entries; only the constant and the
allocation count move.

### Semantics preserved — the three sharp edges

The contract has three details a naive rewrite loses. Each is pinned by a test
written **against the old implementation first**, and each of those tests
passed unchanged before and after:

1. **`None`, empty, and "no numeric value found" all yield `one()`**, not
   `zero()`. Includes `Some(vec![])` and lists holding only `Str` / `Ref`.
2. **Exactly one value is returned verbatim, not folded.** `zero() + x` is not
   bitwise `x` when `x = -0.0` — it gives `+0.0`. Collapsing the single-value
   case into the general fold would silently flip the sign bit. The new code
   keeps `first` for exactly this reason, and
   `test_single_negative_zero_is_returned_verbatim` fails if it is removed.
   The asymmetry is deliberate and now documented: two `-0.0` values still
   collapse to `+0.0`, because the multi-value path genuinely does start at zero.
3. **`List` is descended exactly one level.** A `List` nested inside a `List`
   contributes nothing, so `[[5.0]]` yields `1.0` (nothing found), not `5.0`.

`f32` and `f64` are both covered: `F::from_other` narrows per element, so the
`f32` sum narrows each term *before* adding rather than summing in `f64` and
narrowing once. `test_f32_and_f64_conversion_paths` pins that.

## A.2 Allocation count (measured)

Harness: `hymeko_hnn/examples/alloc_count.rs` — a counting `GlobalAlloc` that
tallies allocations and bytes around a single `from_ir` call.
Run: `cargo run --release --example alloc_count -p hymeko_hnn`.

| scale | weights | incidences | allocs before | allocs after | allocs/inc before → after |
|---|---|---:|---:|---:|---|
| small | `Some` | 1 995 | 2 010 | **15** | 1.008 → **0.008** |
| small | `None` | 1 995 | 15 | 15 | 0.008 → 0.008 |
| large | `Some` | 1 642 260 | 1 642 278 | **18** | 1.000 → **0.000** |
| large | `None` | 1 642 260 | 18 | 18 | 0.000 → 0.000 |

**1 642 260 allocations removed** from one large `from_ir` call — exactly one
per incidence, which is precisely what the hypothesis predicted. Bytes requested
fell from 263 989 184 to 237 713 024 (**−26.28 MB**, −10.0 %).

The decisive observation: after the change the `Some` and `None` rows are
**identical in both allocation count and bytes**. That is the definition of
"no per-incidence allocation" — the weighted path no longer allocates anything
the unweighted path does not.

## A.3 Timing (measured)

Criterion, medians, `before` and `after` measured back-to-back in one session to
limit the thermal drift documented in §7.

| cell | before | after | speedup |
|---|---:|---:|---:|
| `from_ir weights_some / small` | 63.859 µs | **19.095 µs** | **3.34×** |
| `from_ir weights_some / medium` | 18.032 ms | **10.720 ms** | **1.68×** |
| `from_ir weights_some / large` | 170.06 ms | **108.22 ms** | **1.57×** |
| `from_ir weights_none / small` | 17.408 µs | 17.232 µs | 1.01× (unchanged) |
| `from_ir weights_none / medium` | 8.991 ms | 8.251 ms | 1.09× |
| `from_ir weights_none / large` | 106.00 ms | 98.038 ms | 1.08× |

Throughput on the weighted large workload: **9.66 → 15.18 Melem/s**.

### The gap between weighted and unweighted, closed

This is the clearest way to read the result — the `Some`/`None` gap *was* the
allocation, and it is now mostly gone:

| scale | gap before | gap after |
|---|---:|---:|
| small | 3.67× | **1.11×** |
| medium | 2.01× | 1.30× |
| large | 1.60× | 1.10× |

The residual gap is the real arithmetic (an `f64`→`F` conversion and an add per
incidence), which is work the function is supposed to do.

### One flagged regression, investigated and dismissed

The comparison initially flagged `weights_none/small` at **+10.6 %** — on a code
path this change cannot have touched, since the `None` early-return is
byte-identical. Rather than assume noise, it was re-run three times:
**17.043 / 17.156 / 16.823 µs**, against a pre-value of 17.408 µs. The 19.25 µs
reading was a transient outlier; the settled value is 17.232 µs, i.e. unchanged.
Recorded here because "it must be noise" is a hypothesis until it is repeated.

## A.4 Gates

- `cargo fmt --all -- --check` — **pass**
- `cargo clippy --workspace --all-targets -- -D warnings` — **pass**
- `cargo test --workspace --release` — **17/17 unit; 146 integration, 145 pass**
- The known drift is unchanged and untouched: `test_load_meta_geometry`,
  `left: 58, right: 57`.

Integration test count 138 → **146** (+8), all in
`test_weight_extraction.rs`.

## A.5 Files touched

| File | Change |
|---|---|
| `hymeko_core/src/tensor/tensor_val.rs` | **core, under token** — the rewrite |
| `hymeko_core/tests/test_tensor_representations/test_weight_extraction.rs` | new, 8 regression tests (written pre-change) |
| `hymeko_core/tests/test_tensor_representations/mod.rs` | +1 module line |
| `hymeko_hnn/examples/alloc_count.rs` | new, allocation-counting harness |

No benchmark changes were needed — the `view_from_ir_weight_path` group added in
the main round is exactly the discriminating measurement this work required, and
it was reused as-is. No dependencies added or changed.

### `unsafe` waiver

The main round added **no** `unsafe`. This addendum adds one `unsafe impl
GlobalAlloc` in `hymeko_hnn/examples/alloc_count.rs` — an example binary, never
linked into either library. There is no safe way to install a global allocator,
and it is the mechanism the allocation-count requirement calls for. Every method
forwards to `System` with the same pointer and layout it received; the safety
invariants are documented inline at the impl. The alternative, `dhat`, would
have meant a new dependency on a `lockdown: full` crate — a heavier change than
a self-contained example.
