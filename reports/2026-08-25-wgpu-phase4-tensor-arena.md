# WGPU Phase 4 — TensorArena buffer reuse

**Date:** 2026-08-25
**Scope:** `APPROVED-WGPU-PHASE-4: tensor-arena-buffer-reuse`
**Crates touched:** `hymeko_wgpu` only. CPU invariance verified by **hash diff**,
not timestamps (§13).

---

## 1. Files

New:

| file | lines |
|---|---:|
| `hymeko_wgpu/src/arena.rs` | 845 |
| `hymeko_wgpu/src/completion.rs` | 244 |
| `hymeko_wgpu/tests/arena.rs` | 938 |
| `hymeko_wgpu/benches/arena_bench.rs` | 366 |
| `hymeko_wgpu/examples/arena_memory.rs` | 332 |

Modified: `Cargo.toml` (bench registration), `src/lib.rs` (module wiring),
`src/backend.rs` (`tensor_arena()`), `src/resources.rs`
(`GpuTensor::from_parts` / `into_buffer`), `src/operations/spmm.rs` and
`src/operations/activation.rs` (operand lifetime, §3), `tests/activation.rs`
(one closure became a function — consequence of the lifetime).

**New dependencies: none.** The completion tracking uses `wgpu` APIs already in
the tree and `std::sync::atomic`.

**CORE.YAML items touched: none.** `hymeko_wgpu` carries `lockdown: none`.

---

## 2. Arena API

```rust
let arena = backend.tensor_arena(TensorArenaConfig::default());

let a = arena.acquire(shape)?;          // ArenaTensor<'_>, derefs to GpuTensor<f32>
let b = arena.acquire(shape)?;

let mut enc = arena.encoder(Some("chain"));   // tracked encoder
spmm.encode(&mut enc)?;
act.encode(&mut enc)?;
let epoch = arena.submit(enc);                // no wait, no readback

arena.poll_completion()?;               // non-blocking; recycles what is done
```

* `acquire` / `acquire_with_usage` — leases; `Drop` is the release.
* `encoder` / `submit` — tracked encoder and epoch-issuing submit.
* `poll_completion` — **non-blocking** watermark advance.
* `reclaim_all` — the blocking escape hatch, for teardown.
* `trim` — drop cached buffers; never touches leased or in-flight memory.
* `stats` — exact counters (§7).

Deliberately absent: no singleton, no hidden readback, no hidden submit, no
arena handle inside any core tensor type. The arena does not know what an
operation is.

---

## 3. Ownership and lifetime model

### 3.1 States

```
free ──acquire()──> leased ──drop(lease)──> retired ──completion──> free
```

* **free** — in the pool, referenced by nothing.
* **leased** — the buffer is *moved out* of the pool, so a second lease of the
  same allocation is not merely forbidden, it is unrepresentable.
* **retired** — released, but the GPU may still read it; carries the
  `SubmissionEpoch` that must complete first.

### 3.2 The one change outside the arena

`PreparedSpmm` and `PreparedActivation` had **no lifetime parameter**. They hold
a bind group (which keeps its buffers alive internally), so outliving the operand
tensors was memory-safe. With an arena it is **not correct**: releasing a lease
returns the buffer to the pool, which may hand it to a different logical tensor
while the prepared operation still dispatches into it — wrong numbers, no error.

Nothing at runtime can detect this. The brief says to solve with lifetimes what
lifetimes can solve, so `Prepared*<'a>` now borrows its operands and the borrow
checker rejects the mistake at compile time.

**Cost, stated plainly:** a prepared operation can no longer be stored in a
`'static` structure. Nothing in this repository does. One test closure had to
become a function, because a closure cannot express that its return value borrows
its argument.

**This corrects a Phase 3 prediction.** `docs/compute-graph-decision.md` §5 said
an arena "composes with the existing model — `PreparedSpmm` and
`PreparedActivation` need no change". That was wrong: safe reuse required exactly
that change.

---

## 4. In-flight / completion model

### 4.1 The §6 gate was checked before building

The brief required halting if the backend could not supply reliable completion
information. It could not, in the form needed: `submit` returned a
`wgpu::SubmissionIndex`, but only the **blocking** `wait` consumed it, and
waiting per release would destroy the asynchrony the resident chain exists for.

Three candidate primitives were compared rather than one being assumed:

