# Published baseline benchmark — single-host provisional baseline

**Date:** 2026-08-25, 19:47–21:20 CEST (UTC+02:00)
**Host:** one machine only. No second host was available (§14.4).

| results in | source commit |
|---|---|
| §5 – §10 (Criterion suites) | **`8b9d82d6514685c5a8ba3adf0730668119a2f5a1`** |
| §11 crossover, §12 sustained, §10.2 arena memory | **`9bc4884bd8ccecf53cd0ceeeb3b5319c12bad92b`** |

The two are kept apart deliberately. `9bc4884` adds the crossover and sustained
harnesses, which did not exist at `8b9d82d`; attributing their output to a source
manifest that predates them would be false. No library code differs between the
two commits — only the harnesses — but the tables are still not merged.

> **This is a provisional baseline.** Ambient CPU load ranged 4 – 79 % during
> measurement. §11 reports a crossover **bracket**, never a point, and §14 lists
> every result that is not interpretable.

---

## 1. Manifest

```
commit (library)        8b9d82d6514685c5a8ba3adf0730668119a2f5a1
commit (harnesses)      9bc4884bd8ccecf53cd0ceeeb3b5319c12bad92b
branch / remote         main -> ssh://git@amaterasu.local:2222/hymeko/core.git
worktree                clean at both measurement points
Cargo.lock sha256       ca754daa0cc11107b19c8dbf35f1c377212886c7fcb5a9ba2631099a2ba8cb9f
source manifest sha256  b5db41d475ac66876f3db09b694655493b5c9810f66e6420e4dcfc67c5057ca3   (at 8b9d82d)
```

Shader hashes (sha256, at `8b9d82d`):

| shader | sha256 |
|---|---|
| `shaders/spmm.wgsl` | `2de3f5a03bbb85433795d4b651a2ded132174e27b8819a747f0e7eaf0911076c` |
| `shaders/spmm_activation.wgsl` | `e3d2e4122e7153ba522deb8b3932e68661665daffaa9d41fee4ff2353fb589f2` |
| `shaders/activation.wgsl` | `0f5efe4e88bc9d93d8e0e73d4db617f6187684c2ad89cd7e165a7d93beafd6e3` |
| `shaders/activation_inplace.wgsl` | `131803e38be11579171a80212103ae6de0e6cd566765ef8a88749e8c1301b917` |
| `shaders/fill.wgsl` | `8a56713ab2926c0505870cb98deea1a68d8818b56e78c66d98d0e58894a4b407` |
| `tests/shaders/merge_add.wgsl` | `d0f05b48f8c9316a03cbeaa024b7d4d41b9ac86e3aff03813530b45ca491f061` |

---

## 2. Environment

| | |
|---|---|
| CPU | Intel Core Ultra 9 275HX — 24 physical / 24 logical, 2.7 GHz base |
| RAM | 63.4 GB (21.1 GB free at start) |
| OS | Windows 11 Home 10.0.26200 build 26200 |
| GPU | NVIDIA GeForce RTX 5080 Laptop, 16 303 MiB |
| driver | NVIDIA 610.88 |
| backend | wgpu Vulkan, DiscreteGpu |
| rustc | 1.97.1 (8bab26f4f 2026-07-14), LLVM 22.1.6, `x86_64-pc-windows-msvc` |
| cargo | 1.97.1 (c980f4866 2026-06-30) |
| `RUSTFLAGS` | unset |
| release profile | no `[profile.release]` override — cargo defaults (`opt-level=3`, `lto=false`, `codegen-units=16`) |
| Rayon threads | `RAYON_NUM_THREADS` unset → 24 |
| workgroups | SpMM chosen by `spmm_workgroup_for` (fd<16 → 8×8, else 16×8); activation 256 |
| disk | 417.9 GB free |

**Environmental GPU baseline before any measurement:** 3.6 GiB VRAM used, 4 %
utilization, 55 °C. This is the desktop's own consumption (browsers, Teams,
editor) and is **not** HyMeKo's footprint — see §13.

---

## 3. Protocol

* Criterion for §5 – §10; medians reported, all normalised to microseconds.
* §11 uses a purpose-built harness rather than Criterion, because Criterion runs
  one variant to completion before the next, and on this host that separation is
  where the noise enters. It interleaves the four variants **inside each cell**
  and then repeats the whole sweep with the variant order **reversed**.
