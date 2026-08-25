# WGPU Phase 3 — resident activation chain

**Date:** 2026-08-25
**Scope:** `APPROVED-WGPU-PHASE-3: resident-activation-chain`
**Crates touched:** `hymeko_wgpu` only. **No CPU file was modified** — verified by
modification time, not asserted (§14).

---

## 1. Summary

Phase 2 proved one GPU operation works. Phase 3 answers the question that decides
whether a GPU backend is worth having at all: **does a second operation run on the
first one's output without a host round trip?**

It does:

```rust
let mut enc = backend.encoder("chain");
spmm.encode(&mut enc);      // Y = A · X
act.encode(&mut enc);       // Y = f(Y)   in place, one read_write binding
backend.submit(enc);        // ONE submit
let out = backend.read_tensor(&y)?;   // readback only when asked for
```

The measurement that justifies the phase: at 200 000 rows × 128 features, the
identical work costs **45.8 ms with a readback and 6.34 ms resident**. The
readback is **86 % of wall time**. A design that read back after every operation
would have been a slower CPU with extra steps.

Delivered beyond the chain itself:

* a second operation (`ActivationBuilder` / `PreparedActivation`) with five kinds,
  a legal in-place path, and a documented numerical contract;
* a **measured** feature-dimension-aware SpMM workgroup rule, wired in as the
  default, as a pure function with 9 GPU-free unit tests;
* the optional fused kernel (cell H), which wins 1–21 %, with the gain tracking
  the bandwidth model at the large end;
* an SpMM CPU/GPU crossover band, localised to ~12 000 rows at fd8 and ~5 000 at
  fd32;
* a ComputeGraph decision document that **recommends against** building one, with
  three falsifiable conditions that would reverse that.

One substantive mistake was made and corrected inside the phase: the workgroup
rule was first fitted to a single problem size and was 16.5 % slower at
large/fd128. §8 records it, because the recovery is the transferable part.

---

## 2. Cells A–C — activation alone

`A` = CPU single-threaded; `B` = GPU with its own encoder, submit and wait;
`C` = GPU encoded into a shared encoder, **not** submitted.

| workload | fd | A: CPU | B: GPU submit | C: encode only |
|---|---:|---:|---:|---:|
| small | 1 | 32.80 ns | 79.77 µs | 617.4 ns |
| small | 8 | 618.5 ns | 83.37 µs | 642.5 ns |
| small | 32 | 2.678 µs | 88.30 µs | 625.3 ns |
| small | 128 | 7.664 µs | 86.68 µs | 645.1 ns |
| medium | 1 | 2.975 µs | 86.22 µs | 631.8 ns |
| medium | 8 | 32.01 µs | 89.23 µs | 621.7 ns |
| medium | 32 | 215.6 µs | 93.34 µs | 610.5 ns |
| medium | 128 | 2.090 ms | 231.1 µs | 631.4 ns |
| large | 1 | 12.14 µs | 82.79 µs | 625.6 ns |
| large | 8 | 215.9 µs | 91.71 µs | 623.6 ns |
| large | 32 | 2.075 ms | 232.1 µs | 608.2 ns |
| large | 128 | 9.681 ms | 401.1 µs | 623.8 ns |

Three readings, in decreasing order of confidence:

1. **Encoding an operation costs ~0.62 µs, flat.** Cell C is 608–645 ns across a
   range where the work varies by five orders of magnitude, because it measures
   only the CPU-side recording of a dispatch. The marginal cost of adding an
   operation to a chain is therefore essentially constant — which is what makes
   long resident chains viable.
2. **Submit-and-wait has an ~80 µs floor.** Cell B never drops below 79.8 µs
   however small the work. Any GPU operation that must be individually submitted
   and waited on pays this; it is the reason cells D–H matter more than B.
3. **CPU wins until roughly 10⁶ elements.** At medium/fd8 (1.6M elem) the CPU is
   2.8× faster; at medium/fd32 the GPU is 2.3× faster; at large/fd128 the GPU is
   **24×** faster. Elementwise activation is bandwidth-bound, so this crossover is
   about amortising the submit floor, not about arithmetic.

---

## 3. Cells D–H — the chain

`D` = SpMM + readback; `E` = SpMM submit, activation submit, readback;
`F` = both in one submit, readback; `G` = both in one submit, **resident**;
`H` = fused single dispatch, resident.