| primitive | verdict |
|---|---|
| `poll(PollType::Poll)` → `QueueEmpty` | correct but coarse — says everything finished, never *which* submission |
| `poll(Wait { index, timeout: Some(ZERO) })` | per-submission, but leans on zero-timeout semantics wgpu documents only as "max time to wait". Not a basis for deciding memory reuse. |
| **`Queue::on_submitted_work_done`** | **chosen** — documented, exact, portable, no timeout semantics |

### 4.2 The minimal extension

`completion.rs`: a monotonic `SubmissionEpoch` counter plus an
`Arc<AtomicU64>` watermark updated from the completion callback, and driven by
one non-blocking `PollType::Poll`.

The watermark uses `fetch_max`, not `store`. Submissions on one queue complete in
order, so `store` would be correct — but if that assumption were ever violated,
`fetch_max` degrades to *conservative watermark* rather than *frees a live
buffer*. The failure mode is chosen deliberately.

### 4.3 The subtle case: release before submit

A released buffer is tagged with the epoch after which nothing can reference it:

* **no tracked encoder open** → the latest *issued* epoch; everything that could
  reference it is already submitted;
* **a tracked encoder open** → the buffer may sit in commands not yet submitted.
  Tagging with the latest issued epoch would free it too early, so it is held
  *pending* and receives the epoch of the next `submit`.

`acquire → encode → drop lease → submit` is a natural order to write and is
exactly the order that corrupts results under a naive pool. It is covered by
`test_release_before_submit_is_not_recycled_early`.

---

## 5. Safety invariants

| # | invariant | how it holds | evidence |
|---|---|---|---|
| 1 | two live leases never share an allocation | ownership: a leased buffer is moved out of the pool | `test_two_live_leases_never_alias` (checked at the data level, not by handle) |
| 2 | input/output do not alias where unsupported | unchanged Phase 2/3 `prepare()` validation | `tests/spmm.rs`, `tests/activation.rs` |
| 3 | in-flight allocations are not recycled | epoch watermark | `test_in_flight_buffer_is_not_recycled_early`, `test_release_before_submit_is_not_recycled_early` |
| 4 | no foreign-device allocation enters the arena | **structural**: `acquire` is the only source of slabs and creates them on the borrowed backend; there is no insertion API | `test_arena_is_bound_to_one_backend` |
| 5 | double release impossible | `Drop` consumes; lease is not `Clone`/`Copy` | type-level |
| 6 | no stale handle reaches a new owner's buffer | the lease *owns* its tensor; release moves it out. There is no index or generation to go stale | type-level |
| 7 | zero-sized tensors stay valid | 1-element pad; kernels bound by params, not `arrayLength` | `test_zero_sized_tensor_is_valid` |
| 8 | usage-flag compatibility | usage is part of `AllocationKey`, so it partitions the pool | `test_different_usage_is_not_reused` |
| 9 | arena drop needs no unsafe | `Rc<RefCell<..>>` + `Drop`; no raw pointers anywhere | grep: zero `unsafe` in the crate |
| 10 | prepared op never holds an invalidated lease | **compile-time**, via `Prepared*<'a>` (§3.2) | type-level |

Invariants 5, 6 and 10 are enforced by the type system rather than by runtime
checks, which is why they have no test: a test would have to fail to compile.

**Device identity, honestly.** wgpu 30 exposes no public device id, so
`owns_device` compares backend object identity. That is sufficient because one
`WgpuBackend` owns one `wgpu::Device`, and the arena borrows its backend for its
whole life. The residual case — an arena tensor passed to an operation built from
a *different* backend — is caught by wgpu's own bind-group validation, not by
this crate.

---

## 6. Correctness

| suite | tests | result |
|---|---:|---|
| `tests/arena.rs` | 20 | pass |
| `tests/activation.rs` (Phase 3 regression) | 19 | pass |
| `tests/spmm.rs` (Phase 2 regression) | 13 | pass |
| `tests/skeleton.rs` | 8 | pass |
| lib unit (arena, completion, workgroup rule, block_on) | 22 | pass |
| doc-tests | 1 | pass |
| **`hymeko_wgpu` total** | **83** | **pass** |
| workspace `--release --no-fail-fast` | 222 | 221 pass, 1 known drift |

The single failure is the pre-existing 57/58 `test_load_meta_geometry` fixture
drift. **Untouched, not silenced.**

### 6.1 Method

Two techniques, because a pool fails *silently*:

* **Poison** — recycled buffers are pre-filled with `-98765.5`, a value the
  computation cannot produce. A leak shows up as a recognisable number.