* §12 discards a 10 s warm-up and buckets 60 s into 5 s windows.
* Load and GPU telemetry sampled alongside every long run.
* No source change occurred during any measurement series.

### 3.1 Extraction bug found and fixed before reporting

The first pass of my result extractor paired the *lower confidence bound's* unit
with the *median's* value, which is wrong whenever Criterion switches units
mid-line. It produced `static_csr/medium/fd8 = 1.01 µs` sitting between 200 µs
and 7.85 ms — impossible, which is how it was caught. Re-extracted using each
median's own unit; that cell is 1010.1 µs and the series is monotonic. Every
number below comes from the corrected extraction.

---

## 4. Correctness gate

| gate | result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo check --workspace --offline` | clean |
| `cargo build --workspace --release --offline` | clean |
| `cargo test --workspace --release --offline` | **286 passed, 1 failed** |
| `cargo test -p hymeko_wgpu --release` | 117 passed |
| `cargo test -p hymeko_wgpu --doc --release` | 17 passed |

The single failure is `test_load_meta_geometry`, asserting `left: 58, right: 57`
— the pre-existing fixture drift. Not ignored, not silenced, unchanged.

Provenance compile-fail guarantees, all verified present and failing to compile:
no public `ArenaTensor -> &GpuTensor`; no public raw `gpu_tensor`; `TensorOperand`
not externally implementable; pooled operation not encodable into a raw encoder;
prepared operation cannot outlive its operands; no implicit `Deref`.

---

## 5. Frontend (`interning_bench`, `8b9d82d`)

| cell | median | throughput |
|---|---:|---:|
| `lexer_dispatch_and_scan` | 6.870 ms | 418.8 MiB/s |
| `full_frontend_pipeline` | 99.40 ms | 28.9 MiB/s |

Criterion reported +13.7 % and +9.2 % against its stored baseline. **Not a
regression:** the frontend source is unchanged in this work, and that baseline
was recorded in an earlier, quieter session. Cross-run under different load —
classified *noisy/inconclusive* (§14).

---

## 6. CPU CSR (`csr_bench`, `8b9d82d`)

### 6.1 Serial vs Rayon by nnz — SpMV

| nnz | serial | rayon | winner |
|---:|---:|---:|---|
| 1 024 | **0.53 µs** | 21.46 µs | serial 40× |
| 4 096 | **1.74 µs** | 25.49 µs | serial 14.6× |
| 16 384 | **5.32 µs** | 26.41 µs | serial 5.0× |
| 65 536 | **25.26 µs** | 29.24 µs | serial 1.16× |
| 262 144 | 137.05 µs | **44.14 µs** | rayon 3.1× |
| 1 048 576 | 726.2 µs | **105.8 µs** | rayon 6.9× |
| 4 194 304 | 5.228 ms | **536.7 µs** | rayon 9.7× |

**Crossover bracket: 65 536 – 262 144 nnz.** The shipped
`PAR_SPMV_NNZ_THRESHOLD = 81 920` falls inside it — confirmed.

### 6.2 SpMM paths at nnz ≈ 16k

| fd | serial | rayon |
|---:|---:|---:|
| 1 | **19.74 µs** | 28.61 µs |
| 4 | 31.71 µs | **30.66 µs** |
| 8 | **21.87 µs** | 30.70 µs |
| 32 | 51.22 µs | **33.80 µs** |
| 128 | 177.2 µs | **44.84 µs** |

⚠ **Non-monotonic:** serial fd4 (31.71 µs) exceeds serial fd8 (21.87 µs). Less
work should not cost more. Flagged, not averaged away — classified
*noisy/inconclusive*.

### 6.3 Allocation-free paths

| workload | allocating | `*_into` | saving |
|---|---:|---:|---:|
| small | 3.529 µs | 3.580 µs | −1.4 % (below floor) |
| medium | 98.68 µs | **86.64 µs** | 12.2 % |
| large | 751.5 µs | **673.2 µs** | 10.4 % |

Confirmed at medium and large; small is inside the noise floor.

---

## 7. CPU HNN (`hnn_bench`, `8b9d82d`)

### 7.1 Message passing, serial

| kernel | small | medium | large |
|---|---:|---:|---:|
| gather | 1.13 µs | 49.76 µs | 218.2 µs |
| scatter | 1.19 µs | 69.63 µs | 331.5 µs |
| clique diagonal | ⚠ see below | 84.70 µs | 354.5 µs |
| implicit clique step | 2.39 µs | 119.1 µs | 603.7 µs |

⚠ `mp_clique_diag/serial/small` measured **1.0187 ns** — below a single cache
miss, against 84.7 µs at medium. The work is elided or the input is degenerate.
**Not a result**; classified *inconclusive* and excluded from any claim.

### 7.2 Serial vs Rayon crossovers

| kernel | serial wins to | rayon wins from | bracket |
|---|---:|---:|---|
| gather | 16 384 (7.29 vs 28.33 µs) | 65 536 (36.44 vs 33.73 µs) | 16 k – 65 k |
| scatter | 16 384 (11.62 vs 29.02 µs) | 65 536 (115.5 vs 40.17 µs) | 16 k – 65 k |

### 7.3 Convolutions, serial

| layer | medium 32×8 | medium 32×32 | medium 32×128 | large 32×32 |
|---|---:|---:|---:|---:|
| HGNN | 1.258 ms | 3.184 ms | 9.809 ms | 16.35 ms |
| Signed-HGNN | 2.098 ms | 5.403 ms | 20.53 ms | 45.55 ms |

Signed-HGNN costs 1.67 – 2.79× HGNN, rising with feature dimension.

### 7.4 View construction

| path | small | medium | large |
|---|---:|---:|---:|
| `from_design` | 12.89 µs | 2.660 ms | 36.88 ms |
| `from_ir` | 21.04 µs | 12.41 ms | 135.9 ms |
| `from_ir` weights **Some** | 22.71 µs | 11.41 ms | — |
| `from_ir` weights **None** | 17.48 µs | 10.32 ms | — |

The weighted path costs 10.5 % (medium) to 29.9 % (small) more than unweighted.

---

## 8. WGPU SpMM (`spmm_bench`, `8b9d82d`)

### 8.1 Resident vs readback — the dominant effect

| workload | fd | resident | + readback | readback share |
|---|---:|---:|---:|---:|
| small | 1 | 74.67 µs | 146.7 µs | 49 % |
| small | 128 | 76.36 µs | 209.2 µs | 63 % |
| medium | 32 | 145.5 µs | 2 936 µs | 95 % |
| medium | 128 | 465.4 µs | 9 343 µs | 95 % |
| large | 32 | 1 029 µs | 9 063 µs | 89 % |
| large | 128 | **5 444 µs** | **45 837 µs** | **88 %** |

At production scale the host transfer costs 7–8× the compute. This is the single
largest result in the report and it is what makes resident chaining the correct
execution model.

### 8.2 Execution modes at large

| mode | fd1 | fd8 | fd32 | fd128 |
|---|---:|---:|---:|---:|
| one-shot (upload+dispatch+readback) | 93.59 ms | 106.0 ms | 154.7 ms | 139.1 ms |
| static CSR (upload once) | 586.1 µs | 2.971 ms | 10.68 ms | 133.2 ms |
| resident + readback | 530.3 µs | 2.627 ms | 9.063 ms | 45.84 ms |
| **resident** | **365.9 µs** | **464.7 µs** | **1.029 ms** | **5.444 ms** |

### 8.3 CPU vs GPU at the shipped workloads

| workload | fd | CPU serial | CPU rayon | GPU resident |
|---|---:|---:|---:|---:|
| small | 128 | 76.87 µs | **35.09 µs** | 76.36 µs |
| medium | 8 | 1 324 µs | 181.5 µs | **97.65 µs** |
| medium | 128 | 23 879 µs | 2 126 µs | **465.4 µs** |
| large | 32 | 53 167 µs | 6 208 µs | **1 029 µs** |
| large | 128 | 352 690 µs | 33 601 µs | **5 444 µs** |

GPU resident is 6.2× Rayon at large/fd128 and 4.6× at medium/fd128.

### 8.4 Workgroup sweep — the shipped rule reproduces

| cell | 8×8 | 16×8 | 16×16 | winner | margin |
|---|---:|---:|---:|---|---:|
| medium/fd8 | **98.58** | 108.4 | 107.4 | 8×8 | 9.0 % |
| medium/fd32 | 178.1 | 151.9 | **147.6** | 16×16 | 2.9 % ⚠ |
| large/fd8 | **468.3** | 548.5 | 585.6 | 8×8 | 17.1 % |
| large/fd32 | 1 506 | **1 040** | 1 127 | 16×8 | 8.3 % |

Independently reproduces `spmm_workgroup_for`: 8×8 below fd16, 16×8 above. The
medium/fd32 margin is 2.9 % — **below the 3 % floor, a tie**, exactly as Phase 5
concluded when it declined to add a third arm for 16×16.

`prepare` with a warm pipeline cache: **17.13 µs**.

---

## 9. Chain and fusion (`chain_bench`, `8b9d82d`)

| cell | large/fd1 | large/fd8 | large/fd32 | large/fd128 |
|---|---:|---:|---:|---:|
| D SpMM + readback | 546.7 µs | 2 508 µs | 9 766 µs | 46 238 µs |
| E two submits + readback | 615.4 µs | 2 714 µs | 9 744 µs | 47 126 µs |
| F one submit + readback | 554.0 µs | 2 672 µs | 9 458 µs | 45 818 µs |
| G one submit, resident | 371.5 µs | 486.0 µs | 1 164 µs | 5 730 µs |
| **H fused, resident** | **362.1 µs** | **464.0 µs** | **1 023 µs** | **5 416 µs** |

* **Readback share (F vs G) at large/fd128: 87.5 %.**
* **Fusion (G → H):** +2.5 % (fd1, below floor), +4.5 % (fd8), **+12.2 %**
  (fd32), **+5.5 %** (fd128). The fd128 figure reproduces Phase 3's 5.7 %.
* **E vs F** remains inconsistent in sign; no submit-batching claim is made.
* **Encode cost:** `act/C_gpu_activation_encoded` = **0.62 – 0.79 µs/op**,
  flat across workloads — matching the Phase 3 figure of ~0.62 µs.

⚠ **Not measured:** the 2 / 4 / 8 / 16 layer sweep requested in §G5. The existing
harnesses cover one SpMM→activation pair (`chain_bench`) and an 8-layer chain
(`arena_bench`). No 2/4/16-layer data exists and none is invented here.

---

## 10. TensorArena (`arena_bench`, `8b9d82d`; memory at `9bc4884`)

### 10.1 Lease and session costs

| operation | median |
|---|---:|
| `acquire` hit | **267.6 ns** |
| `acquire` miss (allocates) | 894.5 ns |
| `poll_completion` | 138.3 ns |
| session create + drop | 439.2 ns |
| session create + submit (empty) | 22.89 µs |
| same-session `acquire` | 756.5 ns |
| `encode_only` untracked (16 ops) | 6.362 µs → **0.398 µs/op** |
| `encode_only` tracked (16 ops) | 6.552 µs → **0.409 µs/op** |

Tracked vs untracked encode: **+2.8 %, below the 3 % floor — not interpretable**.
A cache-hit acquire is 0.33 % of the ~80 µs submit floor.

### 10.2 Memory, 8-layer chain (counted from `ArenaStats`)

| workload | variant | buffers | device |
|---|---|---:|---:|
| large/fd32 | naive | 8 | 195.3 MiB |
| large/fd32 | ping-pong | 2 | 48.8 MiB |
| large/fd32 | **arena** | **2** | **48.8 MiB** |
| large/fd128 | naive | 8 | 781.2 MiB |
| large/fd128 | ping-pong | 2 | 195.3 MiB |
| large/fd128 | **arena** | **2** | **195.3 MiB** |
| large/fd128 | arena ×4 inferences | **2** | 195.3 MiB | 

**4× reduction, reaching the hand-written ping-pong minimum exactly.** Four
successive inferences add no allocations (6 reuse hits, 2 misses, 0 evictions).

---

## 11. CPU/GPU crossover — provisional, bracketed (`9bc4884`)

288 configurations: 12 row counts × fd {8, 32, 128} × avg_degree {8, 32} ×
{uniform, irregular}, four variants each, forward and reversed = 1 152
measurements. Seed `0x005E_EDC0_FFEE`; exact nnz recorded per row.

### 11.1 The reversed pass contradicts the forward pass

This is the finding, not a footnote. Same code, same cell, only the order of the
four variants inside the cell differs:

| workload (uniform, deg 32, fd 128) | variant | forward | reversed | rev/fwd |
|---|---|---:|---:|---:|
| 1 000 rows | `cpu_rayon` | 59.7 µs | 83.6 µs | **1.40×** |
| 5 000 rows | `cpu_serial` | 2 172 µs | 1 719 µs | **0.79×** |
| 20 000 rows | `cpu_rayon` | 1 336 µs | 1 771 µs | **1.33×** |
| 100 000 rows | `cpu_rayon` | 17 364 µs | 15 829 µs | 0.91× |

Run-to-run swings of **21 – 40 %** at small and medium sizes — an order of
magnitude above the 3 % floor. Only at 100 000 rows do the directions converge
(0.89 – 0.94×), where the work finally dominates the noise. CPU load during this
sweep averaged **29.6 %, peaking at 79 %**.

### 11.2 Brackets, and where the two directions agree

`gpu_resident` overtakes `cpu_rayon` between:

| distribution | degree | fd | forward | reversed | agree? |
|---|---:|---:|---|---|---|
| irregular | 32 | 8 | 8 k – 12 k | 8 k – 12 k | **yes** |
| irregular | 32 | 32 | 5 k – 8 k | 5 k – 8 k | **yes** |
| irregular | 32 | 128 | 3 k – 5 k | 3 k – 5 k | **yes** |
| irregular | 8 | 8 | 20 k – 50 k | 12 k – 20 k | no |
| irregular | 8 | 32 | 8 k – 12 k | 12 k – 20 k | no (opposite) |
| irregular | 8 | 128 | 5 k – 8 k | 3 k – 5 k | no |
| uniform | 8 | 8 | 20 k – 50 k | 12 k – 20 k | no |
| uniform | 8 | 32 | 12 k – 20 k | 8 k – 12 k | no |
| uniform | 8 | 128 | 5 k – 8 k | 2 k – 3 k | no |
| uniform | 32 | 8 | 12 k – 20 k | 5 k – 8 k | no |
| uniform | 32 | 32 | 5 k – 8 k | 3 k – 5 k | no |
| uniform | 32 | 128 | 2 k – 3 k | 512 – 1 k | no |

**Only the three `irregular/degree 32` rows are reproducible across both
directions.** Those are the only crossovers this report treats as provisionally
established:

> **irregular, avg_degree 32: GPU overtakes Rayon at ≈ 8 k – 12 k rows (fd 8),
> ≈ 5 k – 8 k (fd 32), ≈ 3 k – 5 k (fd 128).**

For the other nine configurations the honest statement is a **span**: the
crossover lies somewhere between 512 and 50 000 rows and this host cannot narrow
it. Higher feature dimension moves it earlier — that direction is consistent
across every row and is the one qualitative claim that survives.

**No single crossover point is stated.** Reproducing this on a quiet machine is
the prerequisite for a final number.

---

## 12. Sustained throughput (`9bc4884`)

Workload: 200 000 × 200 000, nnz 5 442 303, fd 128, 8 layers = 5 572.9 MMAC per
chain, fully GPU-resident. 10 s warm-up (193 chains) discarded, then 60 s in 5 s
windows.

| window | chains/s | GMAC/s | ms/chain |
|---|---:|---:|---:|
| 0 – 5 s | 19.46 | 108.4 | 51.40 |
| 5 – 10 s | 19.50 | 108.7 | 51.28 |
| 10 – 15 s | 19.74 | 110.0 | 50.66 |
| 20 – 25 s | 19.18 | 106.9 | 52.13 |
| 30 – 35 s | 18.98 | 105.8 | 52.68 |
| 45 – 50 s | 19.88 | 110.8 | 50.32 |
| 50 – 55 s | 18.51 | 103.1 | 54.03 |
| 55 – 60 s | 18.69 | 104.2 | 53.49 |

**First 10 s: 19.48 chains/s. Last 10 s: 18.60 chains/s. Δ = −4.5 %.**

Above the 3 % floor, so this is **not** collapsed into one median. The cause is
identified from telemetry:

| | start of run | end of run |
|---|---|---|
| GPU utilization | 98 % | 98 % |
| temperature | 71 °C | **81 °C** |
| SM clock | ~2 265 MHz | **~2 000 MHz** |
| power draw | 129.6 W | 129.5 W |
| VRAM used | 4 425 MiB | 4 400 MiB |

**Power is pinned flat at ~129.5 W while the SM clock sags ~12 % and temperature
climbs 10 °C.** That is a power/thermal envelope limit — the same wattage buys
less clock as the part heats — not contention, not memory pressure. Utilization
never leaves 97–99 %, so the chain is genuinely GPU-bound throughout.

Throughput falls 4.5 % against a 12.8 % clock drop, consistent with the kernel
being partly memory-bound: memory clock did not change.

**Reportable figure: 103 – 111 GMAC/s sustained, declining ~4.5 % over 60 s under
a power/thermal limit.** A burst-only number would have overstated it.

---

## 13. Memory

### 13.1 HyMeKo device allocation — from `ArenaStats` (primary)

For the sustained workload (200 k × 128, 8 layers):

| allocation | bytes |
|---|---:|
| arena intermediates (2 × 200 000 × 128 × 4 B) | 204.8 MB |
| dense input X | 102.4 MB |
| CSR (row_ptr + col_ind + values) | 44.4 MB |
| **total HyMeKo device allocation** | **≈ 351.6 MB** |

Buffer count: **2** arena allocations, 0 evictions, regardless of layer count.

### 13.2 nvidia-smi delta (secondary)

VRAM rose from ~3 587 MiB idle to ~4 400 MiB during the run — a delta of
**≈ 810 – 930 MiB**, well above the 351.6 MB actually allocated. The difference is
driver working set, staging buffers, pipeline objects, and desktop applications
fluctuating concurrently. Reported as secondary and **not** used for any claim,
per the standing instruction that the arena's own counters are authoritative.

### 13.3 Host memory

`chain_bench` at large: peak working set 418.5 MiB, peak paged 2 162 MiB, peak
virtual 7 920 MiB. Well inside the 16 GB cap. The large virtual figure is the GPU
driver's sparse reservation, not resident.

---

## 14. Noise, limits, and what is not claimed

### 14.1 Classification

**Confirmed** (reproduced, well above floor):

* readback dominance — 88–95 % of wall time at medium and large;
* GPU resident 6.2× Rayon at large/fd128;
* workgroup rule (8×8 below fd16, 16×8 above) — independently reproduced;
* arena reaching the ping-pong minimum, 4× memory reduction;
* CPU SpMV serial/parallel bracket 65 k – 262 k nnz;
* `*_into` saving 10–12 % at medium and large;
* sustained decline of −4.5 % with an identified power/thermal cause.

**Provisional** (single-host, load-limited):

* every crossover in §11 — bracketed, three configurations reproducible;
* fusion gains at fd1 (+2.5 %, below floor).

**Noisy / inconclusive — no conclusion drawn:**

* frontend +13.7 % / +9.2 % vs stored baseline (cross-run, different load,
  unchanged source);
* `mp_clique_diag/serial/small` = 1.0187 ns (elided or degenerate);
* `csr_spmm_paths/serial/nnz16k` non-monotonic at fd4 vs fd8;
* tracked vs untracked encode (+2.8 %, below floor);
* medium/fd32 workgroup 16×16 vs 16×8 (2.9 %, below floor);
* E vs F submit batching (inconsistent sign).

### 14.2 Load record

| phase | CPU load | GPU |
|---|---|---|
| pre-flight | 14.2 % mean (4 – 28 %) | 4 % util, 55 °C, 3.6 GiB used |
| before crossover | 20.3 % mean | 18 % util, 60 °C |
| during crossover | **29.6 % mean, 12 – 79 %** | 12.9 % util, 65.4 °C, 1 222 MHz, 40.2 W |
| before sustained | 26.1 % mean | 16 % util, 65 °C |
| during sustained | 8 – 49 % | 97–99 % util, 71→81 °C, 129.5 W |

This host does not meet the "quiet machine" standard. Every conclusion above is
qualified accordingly.

### 14.3 Coverage gaps

* **2 / 4 / 16-layer chain sweep not measured** (§9) — only 1-pair and 8-layer
  harnesses exist.
* Upload, dispatch, submit and completion are not separately instrumented; only
  the composite modes in §8.2 and `prepare` in §8.4.
* One adapter only. DX12 is compiled but unmeasured.
* Power limit is not readable on this part (`[N/A]`), so power-capping is
  inferred from flat draw plus falling clock, not from a reported cap.

### 14.4 Amaterasu

Still blocked; **no access attempts were made during this work**. The git service
on port 2222 accepts the key (both pushes succeeded over it); interactive SSH on
port 22 does not. Unblocking needs that key authorised for a login account, or a
working account/host/port. All results are therefore **single-host**.

---

## 15. Reproduction

```bash
git clone ssh://git@amaterasu.local:2222/hymeko/core.git
cd core
git checkout 8b9d82d6514685c5a8ba3adf0730668119a2f5a1     # §5-§10
# or 9bc4884bd8ccecf53cd0ceeeb3b5319c12bad92b              # §11-§12

