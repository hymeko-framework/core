# Phase 9 — WGPU kernel infrastructure consolidation

**Date:** 2026-08-28
**Base commit:** `62f1b96293499e8908d78342d9d963bf8ad12ae7` (tree `1407b9f8ca37fb67958b17de13b607999490caad`)
**Character:** structural consolidation before gather/scatter kernels are added.
Companion documents: `2026-08-28-graph-operation-baseline.md`,
`docs/hypergraph-synchronization.md`.

**Note on the synchronization companion.** That document was revised after this
report was first written. A late requirement established that the source of
truth is a HyMeKo IR stream over IPC, changing dynamically while inference runs.
It is therefore no longer a taxonomy with two deferred branches: it audits the
existing `ipc` feature, specifies a wire protocol and transaction lifecycle, and
promotes **S2 (dynamic topology) from deferred to required and specified**.
Still nothing implemented in Phase 9.

---

## 1. Summary

Five things changed, one of which was a real latent defect rather than a tidiness
problem.

1. **The pipeline cache could alias incompatible kernels.** Demonstrated, not
   inferred: two different WGSL sources registered under one key, the second
   silently discarded, the first's pipeline executed. The cache key is now a
   closed `KernelId` that *owns* source, entry point and layout, so the invariant
   is structural.
2. **Bind-group and pipeline layouts were rebuilt on every `prepare()`.** They
   are now cached per `LayoutId`; raw and fused SpMM share one.
3. **`encode_into` existed twice, byte-identical.** One `PreparedDispatch` now
   holds the four things every prepared operation has in common, and is the only
   ordinary compute-pass encode in the crate.
4. **`upload_csr` validated array lengths and nothing else.** A malformed
   `row_ptr` or an out-of-range `col_ind` reached the shader, where WGPU *clamps*
   the access — so the kernel computed a wrong answer instead of failing. Six
   structural checks now run host-side.
5. **`spmm.wgsl` and `spmm_activation.wgsl` carried two copies of the
   accumulation loop, and the activation formulas existed in three files.** One
   `spmm_acc`, one definition per formula, shared by compile-time concatenation.

No `hymeko_core` or `hymeko_hnn` source was modified. No new dependency, no
`unsafe`, no dynamic dispatch in a hot path, no new crate.

---

## 2. Part 0 — duplication and dependency audit

Recorded against `origin/main` before any edit.

### 2.1 The eight mechanics

| candidate | `spmm.rs` | `activation.rs` | `pipeline.rs` | class | disposition |
|---|---|---|---|---|---|
| bind-group-layout construction | `:605-643` | `:464-480` (two variants) | — | 1 kernel-independent | **removed** → `LayoutId::entries` |
| pipeline-layout construction | `:507-512` | `:378-384` | — | 1 | **removed** → cached per `LayoutId` |
| pipeline cache key + invariant | `:532` | `:386` | `:29-77` | 1 | **replaced** → `KernelId` (§3) |
| params-buffer creation | `:544-551` | `:400-407` | — | 4 resource ownership | retained per operation — the *contents* differ |
| bind-group creation | `:556-587` | `:409-444` | — | 4 | retained — different resources by definition |
| empty-dispatch handling | `:501`, `:708` | `:362`, `:539` | — | 1 | **unified** → `PreparedDispatch` |
| dispatch-group validation | `:440-500` | `:300-360` | — | 2 kernel-specific | retained — 2D tiling vs 1D grid-stride |
| `encode_into` pass body | `:701-720` | `:538-550` | — | 1 | **unified** → `PreparedDispatch::encode_into` |
| SpMM accumulation (WGSL) | `spmm.wgsl:76-92` | — | — | 1 | **unified** → one `spmm_acc` |
| activation formulas (WGSL) | `spmm_activation.wgsl` ×4 | `activation.wgsl` ×5, `activation_inplace.wgsl` ×5 | — | 1 | **unified** → `activation_formulas.wgsl` |
| checked byte/index arithmetic | — | — | — | 1 | **unified** → `sizing.rs` (§5) |

Sites for the last row, all writing `(elements * size_of::<f32>()) as u64`:
`resources.rs:194`, `arena.rs:483`, `arena.rs:959`, plus `readback.rs`'s own
copy and an unchecked `next_power_of_two` at `arena.rs:438`.

### 2.2 Classification of what was *not* removed