* **Differential** — every arena result is compared **bit-exactly** against the
  same computation on freshly allocated buffers.

### 6.2 Three tests failed first; one was a real defect

* `test_byte_budget_...` — **my test was wrong.** It released buffers one at a
  time, so each release reused the single cached slab and the free list never
  reached the ceiling. Fixed by holding five leases simultaneously.
* `test_eight_layer_chain_...` — **my test was wrong.** Layer 0 writes `a` and
  parity alternates, so layer 7 writes `b`; I read `a`.
* `test_repeated_chains_...` — **a real arena defect.** A buffer released *after*
  `reclaim_all` sat in `retired` until something polled, so the next `acquire`
  allocated a second buffer instead of reusing. That is precisely what a
  per-iteration inference loop does. Fixed by polling on the free-list **miss**
  path only — where a non-blocking poll (113 ns) is far cheaper than the
  `create_buffer` (681 ns) it avoids — leaving the hit path untaxed.

---

## 7. Allocation and reuse statistics

From `examples/arena_memory` (counted, not estimated), 8-layer chain:

| case | fd | buffers created | hits | misses | evictions |
|---|---:|---:|---:|---:|---:|
| any (C, single chain) | any | 2 | 0 | 2 | 0 |
| any (D, 4 chains on one arena) | any | 2 | 6 | 2 | 0 |

Eight logical intermediates collapse to **two allocations**, and four repeated
inferences add none. Hit rate over D is 6/8 = 75 %; the two misses are the first
chain's cold start, which no policy can avoid.

---

## 8. Memory — naive vs ping-pong vs arena

`LAYERS = 8`. "logical" is what 8 independent intermediates would occupy.

| case | fd | A naive | B ping-pong | C arena | D arena ×4 | logical |
|---|---:|---:|---:|---:|---:|---:|
| small | 8 | 0.2 MiB (8 buf) | 0.1 (2) | 0.1 (2) | 0.1 (2) | 0.2 |
| small | 32 | 1.0 (8) | 0.2 (2) | 0.2 (2) | 0.2 (2) | 1.0 |
| medium | 32 | 48.8 (8) | 12.2 (2) | 12.2 (2) | 12.2 (2) | 48.8 |
| medium | 128 | 195.3 (8) | 48.8 (2) | 48.8 (2) | 48.8 (2) | 195.3 |
| large | 32 | 195.3 (8) | 48.8 (2) | 48.8 (2) | 48.8 (2) | 195.3 |
| **large** | **128** | **781.2 (8)** | **195.3 (2)** | **195.3 (2)** | **195.3 (2)** | **781.2** |

**The arena reaches the hand-written ping-pong minimum exactly** — same buffer
count, same bytes, at every size. 4× reduction on the headline case.

### 8.1 The brief's estimate was right; the unit was the discrepancy

The brief predicted ~819 MiB naive and ~205 MiB reused. Measured: 781.2 MiB and
195.3 MiB. These are the same numbers in different units — 819 MB = 781.2 MiB and
205 MB = 195.3 MiB. The estimate was accurate; only MB vs MiB differed.

### 8.2 Host memory

| target | filter | wall | peak working set | peak paged | peak virtual | ≤16 GB |
|---|---|---:|---:|---:|---:|:-:|
| `arena_bench` | `chain/.*large` | 112.5 s | **418.5 MiB** | 2162.2 MiB | 7919.8 MiB | yes |

2.6 % of the cap. The ~7.9 GiB *virtual* figure is the GPU driver's sparse
reservation, not resident — the reason CLAUDE.md §4 forbids `ulimit -v` here.

### 8.3 What is not claimed

**No VRAM peak is asserted.** wgpu 30 exposes no reliable device-memory query, so
the "device MiB" column is what the arena *allocated*, which is a lower bound on
VRAM, not the driver's actual peak. The brief explicitly warned against claiming
otherwise.

---

## 9. Performance overhead

### 9.1 Lease cost

| cell | median |
|---|---:|
| `acquire/hit` (warm free list) | **212.6 ns** |
| `acquire/miss` (creates a buffer) | 681.1 ns |
| `acquire/poll` (non-blocking poll alone) | 113.0 ns |

**Acceptance target 1 met.** A cache-hit acquire+release is 212.6 ns — comfortably
sub-microsecond, and **0.27 %** of the ~80 µs submit floor Phase 3 measured. Two
acquires per chain add ~0.4 µs to a chain costing 0.6–48 ms.