All figures from the **final** run, with the corrected workgroup rule (§8)
active. Earlier runs are superseded, not averaged in: they measured a different
code path.

| workload | fd | D | E | F | G | H |
|---|---:|---:|---:|---:|---:|---:|
| small | 1 | 187.0 µs | 272.4 µs | 194.0 µs | 103.6 µs | 89.29 µs |
| small | 8 | 189.4 µs | 277.1 µs | 199.3 µs | 101.9 µs | 93.37 µs |
| small | 32 | 200.7 µs | 289.9 µs | 209.9 µs | 103.3 µs | 91.30 µs |
| small | 128 | 246.4 µs | 353.8 µs | 258.0 µs | 111.3 µs | 94.37 µs |
| medium | 1 | 232.9 µs | 334.5 µs | 249.8 µs | 128.0 µs | 115.1 µs |
| medium | 8 | 710.9 µs | 777.6 µs | 689.1 µs | 131.1 µs | 114.4 µs |
| medium | 32 | 2.381 ms | 2.025 ms | 1.844 ms | 192.4 µs | 174.4 µs |
| medium | 128 | 8.471 ms | 9.813 ms | 9.179 ms | 671.9 µs | 529.8 µs |
| large | 1 | 648.2 µs | 754.8 µs | 665.7 µs | 433.0 µs | 428.5 µs |
| large | 8 | 2.379 ms | 2.571 ms | 2.303 ms | 560.7 µs | 525.2 µs |
| large | 32 | 8.438 ms | 7.458 ms | 7.994 ms | 1.284 ms | 1.147 ms |
| large | 128 | 46.50 ms | 47.31 ms | 45.80 ms | 6.336 ms | 5.973 ms |

### 3.1 Residency is the whole result

| workload | F (readback) | G (resident) | readback share |
|---|---:|---:|---:|
| small/fd1 | 194.0 µs | 103.6 µs | 47 % |
| medium/fd32 | 1.844 ms | 192.4 µs | 90 % |
| medium/fd128 | 9.179 ms | 671.9 µs | 93 % |
| large/fd32 | 7.994 ms | 1.284 ms | 84 % |
| large/fd128 | 45.80 ms | 6.336 ms | **86 %** |

At production scale the host transfer is 5–7× the compute. This is the number
that decides the architecture: an operation model requiring a readback per step
would spend ~87 % of its time moving data that the next operation was about to
consume anyway.

### 3.2 What E vs F does *not* show

One submit versus two is **not** a consistent win in this data. F beats E at
small, medium/fd8 and large/fd32-128; E beats F at medium/fd32 and medium/fd128.
With readback consuming >80 % of both, the submit difference is inside the
residue.

**No claim is made that batching submits is a measured win.** The measured win is
residency (§3.1). One encoder matters because it is what makes residency
*expressible* — not because of a timing result.

### 3.3 Cell H — fusion

| workload | G: two dispatches | H: fused | gain | activation's share of G (ceiling) |
|---|---:|---:|---:|---:|
| small/fd1 | 103.59 µs | 89.29 µs | 13.8 % | ~2 % |
| small/fd8 | 101.90 µs | 93.37 µs | 8.4 % | ~2 % |
| small/fd32 | 103.31 µs | 91.30 µs | 11.6 % | ~3 % |
| small/fd128 | 111.31 µs | 94.37 µs | 15.2 % | ~2 % |
| medium/fd1 | 128.04 µs | 115.06 µs | 10.1 % | ~1 % |
| medium/fd8 | 131.14 µs | 114.37 µs | 12.8 % | ~3 % |
| medium/fd32 | 192.40 µs | 174.36 µs | 9.4 % | ~4 % |
| medium/fd128 | 671.93 µs | 529.78 µs | **21.2 %** | ~22 % |
| large/fd1 | 432.97 µs | 428.50 µs | 1.0 % | ~0 % |
| large/fd8 | 560.68 µs | 525.20 µs | 6.3 % | ~1 % |
| large/fd32 | 1.2840 ms | 1.1472 ms | 10.7 % | ~11 % |
| large/fd128 | 6.3363 ms | 5.9728 ms | 5.7 % | ~4.5 % |

"Ceiling" is cell B minus the ~85 µs submit floor: what fusion could save if it
removed *only* the activation's own work.

A repeat run during the memory measurement gave large/fd128 G = 6.25 ms,
H = 5.94 ms (5.0 % gain) — consistent with the 5.7 % above.

