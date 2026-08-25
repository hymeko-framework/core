# `hymeko_wgpu` — WGPU compute backend

**Created-at:** 2026-08-24
**Approval:** `APPROVED-ARCHITECTURE-EVOLUTION: hymeko-compute-framework-wgpu`
**Phase:** 2 (CSR SpMM) — complete and tested on real hardware.

Every section marks what is **implemented** versus **planned**, and which
measurement supports a decision.

---

## 1. Status

| Component | Status | Evidence |
|---|---|---|
| `WgpuBackendBuilder` / `WgpuBackend` | **implemented** | `test_backend_reports_capabilities` |
| `DeviceCapabilities` | **implemented** | reports RTX 5080, Vulkan, DiscreteGpu |
| `ResourceFactory` | **implemented** | upload/zeroed/write tests |
| `GpuTensor<f32>` | **implemented** | roundtrip test |
| `GpuCsrMatrix<f32>` | **implemented** | `test_upload_csr_validates_all_three_arrays` |
| `PipelineCache` | **implemented** | `test_pipeline_cache_builds_once_and_is_reused` |
| Command encode / submit | **implemented** | `test_two_dispatches_share_one_encoder_and_submit` |
| Explicit readback | **implemented** | `test_encode_submit_readback_roundtrip` |
| `fill.wgsl` validation kernel | **implemented** | not an operation — skeleton probe only |
| **CSR SpMM operation** | **implemented** | 13 tests in `tests/spmm.rs` |
| `SpmmBuilder` / `PreparedSpmm` | **implemented** | validation + re-dispatch tests |
| `OperationFactory` | **implemented** | `backend.operations().spmm()` |
| `WorkgroupShape` sweep (8x8, 16x8, 16x16) | **implemented** | `test_all_workgroup_variants_agree_with_cpu` |
| Timestamp query capability | **implemented** | granted on this adapter, period 1 ns |
| `ComputeGraph` | **planned — Phase 4** | gated on ≥2 operations |
| Runtime backend selection | **planned — Phase 5** | gated on measured crossover |

**One GPU kernel exists: CSR SpMM.** Phases 3-5 remain gated as before -- a
second operation is chosen from measured Phase 2 results, and the compute graph
is not introduced until two operations exist to compose.

---

## 2. Verified environment

Measured by the test suite, not assumed:

```
adapter : NVIDIA GeForce RTX 5080 Laptop GPU
backend : Vulkan
type    : DiscreteGpu
driver  : NVIDIA 610.88
```

The builder defaults to `PowerPreference::HighPerformance` because this host
also exposes an Intel iGPU; without it, wgpu may select the integrated adapter.
Backends are restricted to `VULKAN | DX12` — the only two the crate's feature
set compiles.

---

## 3. Layering and ownership

```
hymeko_core::tensor::compute          <- no wgpu, no device handles
  TensorShape  SparseCsrShape  BackendKind  ComputeError
        |                                  |
        v                                  v
hymeko_hnn  (CPU backend)          hymeko_wgpu  (this crate)
  *_into_serial / *_into_parallel     WgpuBackend owns Device + Queue
  borrowed host slices                GpuTensor / GpuCsrMatrix own Buffers
```

**Ownership rules, enforced by construction:**

1. **No core type ever holds a GPU handle.** `TensorCsr<f32>` and
   `HyperGraphView` are unchanged. `ResourceFactory::upload_csr` *adapts* their
   arrays into device buffers — the Adapter pattern.
2. **`WgpuBackend` owns every device resource.** `GpuTensor` and `GpuCsrMatrix`
   own their `wgpu::Buffer`s and die with them.
3. **`hymeko_wgpu` does not depend on `hymeko_hnn`.** It needs shapes and
   errors, not hypergraph views. That is the entire reason the vocabulary was
   placed in `hymeko_core` rather than in `hymeko_hnn`.
4. **`upload_csr` takes raw arrays, not `&TensorCsr`.** So a `HyperGraphView`
   incidence CSR can reuse the same path in Phase 3 without a second function.

---

## 4. The explicit pipeline

Nothing is implicit. This is the whole point of the design.

```rust
let backend = WgpuBackend::builder()
    .label("hymeko-compute")
    .power_preference(wgpu::PowerPreference::HighPerformance)
    .build()?;                                   // 1. acquire

let res = backend.resources();                   // 2. factory
let a = res.upload_csr("a", shape, row_ptr, col_ind, values)?;
let x = res.upload_tensor("x", shape.input_shape(fd), &host_x)?;
let y = res.zeroed_tensor("y", shape.output_shape(fd))?;

let mut encoder = backend.encoder(Some("step"));  // 3. encode
// ... several operations encode into the SAME encoder ...
let index = backend.submit(encoder);              // 4. one submit
backend.wait(index)?;                             // 5. wait
let host_y = backend.read_tensor(&y)?;            // 6. explicit readback
```

**What deliberately does not happen:**

- no submit per operation — several dispatches share one command buffer
  (`test_two_dispatches_share_one_encoder_and_submit`);
- no readback per operation — `read_tensor` is the only device→host path;
- no pipeline rebuild per dispatch — `PipelineCache` is keyed and asserted
  (`test_pipeline_cache_builds_once_and_is_reused`);
- no CSR re-upload — a `GpuCsrMatrix` is uploaded once and reused;
- no busy-polling — `wait` uses `PollType::Wait`, which blocks in the driver.

---

## 5. Design patterns, and why each one is here

| Pattern | Where | Concrete justification |
|---|---|---|
| **Builder** | `WgpuBackendBuilder` | acquisition is multi-step and fallible at each step (instance → adapter → device), and the failures are worth distinguishing |
| **Abstract Factory** | `WgpuBackend::resources()` | a buffer cannot exist without a device; only the backend owns one |
| **Command** | `encoder()` / `submit()` | batching several operations into one submit is the main GPU performance lever |
| **Adapter** | `upload_csr` / `upload_tensor` | keeps device handles out of core types |
| **Flyweight** | `PipelineCache` | pipeline creation compiles WGSL; per-dispatch rebuild is the classic way to lose to the CPU |
| **Strategy (static)** | CPU `_serial` / `_parallel` | already implemented and measured; monomorphised, no hot-loop cost |

**Rejected, with reasons:**

- **`Box<dyn Backend>`** — erases `F: Real`, forfeits mixed precision (an FPGA
  wants fixed-point, CUDA wants fp16/bf16), and blocks monomorphisation. The
  virtual call itself was never the objection.