| item | class | why it stays |
|---|---|---|
| `SpmmParams` vs `ActivationParams` | 3 operation metadata | Different fields for different kernels. A union type would be a Cartesian-product struct. |
| SpMM 2D tiling vs activation grid-stride | 2 kernel-specific validation | They solve different problems — 65 535-group overflow vs `(row, feature)` mapping. |
| in-place vs out-of-place activation modules | 6 intentional semantic duplication | A WGSL resource-interface limit; see §6.2. |
| chunked copy in `profiling::timed_readback` | 5 profiling-only | Phase 7 rejected chunking on evidence; retained only to re-test on a third vendor. |
| `timed_readback`'s stage composition | 5 profiling-only | Phase 8 established this: a measurement that delegates the composition cannot report the components. |

---

## 3. Part A — the pipeline cache

### 3.1 The risk was real, and was measured

The brief asked whether an equal key could be used with incompatible
source/layout. It could. `get_or_create(device, key, source, layout)` consulted
only `key`; on a hit, `source` and `layout` were **ignored**. Worse, the module
cache keyed on `(shader_name, wg_x, wg_y)` — omitting the entry point *and* the
source.

A probe registered two modules under key `("probe_kernel", "main", 64, 1)`, one
writing `1.0` and one writing `2.0`, then dispatched the pipeline returned for
the *second*:

```
cached pipelines: 1
value written by the SECOND source's pipeline: 1
ALIASING CONFIRMED: the second source was silently discarded.
```

Both `PipelineKey::new` and `get_or_create` were public, so any caller could
have collided with `"spmm"` and hijacked it.

### 3.2 The new invariant

> **An equal cache key necessarily denotes source-compatible compiled code and a
> compatible layout.**

It holds structurally because `KernelId` is a closed enum that maps to source,
entry point and layout. Source is no longer a parameter, so the aliasing case is
not expressible — the probe above cannot be written against the new API.

```rust
enum KernelId { Spmm, SpmmActivation(ActivationEntry),
                ActivationOutOfPlace(ActivationEntry),
                ActivationInPlace(ActivationEntry), Fill }
```

`PipelineBundle` carries `Arc<BindGroupLayout>`, `Arc<PipelineLayout>` and
`Arc<ComputePipeline>` together, so they cannot be mismatched at the call site
either.

**Workgroup specialisation is retained** — extent is part of the key, because
WGSL has no runtime workgroup size. **The LeakyReLU slope is deliberately not**:
`alpha` is a uniform, so `ActivationEntry` carries the discriminant only. Folding
it in would have compiled a pipeline per slope value.

### 3.3 Concurrency

No lock is held across `create_shader_module` or `create_compute_pipeline`;
serialising every thread behind driver compilation is a worse trade than the
race. The consequence is accepted and bounded: two threads missing on one key may
both compile, but **insertion is idempotent**, so the first value to land wins
and the loser receives it. The cache never returns two objects for one key.
`pipeline_cache_converges_under_concurrent_misses` runs eight threads on one key
and asserts one cache entry and one shared `Arc`.

### 3.4 API restriction

`PipelineKey` and `get_or_create` are **removed**. The replacement
`bundle(device, KernelId, wg_x, wg_y)` stays public because it cannot express an
invalid combination — there is no "overly general" argument left to restrict.
`tests/skeleton.rs` migrated to it and, as a side effect, no longer hand-builds
the fill layout: it uses the bundle's.

---

## 4. Part B — shared prepared dispatch

`PreparedDispatch` owns pipeline, bind group, params buffer, extent and label,
and implements `encode_into` once.

`PreparedSpmm<'a, S>` and `PreparedActivation<'a, S>` remain distinct typed
wrappers holding one each. Every Phase 4/5 guarantee is untouched: the operand
`PhantomData<&'a ()>` lifetime, the `EncodeScope` type-state, and the split
`encode` impls that make an untracked encode of a pooled buffer a *type error*.
`PreparedDispatch` is deliberately scope-agnostic and knows nothing about
tensors.

**Rejected: a general `KernelSpec` framework.** The brief permitted it only with
a concrete third production consumer during this phase. There is none — gather
and scatter are the next phase — so it would have been machinery in advance of
its use. Also rejected: `Box<dyn Operation>`, a universal `Operation` trait, and
any runtime kernel-kind switch. None appears.

Encode-only regression: §7.

---

## 5. Part C — CSR and size correctness

### 5.1 One checked size computation

`sizing.rs` provides `f32_storage_extent` / `f32_extent_for`, returning
`StorageExtent { elements, bytes, padded_bytes }`, and `checked_size_class`.

It is used by `ResourceFactory::zeroed_tensor`, `TensorArena::acquire_with_usage`
(both the arena and the session path), and `readback_extent`, which now delegates
rather than repeating the arithmetic.