Two regimes, and the second is **inferred, not isolated**:

* **Gain ≈ ceiling** at the large end — medium/fd128 (22 % vs 21.2 %),
  large/fd32 (11 % vs 10.7 %), large/fd128 (4.5 % vs 5.7 %). Fusion is recovering
  the activation pass, exactly as the bandwidth model predicts. Well understood.
* **Gain > ceiling** at the small end — every small cell gains 8–15 % against a
  ~2 % ceiling. Something besides the activation's own work is being saved. The
  plausible candidate is fixed cost between two *dependent* dispatches (pipeline
  switch plus the cache flush the dependency forces), ~10 µs at this scale.
  **This is a hypothesis. The discriminating test — a two-dispatch chain whose
  second operation is independent of the first — was not run.**

Note that the earlier apparent *tie* at large/fd128 (−0.1 %) was an artifact of
the incorrect workgroup rule, not a property of fusion. With the corrected rule
the cell behaves like the rest of the large end.

**Fusion stays opt-in.** `fused_activation` is never applied automatically:
whether it wins is a property of the workload, and a rule that guessed would
repeat the §8 mistake in a new place.

### 3.4 SpMM crossover — CPU vs GPU, both resident

Eight logarithmically spaced row counts between the Phase 2 small and medium
sizes, SpMM only, GPU measured resident (no readback):

| rows | fd8 serial | fd8 rayon | fd8 GPU | fd32 serial | fd32 rayon | fd32 GPU |
|---:|---:|---:|---:|---:|---:|---:|
| 1 500 | 44.50 | **59.90** | 88.39 | 88.84 | **68.02** | 86.71 |
| 2 500 | 73.27 | **60.24** | 89.41 | 163.8 | **66.43** | 90.68 |
| 4 000 | 110.0 | **50.30** | 92.00 | 249.8 | **86.95** | 100.8 |
| 6 500 | 205.1 | **77.06** | 97.04 | 429.8 | 107.6 | **101.8** |
| 10 000 | 298.0 | **95.39** | 98.73 | 702.0 | 157.3 | **107.1** |
| 16 000 | 487.5 | 123.9 | **98.82** | 1137 | 189.5 | **103.2** |
| 25 000 | 717.4 | 159.3 | **93.52** | 1658 | 268.6 | **125.5** |
| 40 000 | 1167 | 213.6 | **110.4** | 3351 | 589.4 | **150.9** |

(µs; **bold** = fastest in that row.)

* **fd8 crossover ≈ 12 000 rows** (between 10 000 and 16 000).
* **fd32 crossover ≈ 5 000 rows** (between 4 000 and 6 500).

Wider features cross over earlier, which is what the submit-floor model predicts:
the GPU's ~85–90 µs floor is nearly flat in the row count, so the crossover is
set by where CPU work exceeds that floor — and fd32 reaches it with fewer rows.

Two cautions on this table:

1. `cpu_rayon` is **not monotone** (r4000/fd8 at 50.3 µs is faster than r2500 at
   60.2 µs). Rayon's fork/join floor plus ambient load dominates at these sizes.
   The crossover should be read as a **band**, not a threshold.
2. These are resident GPU timings. With a readback the crossover moves far to the
   right — §3.1 shows readback costing 5–7× the compute. **The GPU is worth
   reaching for when the result stays on the device, which is the point of the
   whole phase.**

## 4. Correctness

| suite | tests | result |
|---|---:|---|
| `hymeko_wgpu` lib (workgroup rule + block_on) | 11 | pass |
| `tests/activation.rs` | 19 | pass |
| `tests/spmm.rs` (Phase 2 regression) | 13 | pass |
| `tests/skeleton.rs` | 8 | pass |
| doc-tests | 1 | pass |
| **`hymeko_wgpu` total** | **52** | **pass** |
| workspace (`--release --no-fail-fast`) | 222 | 221 pass, 1 known drift |

The single failure is `domain_transformations::parse_files::test_parse_files::test_load_meta_geometry`
— the pre-existing 57/58 fixture drift. **Untouched, not silenced, not rewritten.**

### 4.1 Tests that exist because something actually went wrong

* **`test_count_exceeding_one_dispatch_grid`** — all 15 activation tests passed
  *before* the grid-stride fix, because none exceeded 65 535 workgroups. The gap
  was exactly where the bug lived. The test now runs 4 194 240 + 4097 elements
  (just past the wg=64 single-grid ceiling, coprime with all three workgroup
  sizes) and asserts the strided result is **bit-identical** to a dispatch that
  fits in one grid.