- **`OperationFactory` in Phase 1** — there is no operation to manufacture yet.
  Introducing the factory before its product is abstraction without a consumer.
- **`ComputeGraph`** — needs ≥2 operations to compose. Gated to Phase 4.
- **Universal erased `Tensor`** — round one is explicitly f32; a runtime dtype
  tag would hide exactly what the first kernel needs to know.
- **`pollster` dependency** — 20 lines of safe `std::task::Wake` replaced it
  rather than widening the authorised vendor set.

---

## 5a. CSR SpMM — the first real operation (Phase 2)

`Y = A · X`, f32, where `A` is CSR `[num_rows × num_cols]`, `X` is dense
row-major `[num_cols × feature_dim]` and `Y` is dense row-major
`[num_rows × feature_dim]`.

### 5a.1 Lifecycle

```text
  ResourceFactory                     OperationFactory
  ───────────────                     ────────────────
  upload_csr_from(&TensorCsr)  ──┐
  upload_tensor(x)             ──┼──► spmm()
  zeroed_tensor(y)             ──┘      .matrix(&a).input(&x).output(&y)
                                        .workgroup(WorkgroupShape)
                                        .prepare()?      ◄── ALL validation
                                              │
                                              ▼
                                        PreparedSpmm
                                          .encode(&mut encoder)?   ◄── hot path
                                              │
  backend.submit(encoder)  ◄──────────────────┘   (batches N operations)
  backend.wait(index)?
  backend.read_tensor(&y)?                        ◄── explicit boundary
```

**Responsibility split:**

| stage | does | does not |
|---|---|---|
| `ResourceFactory` | allocate + upload device buffers | know about operations |
| `SpmmBuilder` | validate shapes, limits, aliasing; build/lookup pipeline; create bind group + params | dispatch, submit, read |
| `PreparedSpmm::encode` | set pipeline, set bind group, dispatch | validate, allocate, submit, read |
| `WgpuBackend` | own device, batch encoders, submit, wait, read back | know what an operation is |

`prepare()` is the only fallible-by-design stage. `encode()` returns `Result`
for forward compatibility but is currently infallible, and performs **no
validation and no allocation** — that is what makes a `PreparedSpmm` cheap to
re-dispatch.

### 5a.2 API example

```rust
use hymeko_wgpu::{WgpuBackend, WorkgroupShape};

let backend = WgpuBackend::builder().label("hymeko").build()?;
let res = backend.resources();

// Adapter: TensorCsr<f32> is unchanged and gains no device handle.
let matrix = res.upload_csr_from("a", &csr)?;
let shape  = matrix.shape();

let input  = res.upload_tensor("x", shape.input_shape(fd), &host_x)?;
let output = res.zeroed_tensor("y", shape.output_shape(fd))?;

let op = backend.operations()
    .spmm()
    .matrix(&matrix)
    .input(&input)
    .output(&output)
    .workgroup(WorkgroupShape::new(16, 8))
    .prepare()?;                       // validates everything

let mut encoder = backend.encoder(Some("step"));
op.encode(&mut encoder)?;              // no submit
// … other operations may encode into the same encoder …
let index = backend.submit(encoder);   // one submit
backend.wait(index)?;
let y = backend.read_tensor(&output)?; // explicit readback
```

Re-dispatch, including after rewriting `X` in place, needs none of the setup:

```rust
res.write_tensor(&input, &new_x)?;     // same buffer, new contents
let mut encoder = backend.encoder(None);
op.encode(&mut encoder)?;              // same PreparedSpmm, same bind group
backend.wait(backend.submit(encoder))?;
```

**Deviations from the sketch in the phase brief**, and why:

| brief | implemented | reason |
|---|---|---|
| `backend.commands()` | `backend.encoder(label)` | Phase 1 name, already in use and tested |
| `backend.submit(..).await` | `backend.submit(..)` sync | Phase 1 is deliberately runtime-free; `await` would require an async executor dependency for no benefit on native |
| `output.read(&backend).await` | `backend.read_tensor(&output)` | keeps every device operation on the object that owns the device; `GpuTensor` stays a passive handle |
| `upload_tensor(&x, shape)` | `upload_tensor(label, shape, &x)` | label-first matches the Phase 1 factory methods |

Responsibilities are separated exactly as the brief requires; only the spelling
differs, and it follows the existing ownership model rather than fighting it.

### 5a.3 Buffer and binding layout

| binding | name | address space | access | Rust source | element |
|---:|---|---|---|---|---|
| 0 | `row_ptr` | storage | read | `GpuCsrMatrix::row_ptr()` | `u32` |
| 1 | `col_ind` | storage | read | `GpuCsrMatrix::col_ind()` | `u32` |
| 2 | `values` | storage | read | `GpuCsrMatrix::values()` | `f32` |
| 3 | `x_data` | storage | read | `GpuTensor::buffer()` (input) | `f32` |
| 4 | `y_data` | storage | **read_write** | `GpuTensor::buffer()` (output) | `f32` |
| 5 | `params` | uniform | read | owned by `PreparedSpmm` | `Params` |

Exactly **one** writable binding. Five storage buffers, validated in `prepare()`
against `max_storage_buffers_per_shader_stage`.

**Parameter block** — WGSL `struct Params` and Rust `SpmmParams` are field-for-field
identical:

| offset | field | type | bytes |
|---:|---|---|---:|
| 0 | `num_rows` | `u32` | 4 |
| 4 | `num_cols` | `u32` | 4 |
| 8 | `feature_dim` | `u32` | 4 |
| 12 | `_pad` | `u32` | 4 |
| | **total** | | **16** |

The uniform address space requires a 16-byte-aligned struct size, so `_pad` is
**explicit** rather than left to the compiler, and a `const` assertion pins
`size_of::<SpmmParams>() == 16` at compile time. There is **no `usize` field**:
every value passes through the checked `try_u32` conversion and an out-of-range
extent produces `ComputeError::IndexOverflow` rather than a wrapped index.

**Buffer sizes** for shape `[r × c]`, nnz `n`, feature dim `f`:

| buffer | bytes |
|---|---|
| `row_ptr` | `4 · (r + 1)` |
| `col_ind` | `4 · n` |
| `values` | `4 · n` |
| `X` | `4 · c · f` |
| `Y` | `4 · r · f` |
| `params` | 16 |
| staging (readback only) | `4 · r · f` |

Empty tensors are backed by a 1-element (4-byte) allocation because wgpu rejects
a zero-sized buffer; the shape stays authoritative and the kernel bounds itself
by `params`, never by `arrayLength`, so the padding is never read.