cargo bench -p hymeko_core --bench interning_bench
cargo bench -p hymeko_core --bench csr_bench
cargo bench -p hymeko_hnn  --bench hnn_bench
cargo bench -p hymeko_wgpu --bench spmm_bench
cargo bench -p hymeko_wgpu --bench chain_bench
cargo bench -p hymeko_wgpu --bench arena_bench

# 9bc4884 only:
cargo run -p hymeko_wgpu --release --example crossover_sweep > crossover.tsv
cargo run -p hymeko_wgpu --release --example sustained_chain
cargo run -p hymeko_wgpu --release --example arena_memory
```

### 15.1 Dependencies — online

`Cargo.lock` pins **checksums for all 310 registry crates** and the single git
dependency (`parser`, commit `e28a205147c48273980e63aa3338ca286974a6ed`). With
network access to crates.io and to the amaterasu git service, `cargo build`
reproduces the exact dependency set. **No vendor tree is needed.**

### 15.2 Dependencies — offline

The original host builds offline because a `vendor/` directory sits *next to* the
repository and a `.cargo/config.toml` in that parent directory redirects
crates.io to it. **Neither is part of this repository:**

* the parent directory is not a git repository at all;
* the vendor tree is ~564 MB across ~334 crates;
* the config hardcodes an absolute path from one machine.

So a fresh clone cannot build offline until an equivalent tree is created:

```powershell
./scripts/dev/prepare_offline_vendor.ps1 -TargetDir vendor -ConfigOut .cargo/config.toml
cargo check --workspace --offline
```

The script takes explicit relative targets, resolves them against the repository
root, refuses to overwrite a populated tree, and captures cargo's own config
stanza. It warns when the generated path is absolute.

> **Warning.** The parent `.cargo/config.toml` on the development host is
> machine-specific and version-controlled nowhere. Do not rely on it, and do not
> commit an absolute vendor path. The 564 MB tree is deliberately not in this
> repository.

The pinned git dependency is **not** made offline-safe by `cargo vendor` alone;
fetch it while online or supply a path override.

---

## 16. Next kernel — measurement-based priority

1. **Fuse SpMM+activation throughout a chain.** Measured 5.5 % at large/fd128 and
   12.2 % at large/fd32 for one fused pair; an 8-layer chain contains eight such
   pairs, and memory and lifetime safety are already solved. Largest remaining
   lever backed by a number.
2. **Instrument upload / dispatch / submit / completion separately.** §8.2 shows
   one-shot at 139 ms against 5.4 ms resident at large/fd128 — a 25× spread whose
   internal breakdown is currently inferred, not measured. Cheap to add, and it
   would tell us whether upload or submit dominates the one-shot path.
3. **Re-run §11 on a quiet host.** The crossover is the one deliverable this host
   could not settle: nine of twelve configurations disagree between measurement
   directions. This needs an idle machine, not more cells.
4. **Add the 2 / 4 / 16-layer sweep** (§14.3) so the per-layer scaling of the
   resident chain is measured rather than extrapolated from 1 and 8.
5. **Do not pursue arena micro-optimisation.** A cache-hit acquire is 268 ns —
   0.33 % of the submit floor — and the pool already reaches the hand-written
   allocation minimum.