A miss is 3.2× a hit, which is the cost the pool exists to avoid.

### 9.2 The 8-layer chain

All three variants measured **in one process, in sequence**.

| case | fd | naive | ping-pong | arena | arena vs ping-pong |
|---|---:|---:|---:|---:|---:|
| small | 8 | 590.3 µs | 667.9 µs | 640.1 µs | −4.2 % |
| small | 32 | 620.1 µs | 617.7 µs | 607.6 µs | −1.6 % |
| small | 128 | 612.5 µs | 674.1 µs | 659.3 µs | −2.2 % |
| medium | 8 | 831.1 µs | 913.1 µs | 887.5 µs | −2.8 % |
| medium | 32 | 1.400 ms | 1.312 ms | 1.306 ms | −0.4 % |
| medium | 128 | 4.440 ms | 4.402 ms | 4.307 ms | −2.1 % |
| large | 8 | 3.833 ms | 3.809 ms | 3.821 ms | +0.3 % |
| large | 32 | 8.747 ms | 9.223 ms | 8.806 ms | −4.5 % |
| large | 128 | 47.41 ms | 47.46 ms | 47.85 ms | **+0.8 %** |

**Acceptance target 2 met.** The worst regression against ping-pong is **+0.8 %**
(large/fd128), well inside the 3 % allowance. Most cells show the arena
marginally *faster*, which is not a real speedup — the two paths do identical GPU
work — but noise around zero, which is the expected result.

A repeat during the memory measurement gave large/fd128 ping-pong 49.78 ms and
arena 50.18 ms: **+0.8 % again**. The one number that matters reproduced.

### 9.3 Encode-only and repeated inference

| cell | ping-pong / fresh | arena | delta |
|---|---:|---:|---:|
| `encode/medium/fd8` | 344.1 µs | 347.8 µs | +1.1 % |
| `encode/medium/fd128` | 384.9 µs | 368.6 µs | −4.2 % |
| `inference/medium/fd32` (×4) | 5.293 ms | 5.352 ms | +1.1 % |
| `inference/medium/fd128` (×4) | 18.57 ms | 18.30 ms | −1.5 % |

Encoding is not where the arena costs anything: two acquires (~0.4 µs) against
~345 µs of encode.

### 9.4 Measurement conditions

Ambient CPU load 22 % before the run — not a quiet machine by CLAUDE.md §3's
standard. Mitigations applied consistently: every decisive comparison is
within-run and in-process; the acceptance figure (+0.8 %) was reproduced in a
second run; no conclusion rests on a difference below ~3 %.

---

## 10. The 8-layer resident chain — result

The chain runs **GPU-resident end to end**: no readback between layers, one
submit for all 16 dispatches, two buffers total.

* correctness: bit-identical to the one-buffer-per-layer reference
  (`test_eight_layer_chain_matches_and_reuses`), with poison proving every
  element was written;
* memory: 195.3 MiB vs 781.2 MiB naive at large/fd128 — **4×**;
* runtime: +0.8 % vs hand-written ping-pong;
* buffers created: 2, and still 2 after four repeated inferences.

---

## 11. Budget and eviction behaviour

Ceilings apply to the **free list only**. Live leases and in-flight allocations
are never evicted, so a chain needing more than the budget still runs — the arena
simply stops caching.

Eviction is deterministic: the victim is the smallest `(bytes, usage)` key, then
the last slab in that bucket. Sorting rather than relying on `HashMap` order is
what makes an eviction-count assertion reproducible.

If a single incoming slab exceeds the whole budget, it is dropped rather than
cached — correct, because at that moment it is referenced by nothing.

Covered by `test_byte_budget_evicts_and_never_touches_live_leases`,
`test_max_cached_buffers_is_enforced`, `test_trim_clears_cache_only`.

Default: 512 MiB / 64 buffers — sized so the measured working set (two 102 MiB
intermediates) caches comfortably.

---

## 12. ComputeGraph re-evaluation

Full argument appended to `docs/compute-graph-decision.md`. Summary:

| Phase 3 condition | Phase 4 verdict |
|---|---|
| chain long enough that manual encoding is the bottleneck | **not met** — encode is ~345 µs of an 831 µs chain, and the arena adds ~1 %. The *branching* half of the condition is **untested**: the 8-layer chain is linear. |
| a backend that cannot use `CommandEncoder` semantics | **not met** — no CUDA/FPGA backend exists |
| a measured win from reordering | **not met** — a dependency chain offers no freedom to reorder |