### 5a.4 Pipeline cache key

```rust
PipelineKey { shader: "spmm", entry_point: "main", workgroup_x, workgroup_y }
```

Only what changes the compiled module is in the key. Buffer identity, matrix
shape, nnz and feature dimension are **not** — they are arguments, not
specialisation. `test_pipeline_is_cached_across_prepares` asserts that two
prepares over *different buffers* share one pipeline, and that a different
workgroup adds exactly one entry.

Phase 1's key held a single scalar workgroup size, which the 1D validation
kernel needed. SpMM maps one invocation to one `(row, feature)` pair and so
needs a 2D workgroup. The key was **widened to 2D** — the minimal change; a
separate 2D cache beside the 1D one would have been duplicate infrastructure.
1D kernels pass `workgroup_y = 1`.

### 5a.5 Kernel structure

One invocation computes one `Y` element:

```wgsl
let feature = gid.x;   // fastest-varying: adjacent lanes read adjacent X/Y
let row     = gid.y;
if (feature >= params.feature_dim || row >= params.num_rows) { return; }

var acc: f32 = 0.0;
for (var k = row_ptr[row]; k < row_ptr[row + 1u]; k = k + 1u) {
    acc = acc + values[k] * x_data[col_ind[k] * params.feature_dim + feature];
}
y_data[row * params.feature_dim + feature] = acc;
```

Properties, each of them deliberate:

- **feature on `gid.x`** so a wave reads contiguous `X` and writes contiguous
  `Y`; mapping rows to `x` would stride both by `feature_dim`.
- **no float atomics** — every invocation owns its `Y` element exclusively. This
  is the same disjoint-output-partition property that let the CPU backend
  parallelise without atomics.
- **accumulation order matches the CPU** term for term (`row_axpy`), so a
  duplicate column index is summed as two separate terms and an empty row yields
  exactly `0`.
- **bounds checked in both dimensions**, because the dispatch is rounded up to
  whole workgroups.
- **bounds taken from `params`, not `arrayLength`**, because buffers are padded.

### 5a.6 Zero-sized operations

A dispatch with a zero extent is invalid in WGPU. When `num_rows == 0` or
`feature_dim == 0`, `prepare()` records the operation as empty and `encode()`
**skips the pass entirely**, issuing no dispatch. The output is already correct
(there are no elements to be wrong), and `dispatch_groups()` reports `(0, 0)` so
a caller or test can observe the decision rather than infer it. Covered by
`test_empty_matrix_and_output` and `test_zero_feature_dim_is_a_valid_noop`.

### 5a.7 Validation performed at `prepare()`

| check | error |
|---|---|
| operand supplied | `ShapeMismatch` |
| `X.rows == A.cols` | `ShapeMismatch` |
| `Y.rows == A.rows` | `ShapeMismatch` |
| `X.cols == Y.cols` (feature_dim) | `ShapeMismatch` |
| input/output not the same buffer | `Backend` ("alias") |
| rows / cols / feature_dim / nnz / row_ptr length fit `u32` | `IndexOverflow` |
| workgroup invocations ≤ device limit | `Backend` |
| ≥ 5 storage buffers per stage | `Backend` |
| dispatch groups ≤ `max_compute_workgroups_per_dimension` | `IndexOverflow` |

CSR array lengths (`row_ptr == rows+1`, `col_ind == values == nnz`) are validated
earlier still, at upload, so malformed metadata never reaches an operation.

Note the ordering consequence: shape checks run before the aliasing check, so a
tensor can only be caught aliasing when its shape is otherwise valid — i.e. with
a square matrix. That is why `test_shape_and_metadata_validation_at_prepare_time`
uses a 3×3 matrix for the aliasing case.

## 5b. Elementwise activation — the second operation (Phase 3)

The point of a second operation is not the operation. It is that two prepared
operations execute **back to back on the same GPU-resident tensor, in one
command encoder and one submit**, with no host round trip between them.

### 5b.1 Lifecycle

Identical in shape to SpMM, deliberately — a reader who knows one knows both:

```
ActivationBuilder  --prepare()-->  PreparedActivation  --encode(&mut enc)-->  ...
   validates                          owns bind group          records a
   shapes, limits,                    + pipeline Arc           dispatch; no
   aliasing                           + dispatch extent        GPU work yet
```

`prepare()` is where every fallible thing happens. `encode()` cannot fail.

### 5b.2 The chain, which is the deliverable

```rust
let mut enc = backend.encoder("spmm+act");
spmm.encode(&mut enc);      // Y = A × X
act.encode(&mut enc);       // Y = f(Y), in place
backend.submit(enc);        // ONE submit
let out = backend.read_tensor(&y)?;   // readback only when asked
```

WebGPU orders dispatches within an encoder, so `act` observes `spmm`'s writes
without an explicit barrier. `Y` never leaves device memory.

### 5b.3 Kind variants: one module, many pipelines

Each activation kind is a separate WGSL **entry point** in one module, selected
by `ActivationKind::entry_point()`.

The `PipelineCache` module key is `(shader, workgroup_x, workgroup_y)` and
deliberately omits the entry point, so **all kinds share one compiled module**
and differ only by pipeline object. The alternative — a `kind: u32` uniform and
a `switch` in the body — was rejected because it puts a branch in every
invocation for a value that is uniform across the entire dispatch.

`LeakyRelu`'s `alpha` is a **uniform value, not a specialisation**: changing it
does not create a pipeline. This is asserted, not assumed
(`test_pipeline_cache_keying`).

No runtime shader-string generation. The only substitutions are the same
`{{WORKGROUP_X}}` / `{{WORKGROUP_Y}}` every kernel in this crate uses.

### 5b.4 In-place: legal, via a separate module

In-place was investigated and **is** supported — but not by binding one buffer
to both `in_data` and `out_data` of the out-of-place shader. WebGPU's
usage-scope rule forbids a buffer being a writable storage binding while also
bound elsewhere in the same pass. That is a validation error, not a portability
grey area.

The legal construction is to stop having two bindings. `activation_inplace.wgsl`
declares **one `read_write` binding** and reads and writes the same element.
No aliasing exists to violate — hence no unsafe, and no backend-specific escape
hatch, as the phase brief requires.

The cost is that the activation expressions appear in two WGSL files (WGSL has
no include mechanism). The duplication is bounded — one line per kind — and both
files are covered by the same CPU-reference tests, so a divergence between them
fails a test rather than going unnoticed
(`test_in_place_and_out_of_place_agree`).

