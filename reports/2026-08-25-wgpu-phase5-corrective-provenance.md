# WGPU Phase 5 corrective — closing tensor provenance laundering

**Date:** 2026-08-25
**Scope:** `PHASE-5-CORRECTIVE: close-tensor-provenance-laundering`
**Crates touched:** `hymeko_wgpu` only.

---

## 1. What was wrong

Phase 5 removed `Deref` from `ArenaTensor` and reported that a pooled buffer
could no longer reach a raw encoder — "the dangerous direction is impossible".

**That claim was false.** An independent API audit found that
`ArenaTensor::tensor() -> &GpuTensor<f32>` was still public. A plain `&GpuTensor`
has scope `Untracked`, so:

```rust
let pooled = arena.acquire(shape)?;
let op = backend.operations().spmm()
    .matrix(m).input(x)
    .output(pooled.tensor())     // Untracked, despite pooled memory
    .prepare()?;
let mut raw = backend.encoder(None);
op.encode(&mut raw)?;            // accepted
```

Removing `Deref` closed the **implicit** path. An **explicit** one remained, one
method call wide.

The accurate description of the pre-corrective state is therefore: *the implicit
`Deref` bypass was removed, but an explicit public raw-tensor escape hatch
remained.* Phase 5's stronger wording has been annotated in place, in both the
report and `docs/compute-graph-decision.md`, rather than quietly edited away.

### 1.1 It was demonstrated, not inferred

Before changing anything, `tests/laundering_probe.rs` was written to perform
exactly the sequence above. It **compiled**, which established the hole
empirically rather than by reading the source. The same code is now a
`compile_fail` doctest (`scope::NoRawTensorAccessor`), so it is a regression test
for a demonstrated defect. The probe file was deleted once it had inverted.

---

## 2. The fix

### 2.1 Raw accessor is crate-private

`ArenaTensor::tensor()` became `pub(crate) fn gpu()`. No public API in the crate
returns a `&GpuTensor` derived from a pooled lease. Verified:

```
$ grep -rn "pub fn .*-> &GpuTensor" hymeko_wgpu/src/
  none
```

### 2.2 `TensorOperand` is sealed

Before, the trait and its `gpu_tensor` method were public, so an external crate
could write:

```rust
struct Sneaky<'a>(&'a ArenaTensor<'a>);
impl TensorOperand for Sneaky<'_> { type Scope = Untracked; }
```

and declare pooled memory untracked. That is the same laundering through a
different door, and fixing only §2.1 would have left it open.

The seal:

```rust
pub(crate) mod sealed {
    pub trait TensorAccess { fn gpu_tensor(&self) -> &GpuTensor<f32>; }
}

pub trait TensorOperand: sealed::TensorAccess {
    type Scope: EncodeScope;
}
```

The module is `pub(crate)`, so crate-internal code (builders, readback, host
writes) can reach the buffer, while an external crate can neither call
`gpu_tensor` nor implement the supertrait — and therefore cannot implement
`TensorOperand` at all. Exactly two impls exist: `GpuTensor<f32>` is `Untracked`,
`ArenaTensor<'_>` is `Tracked`.

### 2.3 Provenance-preserving data access

`read_tensor` and `write_tensor` are now generic over `TensorOperand`:

```rust
backend.read_tensor(&pooled)?;                       // Vec<f32>, no reference
backend.resources().write_tensor(&pooled, data)?;
```

One API serves both tensor kinds, and neither hands back a `&GpuTensor`. Existing
`&GpuTensor` call sites compile unchanged, since `GpuTensor` implements the
trait.

### 2.4 Metadata uses internal access

`ArenaTensor::shape()` now calls the crate-internal `gpu()`. `allocation_bytes()`
and `usage()` were already stored on the lease and never touched the accessor.
No public metadata path depends on handing out a raw tensor.

---

## 3. A consequence worth stating plainly

**An external crate can no longer write its own kernel against arena tensors.**
Building a bind group needs the `wgpu::Buffer`, and no public API exposes it for
pooled memory.

That is deliberate, and the cost was paid immediately: the Phase 5 diamond test
contains a test-only merge kernel and therefore **moved from `tests/diamond.rs`
into `src/fanout_tests.rs`** as a `#[cfg(test)]` module, where crate-internal
access is legitimate.

The alternative — a public raw-buffer accessor — reopens the hole for everyone in
order to serve custom kernels. Custom kernels belong inside the crate, where the
layering is visible. If an external kernel API is ever wanted, it should be
designed as a sealed operation trait, not as a buffer getter.