* **`test_fused_matches_unfused_chain`** — compares fused against unfused
  **bit-exactly**, across 3 feature dims × 4 kinds. Both run the same
  accumulation in the same order on the same hardware, so the only way they can
  differ is if the duplicated loop in `spmm_activation.wgsl` drifted from
  `spmm.wgsl`. A tolerance here would hide precisely what the test is for.
* **`test_subnormal_handling_is_characterised_not_assumed`** — see §5.
* **`test_in_place_and_out_of_place_agree`** — the in-place path is a second WGSL
  file; this is what stops the two from diverging silently.

---

## 5. Numerics

### 5.1 Subnormals — measured, then admitted narrowly

`LeakyRelu { alpha: 0.01 }` on `-f32::MIN_POSITIVE` gives `-1.1755e-40` on the CPU
and exactly `-0.0` on the GPU. Rather than widen a tolerance until the test
passed, the behaviour was **characterised**:

* **storage and copy preserve subnormals exactly** — `Identity` round-trips
  `0x0000_0001` (1.4e-45) bit-for-bit;
* **arithmetic flushes a subnormal *result* to zero, sign preserved.**

That is flush-to-zero on arithmetic, not denormals-are-zero on input; WGSL
guarantees neither. It is admitted as a narrow signed exception
(`subnormal_flushed_to_zero`), with every other value on the same test still
compared exactly. A widened tolerance would have erased the distinction between
"storage preserves subnormals" and "arithmetic does not" — which is the part a
caller needs.

### 5.2 Per-kind contract

`Relu`/`LeakyRelu` use `select`, not `max`: WGSL leaves `max` with a NaN operand
implementation-defined, while any comparison with NaN is well-defined as false.
The result matches what the CPU layers already do
(`if val > zero { val } else { zero }`), so a future CPU/GPU A/B of a full forward
pass compares like with like. Full table in `docs/wgpu-backend.md` §5b.7.

Only `Sigmoid`/`Tanh` are compared with a tolerance, flagged by
`ActivationKind::is_transcendental()`.

---

## 6. The dispatch-limit defect, and the fix

The activation dispatch is 1-D, one element per invocation, so a 256-wide
workgroup addresses at most 65 535 × 256 = 16.7M elements. Large × fd128 is
**25.6M elements — inside the intended range, past the hardware ceiling.**

`prepare()` rejected it as `IndexOverflow` on the first benchmark run. That is the
validation working: the alternative would have been silently truncated output.

Fixed with a grid-stride loop — the "one or, where justified, several elements per
invocation" case the brief permits:

```wgsl
for (var i = gid.x; i < params.count; i = i + params.stride) { ... }
```

```rust
let ideal_groups = count.div_ceil(workgroup);
let groups = ideal_groups.min(limits.max_workgroups_per_dimension);
let stride = groups * workgroup;
```

* **Strides by the whole grid**, not contiguous blocks per invocation, so adjacent
  lanes stay on adjacent elements and coalescing survives.
* **Costs nothing when unneeded**: if the grid covers `count`, `stride >= count`
  and the body runs once.
* `stride` occupies a former padding field, so the uniform block is still 16 bytes
  with no `usize`.

Generalisable lesson: *a limit that a legitimate workload crosses should be
accommodated, not merely reported.*

---

## 7. In-place — investigated, and legal

Binding one buffer as both the read and the `read_write` binding of the
out-of-place shader violates WebGPU's usage-scope rule. That is a validation
error, not a portability grey area, and the brief forbade working around it with
unsafe or backend-specific tricks.

The resolution removes the aliasing instead of evading it: a separate module with
a **single `read_write` binding**, reading and writing the same element. Nothing
aliases, so there is nothing to violate.

Cost: the activation expressions appear in two WGSL files (WGSL has no include
mechanism, and runtime shader-string assembly was out of scope). Bounded to one
line per kind, and covered by `test_in_place_and_out_of_place_agree`.


---

## 8. The workgroup rule — and the error it corrected

### 8.1 What shipped

`spmm_workgroup_for(feature_dim, max_invocations_per_workgroup)`, a `const fn`
applied by `SpmmBuilder::prepare()` when the caller has not named a shape:

```rust
if feature_dim < 16 { 8x8 } else { 16x8 }     // then step down to fit the device
```