### 5b.5 Buffer and binding layout

Out-of-place (`activation.wgsl`):

| binding | type | contents |
|---|---|---|
| 0 | `storage, read` | `in_data: array<f32>`, row-major, `rows × fd` |
| 1 | `storage, read_write` | `out_data: array<f32>`, same extent |
| 2 | `uniform` | `Params` |

In-place (`activation_inplace.wgsl`):

| binding | type | contents |
|---|---|---|
| 0 | `storage, read_write` | `data: array<f32>` |
| 1 | `uniform` | `Params` |

`Params` is 16 bytes with explicit tail padding, asserted at compile time:

| offset | field | note |
|---:|---|---|
| 0 | `count: u32` | valid element count; **never** `arrayLength()`, because empty tensors are padded to a 1-element minimum |
| 4 | `alpha: f32` | LeakyReLU slope; 0 and unread for other kinds |
| 8 | `stride: u32` | total invocations in the dispatch — see 5b.6 |
| 12 | `_pad1: u32` | uniform alignment |

**No `usize` field.** `count` arrives through checked `try_u32`, so an index
that would not survive narrowing is a `ComputeError::IndexOverflow` on the host
rather than a wrapped index in a shader.

### 5b.6 The grid-stride loop, and why it is not premature generality

The activation dispatch is 1-D with one element per invocation. A device caps
workgroups per dimension at 65 535, so a 256-wide group addresses at most
16.7M elements in one grid.

That ceiling is *inside* the workload range: 200 000 rows × 128 features is
25.6M elements, needing 100 000 groups. The first run of `chain_bench` failed
on exactly this — correctly, as an `IndexOverflow` from `prepare()` rather than
as silently truncated output.

The fix is the "one or, where justified, several elements per invocation" case
the phase brief permits:

```wgsl
for (var i = gid.x; i < params.count; i = i + params.stride) { ... }
```

with the host clamping the grid:

```rust
let ideal_groups = count.div_ceil(workgroup);
let groups = ideal_groups.min(limits.max_workgroups_per_dimension);
let stride = groups * workgroup;
```

Two properties make this the right shape:

* **Striding by the whole grid, not by contiguous blocks per invocation.**
  Adjacent lanes stay on adjacent elements in every iteration, so coalescing
  survives. A block-per-invocation split would destroy it.
* **It is free when unneeded.** If the grid already covers `count`, then
  `stride >= count` and the loop body runs exactly once — the same work the
  bounds-checked version did.

Covered by `test_count_exceeding_one_dispatch_grid`, which runs 4 194 240 + 4097
elements (just past the wg=64 single-grid ceiling, coprime with all three
workgroup sizes) and asserts the strided result is **bit-identical** to a
dispatch that fits in one grid.

### 5b.7 Numerical contract

Defined per kind rather than left to the shading language:

| kind | `NaN` | `-0.0` | `-Inf` | `+Inf` |
|---|---|---|---|---|
| `Identity` | `NaN` (bit-preserved) | `-0.0` | `-Inf` | `+Inf` |
| `Relu` | `+0.0` | `+0.0` | `+0.0` | `+Inf` |
| `LeakyRelu { alpha }` | `NaN` | `-0.0` (α>0) | `-Inf` (α>0) | `+Inf` |
| `Sigmoid` | `NaN` | `0.5` | `0.0` | `1.0` |
| `Tanh` | `NaN` | `-0.0` | `-1.0` | `1.0` |

`Relu` and `LeakyRelu` use `select`, **not** `max(v, 0.0)`: WGSL leaves `max`
with a NaN operand implementation-defined, whereas any comparison with NaN is
well-defined as false. The resulting behaviour is exactly what the CPU layers
already do (`if val > zero { val } else { zero }` in `conv/hgnn.rs`), so a
future CPU/GPU A/B of a full forward pass compares like with like.

`Sigmoid` and `Tanh` call transcendental intrinsics whose precision is bounded
but unspecified; `ActivationKind::is_transcendental()` marks them, and only they
are compared with a tolerance.

### 5b.8 Subnormals — measured, not assumed

`test_subnormal_handling_is_characterised_not_assumed` measured, rather than
guessed, what this adapter does:

* **storage and copy preserve subnormals exactly** — `Identity` round-trips even
  `0x0000_0001` (1.4e-45) bit-for-bit;
* **arithmetic flushes a subnormal *result* to zero, sign preserved** —
  `LeakyRelu { alpha: 0.01 }` on `-f32::MIN_POSITIVE` gives `-1.1755e-40` on the
  CPU and exactly `-0.0` on the GPU.

That is flush-to-zero on arithmetic, not denormals-are-zero on input. WGSL
guarantees neither.

This is admitted as a **narrow, signed exception** (`subnormal_flushed_to_zero`)
and explicitly *not* as a widened tolerance: every other value on the same test
is still compared exactly. Relaxing the tolerance would have hidden the
distinction between "storage preserves subnormals" and "arithmetic does not",
which is the part a caller actually needs to know.

### 5b.9 Validation performed at `prepare()`

| check | error |
|---|---|
| input and output present | `ShapeMismatch` |
| input and output element counts equal | `ShapeMismatch` |
| in-place requested with a distinct output | `ShapeMismatch` |
| element count narrows to `u32` | `IndexOverflow` |
| workgroup invocations within device limit | `Backend` |
| workgroup non-zero | `Backend` |
| dispatch stride computable | `IndexOverflow` |

Note what is *not* in the list: "dispatch groups exceed device limit" was a
check, and is now handled by 5b.6 instead. A limit that a legitimate workload
crosses should be accommodated, not merely reported.

## 5c. The SpMM workgroup rule (Phase 3), and the error it corrected

### 5c.1 What is chosen, and by what

`spmm_workgroup_for(feature_dim, max_invocations_per_workgroup)` is a **pure
function** applied by `SpmmBuilder::prepare()` whenever the caller has not named
a shape:

```rust
if feature_dim < 16 { 8x8 } else { 16x8 }      // then step down to fit the device
```

It is resolved in `prepare()` rather than in the constructor because the rule
needs the feature dimension, which is only known once the operands are set.

Properties the phase brief required, and where each lives:

| requirement | how |
|---|---|
| measurement-based | the table in 5c.2; nothing is chosen by intuition |
| separately testable pure function | `spmm_workgroup_for`, 8 unit tests, **no GPU needed** |
| explicit override | `SpmmBuilder::workgroup(shape)`; not checked against the rule |
| no global mutable state | a `const fn` of two arguments — no cache, no cell, no env var |
| in the pipeline-cache key | the shape flows into `PipelineKey::new_2d` |
| device-limit fallback | steps 16×8 → 8×8 → 4×4 → 1×1 to fit `max_invocations_per_workgroup` |

