# WGPU spatial global-entropy forward

**Date:** 2026-09-06 · **Classification:** `CanonicalCpuSemantics + WgpuBackendImplementation`

## Boundary

This boundary adds the WGPU implementation of Nagare's existing
`global_entropy_pool_forward` semantics. It does not add another entropy
definition, a training rule, feedback, a graph-to-raster conversion, or a
CUDA path. The reviewed four-artifact plan was compiled before implementation
at `nagare/docs/plans/2026-09-06-cv-global-entropy-execution/`.

The canonical logical input is rank three, `(K,H,W)`. WGPU storage is the
private physical view `K x (H*W)`; `H` and `W` remain explicit validated
parameters. The output is `K x 3`, ordered `[mean_mass, covariance_trace,
eigen_entropy]` per channel.

## Implementation

- `KernelId::GlobalEntropyPool` closes shader source, entry point and layout
  identity through the existing pipeline cache.
- The kernel reuses `LayoutId::ActivationOutOfPlace`: one read-only storage
  buffer, one writable storage buffer and one uniform. No duplicate layout was
  introduced.
- `GlobalEntropyPoolBuilder::prepare` validates the physical storage against
  `(K,H,W)`, the `K x 3` destination, nonzero dimensions, u32 narrowing,
  workgroup limits and input/output non-aliasing before dispatch.
- One invocation owns one channel and performs the same row-major f32
  reductions and the same `EPS = 1e-6` degeneracy contract as the CPU owner.
- The dispatch is prepared once and can be encoded repeatedly; submit and
  readback remain explicit backend responsibilities.

## Discriminating evidence

The physical WGPU test ran on:

```text
NVIDIA GeForce RTX 3070 Laptop GPU [Vulkan] DiscreteGpu
driver NVIDIA 595.84
```

Three analytical `4 x 4` channels were evaluated together:

| channel | expected `[mean, trace, entropy]` |
|---|---|
| blank | `[0, 0, 0]`, bit-exact |
| unit impulse | `[1/16, 1e-6, 0]` |
| four corners | `[1/4, 1/2, ln(2)]` |

Acceptance was fixed before the GPU result was read:
`2e-5 + 2e-4 * abs(cpu)`. The test also asserts non-vacuity, kernel identity,
output cardinality and rejection of a missing spatial shape, inconsistent
flattened storage, wrong output shape, zero workgroup and read/write aliasing.

The first run failed before computation because the new shader used an
incorrect `__WG_X__` placeholder while the pipeline cache replaces
`{{WORKGROUP_X}}`. The shader was corrected to the existing cache contract;
no numerical bound was changed. A later clippy gate caught and removed a
test-only identity multiplication in an index expression.

## Gates

```text
cargo test -p hymeko_wgpu --test global_entropy_pool -- --nocapture   0   2 passed
cargo test -p hymeko_wgpu pipeline::tests -- --nocapture              0   5 passed
cargo clippy -p hymeko_wgpu --all-targets -- -D warnings              0
cargo check --workspace --all-targets                                 0
RUST_TEST_THREADS=1 cargo test --workspace                            0
  ordinary tests: 598 passed, 0 failed, 7 ignored
  doctests:        27 passed, 0 failed
git diff --check                                                     0
```

The full suite was deliberately single-threaded because this boundary needs a
stable physical-device gate and does not make a throughput claim.

## Claim boundary and next step

This earns only `WgpuGlobalEntropyForwardMatchesAnalyticalContract`. Full
CPU/WGPU parity through the same immutable Nagare `ExecutionPlan` is not yet
claimed; that is the downstream consumer boundary. Training, entropy feedback,
vision accuracy and performance remain unmeasured.

The next step is to pin both Nagare's `hymeko_core` and `hymeko_wgpu` sources to
this one commit, then compile a typed `global_entropy_pool` HyMeKo fixture into
an immutable plan and compare that plan against the existing CPU owner and this
WGPU kernel.
