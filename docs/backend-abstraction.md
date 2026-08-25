# Backend-neutral kernel boundary — architectural proposal

**Status:** proposal only. No GPU or FPGA backend is implemented in this round.
**Created-at:** 2026-08-24
**Context:** CPU optimisation of `hymeko_core` + `hymeko_hnn`
(`docs/plans/2026-08-24-cpu-optimization/`).

---

## 1. The constraint that decides the design

The kernels this boundary would wrap are **memory-bandwidth bound**, not
compute bound. Measured on the reference host (Intel Core Ultra 9 275HX,
24 logical cores):

| kernel | scale | throughput |
|---|---|---|
| `mp_gather` | 1.2 M incidences | 886 Melem/s |
| `mp_scatter` | 1.2 M incidences | 847 Melem/s |
| `csr_spmv` | 4 M nnz | 862 Melem/s |
| `csr_spmm` fd=32 | 4 M nnz | 1.66 Gelem/s |

At ~1 Gelem/s the inner loop is spending on the order of a nanosecond per
element. Anything that inserts a non-inlinable indirection *inside* that loop —
a `dyn` call, a match on a runtime enum, a function pointer — is not a small
tax; it is comparable to the work itself. Conversely, an indirection paid
**once per kernel invocation** (once per whole SpMM, not per row) is free at any
realistic size: a virtual call is single-digit nanoseconds against a 105 ms
`csr_spmm/large/fd32`.

That asymmetry, not taste, is what picks the mechanism.

## 2. Options

| mechanism | cost in the hot loop | runtime device selection | toolchain isolation | verdict |
|---|---|---|---|---|
| **Trait + generic (static) dispatch** | zero — monomorphised, fully inlinable | no | no | **core of the design** |
| **Enum dispatch** | one predictable branch *per call* if placed at the boundary; unacceptable if placed per element | yes | no | **facade layer** |
| **Feature-gated modules** | zero | no | yes | **needed anyway** |
| `Box<dyn Backend>` | virtual call per invocation (fine) but erases the `Real` element type and blocks monomorphisation of `F` | yes | no | rejected |

`Box<dyn …>` deserves the explicit rejection: the problem is not the virtual
call, it is that `TensorCsr<F>` and every kernel are generic over
`F: Real`, and `Real` is not object-safe in a useful way here (`fn zero() ->
Self`, `fn from_other<T: Real>(…)`). Erasing it would force a concrete element
type at the boundary and give up mixed precision — which is precisely what a
future FPGA backend (fixed-point) and a CUDA backend (fp16/bf16) would want.

## 3. Recommended shape: three layers

### Layer 1 — kernel traits, static dispatch (zero cost)

```rust
/// A device that can execute the sparse kernels.
///
/// Implementors are zero-sized marker types; every method is generic over the
/// element type, so a call monomorphises to exactly the code the kernel would
/// have contained inline. There is no vtable and nothing to inline through.
pub trait SparseBackend {
    /// y = A x
    fn spmv_into<F: Real>(csr: &TensorCsr<F>, x: &[F], y: &mut [F]);

    /// Y = A X, X and Y row-major with `feature_dim` columns.
    fn spmm_into<F: Real>(csr: &TensorCsr<F>, x: &[F], feature_dim: usize, y: &mut [F]);
}

pub struct CpuSerialBackend;
pub struct CpuRayonBackend;

impl SparseBackend for CpuSerialBackend {
    #[inline]
    fn spmv_into<F: Real>(csr: &TensorCsr<F>, x: &[F], y: &mut [F]) {
        csr.spmv_into_serial(x, y)
    }
    /* … */
}

impl SparseBackend for CpuRayonBackend {
    #[inline]
    fn spmv_into<F: Real>(csr: &TensorCsr<F>, x: &[F], y: &mut [F]) {
        csr.spmv_into_parallel(x, y)
    }
    /* … */
}
```

The hypergraph kernels get a sibling trait, because their argument is a
`HyperGraphView`, not a `TensorCsr`:

```rust
pub trait HypergraphBackend {
    fn gather_into<V, EW, F>(hg: &HyperGraphView<V, EW, F>, x: &[F], out: &mut [F], use_abs: bool)
    where V: IncVal<F>, EW: EdgeWeight<V, F>, F: Real;

    fn scatter_into<V, EW, F>(hg: &HyperGraphView<V, EW, F>, xe: &[F], out: &mut [F], use_abs: bool)
    where V: IncVal<F>, EW: EdgeWeight<V, F>, F: Real;
}
```

**This layer already exists in all but name.** The
`*_into_serial` / `*_into_parallel` pairs added by this round are exactly the
two CPU implementations; the trait is a thin renaming of an API that is already
written, tested, and proven bitwise-equivalent. Introducing it is a mechanical
change, not a redesign — which is the point of doing the `*_into` split now.

### Layer 2 — enum facade for runtime selection