### 5c.2 The measurement

Two problem sizes. Times in µs; **bold** is that row's winner.

| `feature_dim` | med 8×8 | med 16×8 | med 16×16 | lg 8×8 | lg 16×8 | lg 16×16 |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | **100.4** | 113.2 | 125.2 | **429.8** | 511.6 | 510.8 |
| 4 | **114.5** | 124.3 | 127.3 | **435.7** | 521.2 | 579.5 |
| 8 | **119.2** | 130.1 | 131.6 | **513.7** | 611.7 | 655.1 |
| 16 | 145.5 | 134.7 | **123.0** | 927.1 | **675.7** | 718.3 |
| 32 | 189.8 | **176.2** | 179.0 | 1721.7 | **1197.3** | 1290.4 |
| 64 | 337.8 | 275.9 | **272.4** | 3595.2 | **2456.9** | 2649.1 |
| 128 | 651.0 | 540.2 | **531.5** | 8828.7 | **6035.0** | 7029.0 |

The mechanism is occupancy along `x`. The kernel maps `gid.x` to the feature
axis, so a 16-wide shape at `feature_dim = 1` masks off 15 of every 16 lanes.
Below 16 features, narrow-`x` wins on both sizes by 6–20 %.

### 5c.3 The error, recorded because it is the instructive part

The rule was **first fitted to the medium spec alone**, and read:

```rust
if fd < 16 { 8x8 } else if fd < 32 { 16x8 } else { 16x16 }
```

That version was **16.5 % slower at large/fd128** (7029 vs 6035 µs). It also
produced a visible regression in the chain benchmark — cell G at large/fd128
moved 6.36 ms → 7.00 ms — which was initially, and wrongly, set aside as ambient
noise on the strength of this host's history of phantom regressions.

Two things were wrong, and they are separable:

1. **Generalising a hardware tuning parameter from one problem size.** The
   feature dimension is not the only variable the shape responds to; the row
   count changes which shape wins at `feature_dim >= 16`. One spec cannot show
   that.
2. **Reaching for "it's noise" before running the discriminating test.** The
   correct move — extending the sweep to a second problem size, so all three
   shapes are compared *within one run* — settled it in six minutes. Cross-run
   comparisons on this host carry ~3 % confidence intervals and cannot resolve a
   1–10 % question; within-run A/B can.

### 5c.4 Why two arms and not three

`16×16` leads a few medium cells, so a three-arm rule is tempting. It is not
supported:

* on **large**, 16×8 beats 16×16 by 6–16 % at every `feature_dim >= 16`;
* on **medium**, 16×16's lead is at most **1.6 %**, and it is *not stable*:
  across the two sweep runs the winner at `feature_dim` 16 and 32 flipped, with
  margins inside the confidence intervals.

Trading ≤1.6 % on medium for 16.5 % on large is the entire argument.
`never_selects_16x16_automatically` locks it in, so a future edit cannot quietly
reintroduce the shape that lost.

16×16 remains reachable via the explicit override and is still swept — a shape
that loses today on one adapter is not a shape to delete.

### 5c.5 What is measured and what is not

* Measured: `feature_dim` ∈ {1, 4, 8, 16, 32, 64, 128} × {medium, large} × three
  shapes, on one adapter (RTX 5080 Laptop / Vulkan).
* Interpolated: `feature_dim` 2–3, 5–7, 9–15. The single boundary (16) sits on a
  measured point, so interpolation cannot move a winner.
* **Not measured:** any third problem size, any other adapter, any shape outside
  the three swept. The rule deliberately takes **no row-count argument**, because
  the two arms agree across both sizes measured. If a third size disagreed, the
  function's *signature* would need revisiting, not just its constants.

## 5d. `TensorArena` — intermediate buffer reuse (Phase 4)

### 5d.1 What it is, and what it refuses to be

A buffer pool. It hands out GPU allocations and takes them back. It does **not**
know what an operation is, does not schedule, does not decide when to submit, and
does not own the device.

That restraint is the design. `docs/compute-graph-decision.md` argued the
strongest case for building anything beyond explicit composition was *memory*,
not scheduling — and Phase 4 measured that a pool alone captures the whole win:

| 8 layers, 200 000 × 128 | buffers | device memory |
|---|---:|---:|
| one intermediate per layer | 8 | 781.2 MiB |
| hand-written ping-pong | 2 | 195.3 MiB |
| **`TensorArena`** | **2** | **195.3 MiB** |

The arena reaches the hand-written minimum exactly, at +0.8 % runtime, without
the caller reasoning about which buffer is free at layer *i*.

### 5d.2 Usage

```rust
let arena = backend.tensor_arena(TensorArenaConfig::default());

let a = arena.acquire(shape)?;                 // derefs to GpuTensor<f32>
let b = arena.acquire(shape)?;

let mut enc = arena.encoder(Some("chain"));    // tracked -- see 5d.4
spmm.encode(&mut enc)?;
act.encode(&mut enc)?;
arena.submit(enc);                             // no wait, no readback

arena.poll_completion()?;                      // non-blocking recycle
```

`Drop` is the release; there is no `release()` method, and the lease is not
`Clone`, so double release is unrepresentable rather than merely checked.

### 5d.3 The three states

```
free ──acquire()──> leased ──drop(lease)──> retired ──completion──> free
```

**retired** is the state that makes this more than a free list. A released buffer
may still be read by work in flight; it carries the `SubmissionEpoch` that must
complete before it is safe to hand out. Recycling it early produces wrong numbers
with no error, which is why the epoch machinery exists at all.

### 5d.4 Why the arena owns an encoder type

A released buffer is tagged with the epoch after which nothing can reference it:

* **no tracked encoder open** → the latest *issued* epoch;
* **a tracked encoder open** → the buffer may be in commands not yet submitted,
  so it is held *pending* and gets the epoch of the next `submit`.

Without the second case, `acquire → encode → drop lease → submit` — a perfectly
natural order — would recycle a buffer the pending command buffer still reads.
`ArenaEncoder` exists solely so the arena knows which case applies.

**Escape hatch, and its cost:** `WgpuBackend::encoder` still works and is *not*
tracked. Mixing it with arena tensors and releasing before submitting steps
outside the guarantee. `test_release_before_submit_is_not_recycled_early` pins the
tracked behaviour.