Guarantees: checked element count; checked multiplication by 4 (the previous
`elements * 4` could wrap `usize`); checked conversion to `u64` (the previous
`as u64` truncated silently); `max_buffer_size` enforced with an error naming the
limit; and `padded_bytes >= 4` for the zero-sized case.

**The padding rule is explicit**: `bytes` is logical, `padded_bytes` is what is
allocated. Readback transfers `bytes`, so an empty tensor moves nothing rather
than four bytes of padding. Keeping both numbers in one struct is what stops the
two being confused.

`arena.rs:438`'s `bytes.next_power_of_two()` became `checked_size_class`. The old
form panics in debug and **wraps to zero** in release above 2^63 — which would
have handed out a zero-sized buffer for an enormous request. Unreachable with
today's shapes, which is exactly why it would have survived until it wasn't.

### 5.2 CSR structural validation

`upload_csr` previously checked three array lengths. It now additionally
verifies `row_ptr[0] == 0`, monotonic non-decrease, every entry `<= nnz`,
`row_ptr[rows] == nnz`, and every `col_ind < cols`, each with an error naming the
violated property.

This matters because **WGPU clamps out-of-bounds storage access rather than
faulting**. A bad `col_ind` did not crash; it read a different element and
produced a plausible wrong number. The checks are O(rows + nnz) paid once per
upload, against a matrix that is then dispatched repeatedly.

`malformed_csr_is_refused_with_a_named_defect` covers all five failure modes;
`degenerate_but_valid_csr_shapes_are_accepted` guards against over-rejection
(zero rows, and all-empty rows, must still be legal). Arithmetic failure is
tested through `sizing.rs`'s unit tests, which allocate nothing.

---

## 6. Part D — WGSL semantic reuse

### 6.1 SpMM: one module, one accumulation

`spmm_activation.wgsl` is deleted; `spmm.wgsl` is canonical and carries
`spmm_acc` plus five entry points — `main_raw`, `main_relu`, `main_leaky_relu`,
`main_sigmoid`, `main_tanh`. `KernelId::Spmm` selects `main_raw` statically. No
runtime activation branch.

Numerical requirements preserved: `main_raw` is the old `main` verbatim —
identical loop, identical accumulation order, so duplicate columns are still two
terms, empty rows still write 0, and no reassociation occurs. The bind group
layout and the 16-byte `Params` are unchanged (the old `_pad: u32` slot was
already `alpha: f32` in the fused module, and Rust's `SpmmParams` already carried
`alpha`).

Equivalence was **measured**, not assumed — §7.2.

### 6.2 Activation: one formula definition, two modules

`activation_formulas.wgsl` defines `act_identity`/`act_relu`/`act_leaky_relu`/
`act_sigmoid`/`act_tanh` once. It is prepended to the out-of-place, in-place and
SpMM modules by Rust `concat!(include_str!(..), include_str!(..))` — **compile
time**, not runtime assembly. Both halves are string literals fixed in the
binary; the only substitution `PipelineCache` still performs is the workgroup
extent.

**One-module consolidation was attempted in analysis and rejected.** Serving both
in-place and out-of-place entry points from one module requires declaring
binding 0 as `read_write`, which would force the out-of-place path to bind its
*input* as a writable storage buffer — losing the read-only guarantee at the API
boundary and the driver's read-only optimisation, to save two declarations. The
brief's escape clause applies: separate entry-point wrappers retained, one
canonical semantic definition established, limitation documented in the shader
header.

Net WGSL: 172 → 160 code lines, with each formula and the accumulation loop now
written once instead of two or three times.

---

## 7. Regression results

### 7.1 Test and lint gates

| gate | result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass, 0 issues |
| `cargo check --workspace` / `build --workspace` | pass |
| workspace tests (`--no-fail-fast`) | **337 passed, 1 pre-existing failure** |
| `hymeko_wgpu` lib + integration + doc | 168 passed, 0 failed |
| WGPU benchmark smoke | pass |
| graph-operation benchmark smoke | pass |

The single failure is the known `test_load_meta_geometry` 57/58 fixture drift in
`hymeko_core`. Untouched and unsilenced; no Phase 9 file touches `hymeko_core`.

**Correction to the Phase 8 report.** It recorded "169 passed" workspace-wide.
That number was wrong — not because tests were failing, but because
`cargo test --workspace` stops at the first failing crate, so `hymeko_wgpu`'s
tests never ran in that invocation. With `--no-fail-fast` the true count on the
Phase 8 tree was already ~322. Phase 9's 337 is the first correctly-measured
workspace figure. The per-crate numbers in Phase 8 were correct throughout.

### 7.2 Resident GPU performance — the shader merge