| brief requirement | how it is satisfied |
|---|---|
| measurement-based | §8.2; nothing chosen by intuition |
| separately testable pure function | 9 unit tests, **no GPU required** |
| explicit override | `SpmmBuilder::workgroup()`, not checked against the rule |
| no global mutable state | `const fn` of two arguments — no cache, cell, or env var |
| in the pipeline-cache key | the shape flows into `PipelineKey::new_2d` |
| device-limit fallback | 16×8 → 8×8 → 4×4 → 1×1 |
| winner not hardcoded before measurement | the first version was measured, found wrong, and replaced |

### 8.2 The measurement (µs; **bold** = row winner)

| `feature_dim` | med 8×8 | med 16×8 | med 16×16 | lg 8×8 | lg 16×8 | lg 16×16 |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | **100.4** | 113.2 | 125.2 | **429.8** | 511.6 | 510.8 |
| 4 | **114.5** | 124.3 | 127.3 | **435.7** | 521.2 | 579.5 |
| 8 | **119.2** | 130.1 | 131.6 | **513.7** | 611.7 | 655.1 |
| 16 | 145.5 | 134.7 | **123.0** | 927.1 | **675.7** | 718.3 |
| 32 | 189.8 | **176.2** | 179.0 | 1721.7 | **1197.3** | 1290.4 |
| 64 | 337.8 | 275.9 | **272.4** | 3595.2 | **2456.9** | 2649.1 |
| 128 | 651.0 | 540.2 | **531.5** | 8828.7 | **6035.0** | 7029.0 |

Mechanism: the kernel maps `gid.x` to the feature axis, so a 16-wide shape at
`feature_dim = 1` idles 15 of every 16 lanes.

### 8.3 The error

The rule was **first fitted to the medium spec alone** and read
`fd < 16 → 8×8; fd < 32 → 16×8; else 16×16`. Measured against the large spec that
last arm was **16.5 % slower at fd128** (7029 vs 6035 µs).

It also produced a visible chain regression — cell G at large/fd128 moved
6.36 ms → 7.00 ms — which was **initially and wrongly set aside as ambient
noise**, on the strength of this host's two documented phantom regressions.

Two separable faults:

1. **Generalising a hardware tuning parameter from one problem size.** The row
   count changes which shape wins at `fd >= 16`; one spec cannot show that.
2. **Reaching for "it's noise" before running the discriminating test.** The
   prior was *available* — this host really does produce phantom regressions —
   which is exactly what made it dangerous. A convenient explanation stood in for
   a measurement.

The correct move cost six minutes: extend the sweep to a second problem size so
all three shapes are compared **within one run**. Cross-run comparisons here
carry ~3 % confidence intervals and cannot settle a 1–10 % question; within-run
A/B can.

Confirmation the fix worked, not merely that the code changed: cell G at
large/fd128 returned to **6.336 ms** (6.360 before the rule, 7.003 with the bad
rule, 6.336 with the corrected one), and an independent repeat during the memory
measurement gave 6.25 ms.

### 8.4 Why two arms and not three

`16×16` leads a few medium cells, so a third arm is tempting. It is not
supported:

* on **large**, 16×8 beats 16×16 by 6–16 % at every `fd >= 16`;
* on **medium**, 16×16's lead is at most **1.6 %**, and it is not stable — across
  the two sweep runs the winner at fd 16 and fd 32 flipped, with margins inside
  the confidence intervals.

Trading ≤1.6 % on medium for 16.5 % on large is the whole argument.
`never_selects_16x16_automatically` locks it in so a later edit cannot quietly
reintroduce the losing shape. 16×16 stays reachable via the override and is still
swept — a shape that loses on one adapter is not a shape to delete.

---

## 9. Memory

Measured with `scripts/dev/measure_peak_memory.ps1`, which reads
`PeakWorkingSet64` from a `System.Diagnostics.Process` handle sampled while alive
(`Start-Process -PassThru` reports 0 for these counters).

| target | filter | wall | peak working set | peak paged | peak virtual | ≤16 GB |
|---|---|---:|---:|---:|---:|:-:|
| `chain_bench` | `chain/.*large` | 248.2 s | **537.8 MiB** | 1495.8 MiB | 7238.7 MiB | yes |

Peak RSS is 3.3 % of the 16 GB cap. The 7.2 GiB *virtual* figure is the sparse
address space a GPU driver reserves at init — it is not resident, and it is
exactly why CLAUDE.md §4 forbids `ulimit -v` as a memory gate for these
workloads.