### 5d.5 Completion tracking — the minimum backend extension

Phase 3's backend could only answer "has this submission finished?" by
**blocking**. Waiting per release would serialise the host against the GPU and
discard the asynchrony the resident chain exists for.

`completion.rs` adds a monotonic `SubmissionEpoch` plus a watermark fed by
`Queue::on_submitted_work_done`, advanced by one non-blocking
`device.poll(PollType::Poll)`.

Chosen over two alternatives, for reasons worth keeping:

* `PollType::Poll` → `QueueEmpty` is correct but coarse — it never says *which*
  submission finished, so a pool would recycle nothing while any later work is in
  flight;
* a zero-timeout `Wait` would be per-submission, but leans on timeout semantics
  wgpu documents only as "max time to wait". An unspecified edge case is not a
  sound basis for deciding memory reuse.

The watermark uses `fetch_max`, not `store`: if completions ever arrived out of
order, the result is a *conservative* watermark rather than a freed live buffer.
The failure direction is chosen deliberately.

### 5d.6 Why prepared operations gained a lifetime

`PreparedSpmm` and `PreparedActivation` are now `Prepared*<'a>` and borrow their
operands.

They did not need to for memory safety — the bind group keeps its buffers alive
internally. They need to for *correctness under reuse*: releasing a lease returns
the buffer to the pool, which may hand it to a different logical tensor while a
prepared operation still dispatches into it. Nothing at runtime can detect that,
so the borrow checker rejects it instead.

Cost: a prepared operation cannot be stored in a `'static` structure. Nothing in
this crate does.

### 5d.7 Reuse policy and budget

`ReusePolicy::ExactSize` (default) reuses only byte-identical allocations. It is
the right default because a layered chain at fixed feature dimension asks for the
same size every layer, so the hit rate is already maximal and nothing is wasted.

`ReusePolicy::SizeClass` reuses within a power-of-two class — up to 2× internal
fragmentation for hit rate on ragged sizes. Off by default: on every workload
measured it would spend memory to buy nothing.

`TensorArenaConfig` bounds the **free list only**. Live leases and in-flight
allocations are never evicted, so a chain that genuinely needs more than the
budget still runs — the arena simply stops caching. Eviction picks the smallest
`(bytes, usage)` key deterministically, so an eviction-count assertion is
reproducible.

### 5d.8 Measured cost

| operation | median |
|---|---:|
| `acquire` + release, cache hit | 212.6 ns |
| `acquire` + release, miss (creates a buffer) | 681.1 ns |
| `poll_completion` alone | 113.0 ns |

A hit is **0.27 %** of the ~80 µs submit floor. Two acquires per chain add ~0.4 µs
to a chain costing 0.6–48 ms.

The poll is on the **miss path only**. Polling on every acquire would tax the hit
path; skipping it entirely stranded buffers released after the last poll, which
is exactly what a per-iteration inference loop does — a real defect found by
`test_repeated_chains_on_one_arena_reuse_and_stay_correct`.

### 5d.9 Deliberate non-features

No background thread, no defragmentation, no suballocation from a large buffer,
no best-fit search. Each is a real technique; none is justified by a measurement.
With two allocations per chain and a 212 ns hit, there is nothing left to win —
adding them would be optimising before profiling.

## 5e. Tracked sessions and encode scope (Phase 5)

### 5e.1 The hole, and why nothing cheaper closed it

Phase 4's arena tracked releases only for encoders it created. A pooled buffer
could be encoded into a raw `backend.encoder()`, released, and recycled while the
unsubmitted command buffer still referenced it — wrong numbers, no error.

Two cheaper fixes were considered and rejected:

* **documentation plus a test** (what Phase 4 shipped) holds only until someone
  writes the natural thing;
* **a runtime check inside `encode`** is impossible: by the time an operation
  holds `&GpuTensor<f32>`, provenance is gone. A pooled and a plain tensor are
  the same type.

Provenance therefore has to survive into the type, or it is unavailable at the
moment the decision is made.

### 5e.2 The construction

| piece | role |
|---|---|
| `TensorOperand` | associates an `EncodeScope` with each tensor type — `GpuTensor` is `Untracked`, `ArenaTensor` is `Tracked` |
| `JoinScope` | combines operand scopes; `Tracked` absorbs, so one pooled operand makes the operation tracked |
| two `encode` impls | `Untracked` takes `&mut wgpu::CommandEncoder`; `Tracked` takes `&mut TrackedSession`. There is no third |

Mixing pooled and caller-owned operands stays legal — the join handles it.

Runtime cost is zero: the markers are `PhantomData`. Measured at 0.36 µs/op
encode for tracked against 0.41 µs for untracked, i.e. no detectable difference.

### 5e.3 What it cost

`ArenaTensor` no longer implements `Deref<Target = GpuTensor>`, because `&*lease`
would produce a plain `&GpuTensor` and launder the provenance. `lease.tensor()`
is the explicit accessor for readback and host writes.

Consequence worth knowing before writing a helper: a function taking
`&GpuTensor` and one taking `&ArenaTensor` are different functions. The
benchmark and the memory example each carry one such twin.

### 5e.4 Same-session reuse

`TrackedSession::acquire` may reclaim a slab released **earlier in the same
session**, which `TensorArena::acquire` refuses to do.

Safe because a released lease can no longer be named by any operation (they
borrow it), so every dispatch touching the buffer was encoded before the release,
and commands in one command buffer execute in order.

**Not** safe across sessions: two sessions can be open at once and submit in
either order, so a slab released under one could be written by the other first.
Eligibility is therefore keyed on session id, and having two sessions open
disables it entirely.

This is what lets a fan-out reach its liveness minimum: in the diamond, `shared`
dies and `output` takes over its allocation, both inside one session. Without it
the arena allocates four buffers for a topology whose true peak is three.

### 5e.5 Session lifecycle

```text
arena.session(label) ──> encode ──> submit()  ──> SubmissionEpoch
                            └────── drop ─────>  nothing executed
```

Releases during an open session are held *pending* — undated, since the commands
may not be submitted. `submit()` stamps them with the real epoch; dropping
without submitting frees them immediately, because unsubmitted commands can never
run.

`submit(self)` consumes the session, so a second submit is a compile error rather
than a runtime check.

## 6. Adding a new operation (Phase 2 onward)

1. Write the WGSL in `shaders/<op>.wgsl`. Use `{{WORKGROUP_SIZE}}`; the cache
   substitutes it, because WGSL has no runtime workgroup size.