Raw SpMM is now `main_raw` inside a five-entry-point module. Kernel-only GPU time
(K2, timestamp-bracketed), Blackwell, against the Phase 7 baseline of the
separate module:

| deg | fd | K2 baseline | K2 Phase 9 | Δ |
|---:|---:|---:|---:|---:|
| 8 | 8 | 47.1 µs | 47.2 µs | +0.2 % |
| 8 | 32 | 184.5 µs | 182.7 µs | −1.0 % |
| 8 | 128 | 1 196.1 µs | 1 236.7 µs | +3.4 % |
| 32 | 32 | 569.9 µs | 569.1 µs | −0.1 % |

The fd128 cell exceeded the 3 % gate, so it was repeated rather than excused.
Four Phase 9 runs of that cell: 1 236.7 / 1 235.5 / 1 190.6 / 1 206.7 µs, median
1 221 µs. **The baseline 1 196.1 µs lies inside that range**, whose peak-to-peak
spread is 3.9 %. Median delta +2.1 %, within gate.

The question was then settled on the second host, which Phase 7 showed has
thermal headroom (47 °C, 22.9 W of 215 W) and gives tight variance:

| deg | fd | K2 baseline | K2 Phase 9 | Δ |
|---:|---:|---:|---:|---:|
| 8 | 8 | 433.8 µs | 434.6 µs | +0.2 % |
| 8 | 32 | 1 297.0 µs | 1 294.6 µs | −0.2 % |
| 8 | 128 | 4 053.8 µs | 4 056.8 µs | +0.1 % |
| 32 | 32 | 5 343.3 µs | 5 340.1 µs | −0.1 % |

**Within ±0.2 % on every cell.** The shader merge is performance-neutral, and
the Blackwell fd128 reading was laptop variance rather than a module-size effect.

### 7.3 Encode-only and preparation

`encode_into` is now one function call deeper (`PreparedSpmm::encode_into` →
`PreparedDispatch::encode_into`) on a private method with a single caller —
inlined. K1 (host wall including submit and wait) moved −0.3 % to +6.2 % across
the four cells; as established in Phases 5, 7 and 8, this harness cannot resolve
host-wall changes below ~5 %, and K2 shows the GPU work is unchanged.

Pipeline *preparation* strictly improves: layouts are built once per `LayoutId`
rather than per `prepare()` call. `raw_and_fused_spmm_share_layout_but_not_pipeline`
asserts `layout_count() == 1` after preparing two SpMM kernels.

### 7.4 Phase 8 reusable readback is retained

`readback.rs`'s engine is unchanged apart from delegating its size check to
`sizing.rs` — a setup-time call, not per-byte work. Its 13 workspace tests pass.

Measured through the production API (isolated variant D, amaterasu):

| MiB | Phase 8 | Phase 9 | Δ |
|---:|---:|---:|---:|
| 32 | 7.093 GB/s | 7.071 GB/s | −0.3 % |
| 64 | 7.168 GB/s | 7.155 GB/s | −0.2 % |
| 96 | 7.222 GB/s | 7.196 GB/s | −0.4 % |
| 128 | 7.265 GB/s | 7.231 GB/s | −0.5 % |

Retained. The same measurement on Blackwell first read −3 % to −10 %, which was
**my own contamination** — see §10.7.

---

## 8. Line accounting

Code lines, excluding docs, comments and blanks:

| component | before | after | Δ |
|---|---:|---:|---:|
| `pipeline.rs` (non-test) | 134 | 325 | **+191** |
| `operations/spmm.rs` | 549 | 484 | **−65** |
| `operations/activation.rs` | 420 | 352 | **−68** |
| `readback.rs` | 251 | 231 | **−20** |
| `resources.rs` | 227 | 268 | +41 (CSR validation) |
| `arena.rs` | 601 | 604 | +3 |
| `operations/dispatch.rs` (new, non-test) | 0 | 50 | +50 |
| `sizing.rs` (new, non-test) | 0 | 53 | +53 |
| WGSL | 172 | 160 | **−12** |
| **net** | | | **+173** |

The honest reading: **duplication removal is net negative** (−153 across the two
operation files, readback and WGSL). The `+191` in `pipeline.rs` and the `+41` in
`resources.rs` are new *capability*, not abstraction overhead — kernel identity,
cached layouts, bundles, six CSR structural checks and four checked-arithmetic
paths that did not exist. `dispatch.rs` at 50 lines replaces ~26 duplicated ones
and is the only place the +50 could be called overhead; it removes the second
copy of an encode body that a future gather/scatter kernel would have become the
third copy of.

Tests grew from 153 to 168 in `hymeko_wgpu`.