**Still no ComputeGraph.** The problem it was proposed to solve — intermediate
memory — was solved by a buffer pool that reaches the hand-written optimum exactly
at ≤1 % cost.

One new argument *for* a graph did surface, and is recorded rather than buried:
**the arena cannot see an encoder it did not create.** `WgpuBackend::encoder`
bypasses the pending-release tracking. A graph owning execution would close that
by construction — but so does making the tracked encoder the ergonomic path,
which is far cheaper than a scheduler.

The next thing that would change the verdict is a **branching / fan-out
topology**, not a longer linear chain: a linear chain of any length needs two
leases.

---

## 13. CPU / core / HNN invariance — by hash

Checkpoint captured before any edit: `git status` (42 entries),
`git diff --name-only` (24), `git diff --stat` (2275+/406−), and a 164-entry
SHA-256 manifest.

```
$ diff cpu_manifest.sha256 cpu_manifest_now.sha256
IDENTICAL: all 140 hymeko_core + hymeko_hnn files unchanged by hash
```

The 140 files cover every `.rs` and `.toml` in `hymeko_core/` and `hymeko_hnn/`.
No timestamp was used for this claim.

Backup: `../phase3-backup-20260825/`, verified by comparing all 159 source hashes
against the working tree — identical. Not auto-deleted.

**Not modified:** `hymeko_core`, `hymeko_hnn`, CPU benchmarks, CPU thresholds,
`vendor/`, and the workspace dependency tree (`Cargo.lock` unchanged; the only
`Cargo.toml` edit is a `[[bench]]` registration).

### 13.1 §6.5 anti-patterns

None introduced.

* **#1/#5 (Cartesian API):** reuse policy is a config field (`ReusePolicy`), not
  `acquire_exact()` / `acquire_sizeclass()`.
* **#7 (string-typed config):** `ReusePolicy` is an enum.
* **#11 (global mutable state):** no singleton arena; the `Rc<RefCell<PoolInner>>`
  is one instance per arena, created and dropped by the caller. The `AtomicU64`
  watermark is owned by its tracker, not global.
* **#13 (`_v2` files):** none; existing files edited in place.
* **#18/#19:** the arena defaults to `ExactSize` because it is what the measured
  workloads need; `SizeClass` is opt-in and documented as buying nothing on
  chains whose sizes repeat exactly. No unmeasured optimisation was defaulted.

**Deliberate non-features, all justified by absence of measurement:** no
background thread, no defragmentation, no suballocation from a large buffer, no
best-fit search.

---

## 14. Measurement-based recommendation for the next kernel

Ranked by what the Phase 3–4 measurements actually support.

1. **The next kernel should reduce dispatch count, not buffer count.** Memory is
   now solved: two buffers serve any linear chain. Runtime at large/fd128 is
   47.4 ms for 16 dispatches, and Phase 3 measured fusion (cell H) recovering
   5–21 % by removing one dispatch of a pair. A **fused SpMM+activation used
   throughout the chain** would remove 8 of the 16 — the single largest
   measured lever available.
2. **Do not pursue suballocation.** With 2 allocations per chain, the buffer
   *object* count is already minimal, and `acquire/hit` at 212.6 ns is 0.27 % of
   the submit floor. There is nothing to win.
3. **Do not add an allocation fast path.** A miss costs 681 ns and happens twice
   per arena lifetime.
4. **Measure a branching topology before extending the arena.** Every Phase 4
   number is from a linear chain, and that is exactly the case the ComputeGraph
   conditions single out as untested. A fan-out workload is the next thing that
   could change an architectural decision — worth more than another 5 % on a
   linear chain.
5. **The SpMM workgroup rule is still tuned on one adapter.** Unchanged from
   Phase 3, and it remains the largest single-cell uncertainty (16.5 % swing
   between shapes at large/fd128).

### 14.1 Open issues

1. `WgpuBackend::encoder` bypasses arena release tracking (§12). Documented and
   test-pinned, not closed.
2. Branching / fan-out lifetime is untested.
3. `Prepared*<'a>` cannot be stored `'static` — no current caller needs to.
4. One adapter only (RTX 5080 Laptop / Vulkan). DX12 compiled, unmeasured.
5. VRAM peak not claimed (§8.3).
6. Amaterasu remains blocked; no access attempts were made this phase.