Static dispatch cannot answer "which device did the user configure?". One enum
at the **outermost** call site answers it, and the branch is amortised over an
entire kernel invocation:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    CpuSerial,
    CpuRayon,
    #[cfg(feature = "cuda")]
    Cuda(CudaDeviceId),
    #[cfg(feature = "fpga")]
    Fpga(FpgaHandle),
}

impl Device {
    pub fn spmm_into<F: Real>(self, csr: &TensorCsr<F>, x: &[F], fd: usize, y: &mut [F]) {
        match self {
            Device::CpuSerial => CpuSerialBackend::spmm_into(csr, x, fd, y),
            Device::CpuRayon  => CpuRayonBackend::spmm_into(csr, x, fd, y),
            #[cfg(feature = "cuda")]
            Device::Cuda(id)  => CudaBackend::new(id).spmm_into(csr, x, fd, y),
            #[cfg(feature = "fpga")]
            Device::Fpga(h)   => FpgaBackend::new(h).spmm_into(csr, x, fd, y),
        }
    }
}
```

Rule to enforce in review: **the enum never appears below the kernel entry
point.** If a `match device` shows up inside a row loop, that is the anti-pattern
this layering exists to prevent (and CLAUDE.md §6.5 #9's "bypassing existing
Strategy traits at a layer boundary" in its performance form).

The existing size-based auto-dispatch (`PAR_WORK_THRESHOLD`,
`PAR_INCIDENCE_THRESHOLD`) becomes a `Device::CpuAuto` variant rather than a
hardcoded `if` inside the kernel — same measured thresholds, now a policy the
caller can override.

### Layer 3 — feature-gated device modules

`cuda` and `fpga` need their own toolchains, build scripts, and heavy
dependencies. They belong behind cargo features so the default CPU build never
pays for them:

```toml
[features]
default = []
cuda = ["dep:cust", "dep:cudarc"]
fpga = ["dep:xrt-sys"]
```

Feature gates alone are insufficient (they cannot express runtime selection),
which is why all three layers are needed rather than any one of them.

## 4. Why the `*_into` signatures are the actual enabling change

A device backend cannot use an API that allocates and returns a `Vec<F>`: the
result must land in device-resident or pinned host memory that the caller owns
and reuses across iterations. The signatures introduced this round —

```rust
fn spmm_into(&self, x: &[F], feature_dim: usize, y: &mut [F])
```

— have the three properties a device backend needs:

1. **explicit caller-owned output** — the buffer can be pinned, mapped, or
   device-resident;
2. **no hidden allocation** — nothing to intercept or override;
3. **no ownership transfer** — no `Vec` to move across a device boundary.

The eventual generalisation replaces `&[F]` / `&mut [F]` with a buffer trait:

```rust
pub trait DeviceBuffer<F> {
    fn as_device_ptr(&self) -> DevicePtr<F>;
    fn len(&self) -> usize;
}
```

with `&[F]` implementing it trivially for the CPU tiers. That is a
*widening* of the current signature, not a break — existing call sites keep
compiling.

## 5. What a device backend will additionally need (not required now)

Flagged so they are not discovered late:

- **Asynchrony.** CUDA kernels are launched, not called. `spmm_into` returning
  `()` implies completion. A device backend needs either an internal
  synchronise (correct, slow — serialises the pipeline) or a returned
  completion token. Recommendation: keep the sync signature for v1, and add
  `spmm_into_async(&self, …) -> Completion` alongside rather than changing the
  existing one.
- **Residency.** Uploading the CSR per call would dominate everything. The
  device backend needs a `PreparedMatrix` handle that owns the uploaded
  `row_ptr` / `col_ind` / `val` across calls — analogous to how
  `GcnCliqueLayer` already precomputes its normalised adjacency once.
- **Numerics.** The bitwise-identity guarantee that holds between
  `CpuSerialBackend` and `CpuRayonBackend` **will not hold** on GPU or FPGA:
  segmented reductions reassociate, and fixed-point changes the arithmetic
  outright. The test suite must therefore split into a *bitwise* tier (CPU
  backends against each other) and a *tolerance* tier (device backends against
  a CPU reference). Do not weaken the existing bitwise assertions to accommodate
  a future device — add a second tier beside them.
- **Fallback.** Not every kernel will exist on every device on day one. A
  `Device` variant should be able to fall back to a CPU tier per kernel, and
  say so in a log, rather than failing to compile or silently doing nothing.

## 6. What not to do now

- Do not introduce the traits before there is a second real implementation.
  Layer 1 is a mechanical extraction from code that already exists; doing it
  speculatively adds indirection with no consumer, which is the "defensive
  optimization" CLAUDE.md §3 forbids in its architectural form.
- Do not add a `Device` parameter to the public kernel API yet. The current
  auto-dispatch is measured and correct; a parameter with one useful value is
  noise.
- Do not restructure the incidence data layout for a hypothetical GPU.
  The current SoA-of-flat-`Vec` layout (`edge_offsets` + `flat_edge_*`) is
  already the standard CSR shape every GPU sparse library consumes, and the
  node-first mirror is what a GPU scatter would want for the same
  no-atomics reason it serves here.