---

## 9. Rejected abstractions

| considered | rejected because |
|---|---|
| general `KernelSpec` framework | No concrete third production consumer during this phase. |
| `Box<dyn Operation>` / universal `Operation` trait | Dynamic dispatch in a hot path; the brief forbids it and the type-state would be erased. |
| runtime kernel-kind switch in `encode` | A branch per invocation for a dispatch-uniform value. |
| single WGSL module for in-place + out-of-place | Would force the out-of-place input to bind as writable storage (§6.2). |
| folding `alpha` into kernel identity | One compiled pipeline per LeakyReLU slope. |
| a macro to hide the duplicated encode | `PreparedDispatch` is a value, not a syntax trick. |
| holding the cache mutex across compilation | Serialises every thread behind driver work; idempotent insert is cheaper (§3.3). |

---

## 10. Mistakes and false assumptions found

1. **The pipeline cache aliasing hole** — a genuine latent defect that the
   existing tests could not have caught, because none of them registered two
   sources under one key. Found by asking the brief's question and then *testing*
   it rather than reading the code and concluding.
2. **`upload_csr` trusted its input.** The comment above it said validation
   happened at `prepare()`; structural CSR validity was checked nowhere, and
   WGPU's clamping meant the failure mode was a wrong number rather than an
   error.
3. **My own Phase 8 workspace test count was wrong** (§7.1) — `--workspace`
   without `--no-fail-fast` under-reports whenever any crate fails.
4. **`MeshConv` does not exist.** `mesh_nn/mesh_conv.rs` is a 0-line file. The
   brief said to benchmark it "if it is currently complete and correct"; it is
   neither, and it is excluded rather than stubbed.
5. **A test assertion of mine was inert.** In the layout-sharing test I first
   wrote `Arc::ptr_eq(&Arc::from(ptr_a), &Arc::from(ptr_b))`, which builds two
   *new* `Arc<*const T>` and is always false; a `|| std::ptr::eq(..)` beside it
   was silently carrying the assertion. Replaced with the direct pointer
   comparison. The same class of error as the Phase 5 `compile_fail` lesson: an
   assertion that cannot fail proves nothing.
6. **Three line-splice off-by-ones** while editing `spmm.rs` and
   `activation.rs` — a `perl` insert shifted line numbers before an `awk` splice
   used them. Each caught immediately by `cargo check`; no result affected.
7. **I contaminated my own GPU measurement.** While verifying that Phase 8
   readback was retained, I had left the WGPU benchmark smoke running in the
   background on the same GPU. The readback numbers came out 3-10 % low with
   inconsistent signs, and two repeat runs disagreed with each other by 10 %.
   That is exactly the concurrent-GPU-contention rule this project already has on
   record, broken by me in the same session. After the bench exited, Blackwell
   still read low — but at **945 MHz against 2 377 MHz** earlier, i.e. thermally
   throttled after a session of benchmarking. The question was settled on
   amaterasu (47 C, 22.9 W of 215 W): **-0.3 % to -0.5 %**, retained. The lesson
   is not "laptops are noisy" — it is that a cross-host check resolved in one run
   what three same-host repeats could not.

---

## 11. Decision — kernel infrastructure

**A — consolidation is beneficial and retained.**

The cache change fixes a demonstrated correctness hole rather than a style
concern, and layouts are no longer rebuilt per `prepare()`. `PreparedDispatch`
removes the second copy of the encode body before gather/scatter would have made
it a third. WGSL now has one accumulation and one definition per activation
formula.

Not B: nothing introduced was reverted, because the one over-broad candidate
(`KernelSpec`) was rejected before being built.

Partially C: **further consolidation should wait for gather/scatter.** Two
specific items are deliberately left alone until a third operation exists — a
shared params-buffer helper (the contents differ per kernel today, so a helper
would have two callers and one shape each) and any generalisation of dispatch-
group validation (2D tiling and 1D grid-stride are genuinely different problems
at n = 2).

---

## 12. Limitations

* The Blackwell host was thermally throttled for the later measurements in this
  session (945 MHz vs 2 377 MHz). Its numbers establish correctness and rough
  scale only; every performance conclusion in §7.2 and §7.4 rests on amaterasu.
* `layout_count()` proves layouts are cached, not that the *driver* deduplicates
  identical layouts internally; the saving claimed is host-side object creation.
* The concurrency test exercises 8 threads on one key on one machine. It shows
  convergence, not the absence of all races.
* Encode-only cost is inferred from K2 invariance plus inlining, not from a
  dedicated encode-only microbenchmark; the existing harness cannot resolve the
  difference (§7.3).