2. Define a `PipelineKey::new("<op>", "main", workgroup_size)`. Put in the key
   **only** what changes the compiled code — never buffer identity or shape.
3. Build a `<Op>Builder` that validates shapes with `TensorShape::validate_len`
   and narrows indices with `try_u32`. Validation happens at *prepare* time, so
   the dispatch path stays free of checks.
4. Return a `Prepared<Op>` holding the pipeline `Arc` and the bind group, so
   re-dispatch requires neither.
5. Expose `encode(&self, &mut CommandEncoder)`. **Do not submit inside it.**
6. Add a CPU-reference correctness test with absolute + relative tolerance.
7. Add the six benchmark cells from §8.

## 7. Adding a new backend (CUDA / FPGA)

The seam is already in place: `hymeko_core::tensor::compute` names shapes and
errors without naming a device, and every CPU kernel already takes explicit
output buffers (`*_into`), which is what a device backend needs in order to
write into pinned or device-resident memory.

An FPGA backend attaches at the same three points this crate uses:

1. **Resource adaptation** — an equivalent of `ResourceFactory` producing
   device-resident CSR/tensor handles from the same host arrays.
2. **Prepared operation** — validation and resource binding done once;
   `encode`/`enqueue` cheap and repeatable.
3. **Explicit transfer boundary** — the analogue of `read_tensor`.

What an FPGA backend will additionally need, flagged now rather than
discovered later: fixed-point element types (hence the deliberate absence of a
hardcoded f32 assumption in the vocabulary), and a bitstream/kernel-image cache
analogous to `PipelineCache`.

---

## 8. Benchmarking — cell separation

Defined in Phase 2 and used unchanged since; measured results live in sections
11 onward and in `reports/`.

Six separated cells, never merged into one number:

| Cell | Contents |
|---|---|
| `cpu_serial` | `spmm_into_serial` |
| `cpu_rayon` | `spmm_into_parallel` |
| `gpu_oneshot` | upload + dispatch + readback |
| `gpu_resident` | encode + submit + wait; CSR and input already on device |
| `gpu_resident_readback` | resident + readback |
| `pipeline_creation` | separate group — never inside a kernel cell |

Sweep small/medium/large × `feature_dim` ∈ {1, 8, 32, 128}, warm-up first,
≥3 separate processes, recording adapter name, backend (Vulkan/DX12), workgroup
size, buffer sizes, and the crossover point.

Run:

```sh
cargo bench -p hymeko_wgpu --offline            # GPU cells
cargo bench --bench csr_bench --offline         # CPU baseline
pwsh scripts/dev/criterion_report.ps1 -Baseline after -Compare <new>
```

---

## 9. CPU vs GPU numerics — the rule

The CPU backend guarantees **bitwise** serial ≡ parallel equality, asserted by
31 regression tests. **That guarantee does not extend to the GPU**, and no test
should pretend otherwise: a GPU SpMM may reassociate a row's accumulation
across invocations, and fused multiply-add changes rounding.

Therefore:

- CPU↔CPU comparisons stay **bitwise** — do not weaken them;
- CPU↔GPU comparisons use **absolute + relative tolerance**, with NaN and Inf
  handled explicitly rather than by tolerance;
- the two tiers live side by side. The bitwise tier is never relaxed to
  accommodate a device.

## 10. Test policy — skip vs fail

A missing adapter is an environment fact → **skip**, reported on stderr.
Anything else → **fail**:

- shader compilation failure;
- numerical mismatch against the CPU reference;
- a pipeline rebuilt when it should have been cached;
- a readback returning the wrong data.

`ComputeError::BackendUnavailable` is a separate variant from
`ComputeError::Backend` precisely so this distinction is expressible in code
rather than by string matching.

---

## 11. Phase 2 measured results (RTX 5080 Laptop, Vulkan)

**Status: PROVISIONAL.** Background CPU load sampled at 2 / 9 / 11 / 23 / 26 %
across five samples immediately before the run — unstable, and above the 10 %
bar the brief sets for a final crossover. GPU was idle (2 %, 615 MHz, 22 W) with
3217 MiB of its 16303 MiB already held by other processes. Numbers below are
sound in shape and ratio but must not be quoted as the final crossover.

Manifest hash **at benchmark time**: `92420f3292b5a524c7253d9a5980f1b07adbc7d966aa84d3659b0e5b8c37afcc`
(155 files, 850 645 bytes). The tree has since moved to
`f7b63884bb1bda9ff0c840fd00329d1d0de88ecd8d467100ab106a9d4aa318d0` -- the delta
is `cargo fmt` output plus one `type ModuleKey` alias added to satisfy
`clippy::type_complexity`, both semantically inert. Recorded rather than
back-dated, so a re-run can tell exactly what changed.

Rayon threads: 24. Single process per cell, Criterion medians.

### 11.1 Cell separation — where the time actually goes

Large workload (200k×200k, 4.0M nnz), `feature_dim = 32`:

| cell | contents | median | derived cost |
|---|---|---:|---|
| F resident | encode + submit + wait | **1.168 ms** | kernel + submit latency |
| E resident + readback | F + readback | 8.460 ms | **readback ≈ 7.29 ms** |
| D static CSR | E + X upload | 9.213 ms | **X upload ≈ 0.75 ms** |
| C one-shot | D + CSR upload + prepare | 33.512 ms | **CSR upload ≈ 24.3 ms** |

Reading that column is the whole point of separating the cells:

- **Readback dominates anything that returns to the host** — 7.29 ms to move
  25.6 MB device→host is ≈3.5 GB/s.
- **Upload is ~10× faster than readback** — 25.6 MB host→device in 0.75 ms is
  ≈34 GB/s. The asymmetry is the staging-buffer + map + copy path on the way
  back, not PCIe bandwidth.
- **CSR upload costs 24.3 ms for ~33 MB** (≈1.4 GB/s) because it also performs
  the checked `usize → u32` narrowing of `row_ptr` and `col_ind` into two
  transient `Vec<u32>`. That is the price of not letting a wrapped index reach
  the kernel, and it is paid **once** if the matrix stays resident.

**Consequence:** a one-shot GPU SpMM is never worth it (33.5 ms vs 5.9 ms on CPU
Rayon). The GPU wins only when the CSR is resident and the result does not
round-trip per call.

### 11.2 CPU vs GPU, resident execution (cell F vs cell B)