No GPU memory budget was exceeded: the largest resident set is the large spec at
fd128 — X, Y and one intermediate at ~102 MiB each, plus the CSR.

---

## 10. Gates

| gate | result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo build -p hymeko_wgpu --release --offline` | clean |
| `cargo test --workspace --release --no-fail-fast` | 221 pass, 1 known drift |
| `cargo test -p hymeko_wgpu --release` | 52 pass |
| benchmark smoke (`--test`, all 165 cells) | pass |

**New suppressions:** none. **New `unwrap`/`expect` in non-test code:** none.
**New dependencies:** none — the fused kernel reuses the existing bind group,
params struct and pipeline cache.

### 10.1 Complexity

No function added in this phase exceeds the §6.2 warn thresholds.
`ActivationBuilder::prepare` and `SpmmBuilder::prepare` are the longest
(≈90 and ≈120 lines) and are straight-line validation sequences: cyclomatic
complexity is dominated by independent `if … return Err` guards, nesting depth
≤ 2. Splitting them would scatter the validation contract that §5b.9 documents as
a single table.

### 10.2 §6.5 anti-patterns

None introduced. Specifically:

* **#1 / #5 (Cartesian API surface):** fusion was added as a **config field**
  (`fused_activation`) on the existing builder, not as a `spmm_relu()` /
  `spmm_leaky_relu()` family. Same for the workgroup override.
* **#7 (string-typed config):** `ActivationKind` is an enum with data
  (`LeakyRelu { alpha }`), never a `&str`.
* **#11 (global mutable state):** the workgroup rule is a `const fn`; no cache,
  no `once_cell`, no env var read at a call site.
* **#13 (`_v2` proliferation):** no versioned files; `spmm.rs` was edited in
  place.
* **#18 (optimising before profiling):** the fusion kernel was written only after
  the standalone activation cells quantified what there was to save — and its
  measured gain is checked against that ceiling in §3.3.
* **#19 (measured optimisation left opt-in):** the workgroup rule is the
  **default**, not a flag. Fusion is deliberately *not* defaulted, because it is
  a workload-dependent win, not a pure speed win — the same rule's other half.

**Duplication accepted, with justification:** the activation expressions appear
in three WGSL files (out-of-place, in-place, fused). WGSL has no include
mechanism and runtime shader-string assembly was out of scope for this phase.
Each instance is one line per kind, and all three are pinned to each other by
`test_in_place_and_out_of_place_agree` and `test_fused_matches_unfused_chain`, so
drift fails a test.

---

## 11. Files touched

All under `hymeko_wgpu/`. **No CPU file modified**, verified by modification
time: every file changed after the phase started is in this crate.

| file | status | lines |
|---|---|---:|
| `src/operations/activation.rs` | new | 542 |
| `src/operations/spmm.rs` | modified (workgroup rule, fusion, params) | 822 |
| `src/operations/mod.rs` | modified (re-exports) | 71 |
| `shaders/activation.wgsl` | new | 105 |
| `shaders/activation_inplace.wgsl` | new | 76 |
| `shaders/spmm_activation.wgsl` | new | 104 |
| `tests/activation.rs` | new | 1177 |
| `benches/chain_bench.rs` | new | 470 |
| `docs/wgpu-backend.md` | §5b, §5c added | — |
| `docs/compute-framework.md` | §14 added | — |
| `docs/compute-graph-decision.md` | new | — |

**CORE.YAML items touched: none.** `hymeko_wgpu` carries `lockdown: none`; no
approved-edit token was needed or used this phase.

---

## 12. Provenance

* Git SHA: `45b0512caa647b324cd6ebde0c44b01010cd0fe9`; working tree **dirty**
  (42 entries — the whole uncommitted multi-phase body of work; nothing has been
  committed, per the standing instruction not to commit automatically).
* Toolchain: `rustc 1.97.1 (8bab26f4f 2026-07-14)`, `cargo 1.97.1`.
* Host: Windows 11 Home 10.0.26200.
* GPU: **NVIDIA GeForce RTX 5080 Laptop GPU**, Vulkan, DiscreteGpu, driver
  NVIDIA 610.88.
* Criterion baselines saved: `p3` (full matrix), `p3h` (chain + H), `wgsweep`
  (two-size workgroup sweep), `final` (chain + crossover, corrected rule).
* Raw logs: `<scratchpad>/chain_p3.log`, `chain_p3h.log`, `wgsweep.log`,
  `final.log`.
* Workloads are deterministic: `hymeko_hnn::workloads` builds every spec from a
  seeded LCG, so no fixture hash is needed to reproduce them.

### 12.1 Measurement conditions — stated, not hidden

Ambient CPU load was **6–32 % (mean 19.6 %)** before the first run and 26 % after.
This is not a quiet machine by CLAUDE.md §3's standard.

Consequences, applied consistently:

* GPU-resident cells (B, C, G, H, crossover GPU) are least affected — they are
  bounded by device execution and a submit floor.
* CPU cells (A, crossover serial/rayon) carry the most uncertainty; `cpu_rayon`
  is visibly non-monotone in §3.4 and is reported as a band.
* **All comparisons that decide something are within-run.** The G/H comparison,
  the workgroup sweep, and the crossover triples were each measured inside a
  single run for exactly this reason.
* Confidence intervals are ~3 % on this host; no conclusion in this report rests
  on a difference smaller than that.

---

## 13. Open issues and follow-ups

1. **Fusion's small-cell gain exceeds its bandwidth ceiling** (§3.3). Hypothesis:
   fixed cost between dependent dispatches. Discriminating test not run — a
   two-dispatch chain whose second op is independent of the first.
2. **Third problem size for the workgroup rule.** Two sizes agree on the two-arm
   rule; a third could still disagree, and would change the function's
   *signature* (a row-count argument), not just its constants.
3. **Only one adapter measured.** Every workgroup and crossover number is
   RTX 5080 / Vulkan. The DX12 backend is compiled in but unmeasured.
4. **`WorkgroupShape::default()` is now vestigial** for SpMM — the builder uses
   the rule. It remains for callers constructing a shape directly.
5. **Amaterasu remains blocked** (§14).
6. Phase 4 should build a `TensorArena`, not a ComputeGraph — see
   `docs/compute-graph-decision.md` for the argument and the three conditions
   that would reverse it.

---

## 14. Constraint compliance

| phase-brief constraint | status |
|---|---|
| Identity / ReLU / LeakyReLU with parameterisable alpha | done; Sigmoid and Tanh added as separate pipeline variants |
| no runtime shader-string generator | none; only the two workgroup substitutions every kernel uses |
| investigate in-place; no unsafe or backend-specific workaround | done — legal via a single `read_write` binding (§7) |
| workgroup sweep 64/128/256, winner not hardcoded before measurement | swept; the SpMM rule was measured, found wrong, and re-measured |
| 5–8 logarithmic intermediate workloads | 8 row counts between small and medium (§3.4) |
| cells A–G, H only after standalone activation measured | all measured; H written after cells A–C |
| no general ComputeGraph | none built; decision document delivered instead |
| no CPU file modified | verified by modification time |
| 57/58 fixture drift untouched | untouched, not silenced, not rewritten |
| no tolerance relaxed to pass a test | none; the subnormal case is a narrow signed exception, not a widened tolerance (§5.1) |
| no commit without explicit permission | nothing committed |

### 14.1 Amaterasu

Still blocked, and **no further access attempts were made this phase** — no
password attempts, no SSH configuration changes, no automatic host-key additions,
per the standing instruction.

State as established in Phase 2: port 22 at 192.168.1.228 presents the *same*
host key as `amaterasu.local:22`, so addressing was never the problem —
authentication is. Port 2222 (a git service, different host key) accepts
`id_ed25519` for `git@`; port 22 (Ubuntu 24.04, OpenSSH 9.6p1) rejects it for
both `kyber` and `git`.

**Environmental change to declare:** a `known_hosts` entry for the host was added
during Phase 2. It is the only remote-side change made across either phase.

Unblocking needs one of: this host's `id_ed25519` public key -- identified by
fingerprint `SHA256:Op1S8VGJ5w1m+9SoqzrUGxdlfu0wNn0TR8Wcphzv3TU`, comment
`msi-x-ai`, and available locally at `~/.ssh/id_ed25519.pub` -- authorised in the
target account, or a working account/host/port.

(The full key blob was trimmed to its fingerprint before publishing: the
fingerprint is what identifies the key operationally, and a repository is not
where an identity artifact needs to live.) All Phase 3 numbers are therefore
single-host, which §13 item 3 records as a limitation rather than a footnote.