---

## 4. Compile-fail coverage

**17 doctests: 9 `compile_fail` + 8 compiling twins.** Every rejection is paired
with code differing only in the forbidden step, so a failure cannot pass for an
unrelated reason.

| # | guarantee | rejected as | twin proves |
|---|---|---|---|
| 1 | pooled operation into a raw encoder | `E0599` | the session path compiles |
| 2 | prepared operation outliving its operands | `E0505` | dropping the op first compiles |
| 3 | double release | `E0382` | one release compiles |
| 4 | aliasing a lease (`!Clone`) | `E0277` | — |
| 5 | double submit | `E0382` | one submit compiles |
| 6 | **no public `tensor() -> &GpuTensor`** | `E0599` | readback / write / shape compile |
| 7 | **no public `gpu_tensor()`** | `E0599` | — |
| 8 | **external `TensorOperand` impl** | `E0277` | both sanctioned operands compile |
| 9 | **no `Deref` to `GpuTensor`** | `E0614` | `shape()` compiles |

Rows 6–9 are new in this corrective. Rows 6 and 7 are the demonstrated hole.

---

## 5. Regression

| gate | result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo check --workspace --offline` | clean |
| `cargo build -p hymeko_wgpu --release --offline` | clean |
| `cargo test --workspace --release` | **286 passed, 1 failed** (known drift) |
| doctests | **17 passed** |

Per-suite, all passing: Phase 2 SpMM (13), Phase 3 activation/chain (19), Phase 4
arena (20), tracked session (9), diamond fan-out (5, now a unit-test module),
skeleton (8), lib unit (31).

Readback and host-write paths are exercised throughout — the arena tests poison
buffers via `write_tensor` and verify via `read_tensor`, both now through the
operand API. Legitimate access still works; only the raw reference is gone.

### 5.1 Hot path

The change is a visibility narrowing plus a monomorphised generic, so no runtime
effect is expected. Measured anyway:

| cell | before corrective | after |
|---|---:|---:|
| `acquire/hit` | 226.2 ns | 214.6 ns |
| `acquire/miss` | 775.3 ns | 658.6 ns |
| `acquire/poll` | 109.8 ns | 109.1 ns |
| `encode_only/untracked` (16 ops) | 6.55 µs | 4.86 µs |
| `encode_only/tracked` (16 ops) | 5.84 µs | 4.54 µs |

All moved slightly *faster*, which is host noise around zero rather than a
speedup — the encode path is byte-for-byte the same code. No regression.

### 5.2 CPU / core / HNN invariance

```
$ diff cpu_manifest.sha256 cpu_corrective.sha256
CPU: IDENTICAL 140/140
```

`Cargo.lock` `ca754daa0cc11107` and `Cargo.toml` `82ad3f1242f2cefb` — both
byte-identical to the Phase 5 checkpoint. Dependency tree unchanged, `vendor/`
untouched, no new dependency (`compile_fail` is a rustdoc feature).

---

## 6. What this says about the earlier claim

Phase 5 verified that *the paths it built* were closed, and did not audit the
*rest of the public surface* for equivalents. Removing `Deref` and adding an
explicit accessor in the same change is precisely the shape of mistake that
survives self-review: the new accessor looked like the sanctioned escape, so it
was documented as one rather than examined as a hole.

The generalisable rule: **when a guarantee is stated as "impossible", the
evidence has to be a compile-fail test of the specific forbidden code, not the
absence of the mechanism that was just removed.** Guarantees 6–9 exist because
that standard was not met the first time.

---

## 7. Files

| file | change |
|---|---|
| `src/scope.rs` | sealed `TensorOperand`; guarantees 5–7 added |
| `src/arena.rs` | `tensor()` became `pub(crate) gpu()`; `shape()` uses it |
| `src/backend.rs` | `read_tensor` generic over `TensorOperand` |
| `src/resources.rs` | `write_tensor` generic over `TensorOperand` |
| `src/fanout_tests.rs` | moved from `tests/diamond.rs` (needs crate-internal buffer access) |
| `src/lib.rs` | `#[cfg(test)] mod fanout_tests` |
| `tests/arena.rs`, `tests/session.rs` | operand API instead of `.tensor()` |
| `tests/laundering_probe.rs` | added to demonstrate the hole, then **deleted** |

No CORE.YAML item touched; `hymeko_wgpu` carries `lockdown: none`.