| workload | fd | CPU serial (A) | CPU Rayon (B) | GPU resident (F) | F vs Rayon |
|---|---:|---:|---:|---:|---:|
| small (1k, 8k nnz) | 1 | 9.58 µs | 26.22 µs | 82.65 µs | **0.32×** |
| small | 8 | 10.79 µs | 28.54 µs | 94.68 µs | **0.30×** |
| small | 32 | 24.28 µs | 30.46 µs | 90.65 µs | **0.34×** |
| small | 128 | 83.90 µs | 41.06 µs | 92.22 µs | **0.45×** |
| medium (50k, 800k nnz) | 1 | 1.150 ms | 137.3 µs | 111.5 µs | 1.23× |
| medium | 8 | 1.521 ms | 198.1 µs | 115.2 µs | 1.72× |
| medium | 32 | 4.028 ms | 567.1 µs | 171.6 µs | **3.30×** |
| medium | 128 | 30.30 ms | 2.179 ms | 489.4 µs | **4.45×** |
| large (200k, 4M nnz) | 1 | 10.92 ms | 1.018 ms | 441.1 µs | 2.31× |
| large | 8 | 14.50 ms | 1.716 ms | 618.8 µs | 2.77× |
| large | 32 | 69.07 ms | 5.931 ms | 1.168 ms | **5.08×** |
| large | 128 | 410.5 ms | 42.06 ms | 6.145 ms | **6.84×** |

Against **CPU serial** the large/fd128 ratio is **66.8×**; against 24-thread
Rayon it is 6.84×. Both are worth stating — the CPU baseline this project
already optimised is the honest comparator.

Peak GPU throughput: **209 Gelem/s** (medium/fd128) and 149 Gelem/s
(large/fd32), against Rayon's best of 47 Gelem/s.

### 11.3 Crossover — provisional

**GPU resident execution has a hard floor of ≈82–95 µs** that is independent of
workload size: every `small` cell lands there regardless of `feature_dim`, and
even `medium/fd1` (800k MAC) costs only 111 µs. That floor is submit + queue +
completion latency, not compute.

The practical rule that follows:

> A single GPU dispatch cannot beat a CPU call that already finishes in under
> ~90 µs, no matter how much arithmetic it contains.

Measured crossover, per feature dimension, against **CPU Rayon**:

| fd | small (CPU wins) | medium (GPU wins) | crossover lies between |
|---:|---|---|---|
| 1 | 8 k MAC | 800 k MAC | 8 k – 800 k MAC |
| 8 | 64 k MAC | 6.4 M MAC | 64 k – 6.4 M MAC |
| 32 | 256 k MAC | 25.6 M MAC | 256 k – 25.6 M MAC |
| 128 | 1.0 M MAC | 102 M MAC | 1.0 M – 102 M MAC |

Every feature dimension flips between the small and medium workloads, so the
crossover is bracketed but **not located** — the workload set has a 100× gap
there and no intermediate points. Pinning it needs a finer sweep on a quiet
machine, which is deferred rather than guessed.

Against **CPU serial** the crossover is far lower, since serial peaks at
~12 Gelem/s and loses at every scale from medium upward.

### 11.4 Workgroup sweep — no single winner

Cell F timings (resident execution):

| workload | fd | 8×8 | 16×8 | 16×16 | best |
|---|---:|---:|---:|---:|---|
| medium | 8 | 125.3 µs | 135.5 µs | **124.2 µs** | 16×16 (≈8×8) |
| medium | 32 | 203.7 µs | 169.9 µs | **168.9 µs** | 16×16 (≈16×8) |
| large | 8 | **525.3 µs** | 602.7 µs | 651.2 µs | **8×8**, 1.24× over 16×16 |
| large | 32 | 1.666 ms | **1.141 ms** | 1.233 ms | **16×8**, 1.46× over 8×8 |

**The optimum depends on `feature_dim`, and the mechanism is visible in the
mapping.** `workgroup_x` indexes the feature axis. When `feature_dim = 8` and
`workgroup_x = 16`, half of every wave's X lanes address features that do not
exist and are masked off by the bounds check — so 8×8 wins at large/fd8. When
`feature_dim = 32`, a 16-wide X dimension is fully occupied and buys better X
coalescing — so 16×8 wins at large/fd32, by 1.46×.

`WorkgroupShape::default()` remains 16×8 and is **not** presented as optimal:
it is the best single choice across the measured cells, but a feature-dim-aware
rule (roughly `workgroup_x = min(16, next_pow2(feature_dim))`) is the obvious
next step and is deliberately left unimplemented until a wider sweep supports it.

### 11.5 Prepare cost

`prepare()` with a warm pipeline cache: **22.95 µs**. That covers validation,
the params buffer write and bind-group creation — not shader compilation, which
the cache serves. It is ~25 % of the resident dispatch floor, which is why
`PreparedSpmm` exists as a reusable object rather than being rebuilt per call.

### 11.6 Memory

| item | large workload (200k×200k, 4M nnz, fd=32) |
|---|---|
| CSR host (`TensorCsr<f32>`) | `row_ptr` 8·200 001 + `col_ind` 8·4M + `val` 4·4M ≈ **49.6 MB** |
| CSR device | `4·200 001 + 4·4M + 4·4M` ≈ **32.8 MB** |
| X device | 200 000 · 32 · 4 = **25.6 MB** |
| Y device | 200 000 · 32 · 4 = **25.6 MB** |
| params | 16 B |
| staging (readback only) | 25.6 MB, allocated per `read_tensor` call |
| reused across dispatch | CSR, X, Y, params, bind group, pipeline |
| created per dispatch | command encoder only |
| created per readback | staging buffer |

Device CSR is **smaller** than host CSR (32.8 vs 49.6 MB) because host indices
are `usize` (8 B) and device indices are `u32` (4 B). Alignment padding is
limited to the 1-element minimum on empty buffers and the 16-byte params block.

**No VRAM peak is claimed.** wgpu exposes no reliable allocated-memory query,
and `nvidia-smi` reports process-wide usage that includes the 3.2 GB other
applications already hold — attributing a peak to this process from that number
would be a guess.

### 11.7 Kernel-only timestamps (cell G)

`Features::TIMESTAMP_QUERY` is **granted** on this adapter with a period of
1 ns, so the capability is present and reported in `DeviceCapabilities`.
Kernel-only timings are **not** included in the tables above: Criterion measures
host wall time, and mixing a GPU timestamp domain into the same cell would
conflate two clocks. The brief's rule — do not infer kernel time indirectly —
means the difference `F − (submit latency)` is also not reported as kernel time.
Wiring a query set into `PreparedSpmm::encode` is a small, well-scoped follow-up.
