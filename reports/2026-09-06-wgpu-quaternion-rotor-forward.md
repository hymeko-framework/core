# WGPU quaternion-attention and rotor-spike forward boundary

Date: 2026-09-06

Repository: `hymeko/core`

Entry commit: `43cdf45253a138c1595b0f384c28ee26630e523d`

## Outcome

The existing `hymeko_wgpu::OperationFactory` now prepares two additional
forward-only operations:

- sparse, single-head, untransported quaternion rotor attention over a
  validated CSR topology; and
- star-centred rotor-spike orientation population coding.

Both are WGPU execution owners. They do not define model semantics, accept a
HyMeKo document, or create an alternative IR. Their public builders require
explicit shapes and already materialised buffers; Nagare remains responsible
for lowering canonical `hymeko_core::Ir` into an immutable execution plan.

`rotor_spike` is a static orientation population code. This boundary does not
classify it as a temporal spiking neural network.

## Scope and ownership

The implementation extends the existing closed kernel/layout identities and
the existing operation factory. It adds no backend, dependency, runtime
selector, graph representation, parser, optimizer, or training path.

The attention contract is deliberately narrow: four-dimensional quaternion
tokens, one head, untransported values, one preferred rotor per token, and a
validated square CSR adjacency. The star rotor-spike contract takes node
features, star centres and a fixed-width membership table, and emits every
orientation bin. Projection indices must be distinct and in range.

## Verification

The focused `quaternion_rotor` integration test runs on the available NVIDIA
RTX 3070 Laptop GPU through the Vulkan WGPU backend and compares every output
element with an independent small analytical reference.

The tests cover:

- sparse quaternion attention, including an empty CSR row;
- quaternion double-cover invariance under preferred-rotor sign reversal;
- the complete star rotor-spike output tensor; and
- malformed CSR and invalid-shape rejection.

The first shader compilation failed before numerical comparison for two Naga
compatibility defects introduced in this boundary: `partition` was a reserved
WGSL identifier, and a pointer to a storage runtime array could not be passed
through a helper function. The shader now uses a non-reserved identifier and
separate global-buffer load helpers. No tolerance was changed in response.

## Gates and real exits

| Gate | Result |
|---|---:|
| `cargo check -p hymeko_wgpu --all-targets` | exit 0 |
| `cargo test -p hymeko_wgpu --test quaternion_rotor` | 3 passed, 0 failed |
| `cargo test -p hymeko_wgpu --lib` | 52 passed, 0 failed |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |
| `git diff --check` | exit 0 |
| `RUST_TEST_THREADS=1 cargo test --workspace` | 595 passed, 0 failed, 7 ignored; doc tests 27 passed |

An initial parallel `cargo test --workspace` run terminated the pre-existing
`activation` test process with `SIGSEGV` after 12 tests. No new test had run in
that process. `RUST_TEST_THREADS=1 cargo test -p hymeko_wgpu --test activation`
then passed 19/19, and the complete serial workspace run passed. This is
classified as a likely concurrent GPU-driver/test-process instability, not as
a verified defect in either new kernel. The boundary is therefore certified
under the explicitly recorded serial GPU-test contract; the parallel-process
failure remains visible here rather than being reported as an all-green first
run.

## Claim boundary

This boundary establishes operation-level WGPU correctness only on the tested
shapes. It makes no end-to-end Nagare claim, no speed claim, no CUDA claim, no
training claim, and no real-dataset claim. Those require the canonical
IR-bound Nagare Slice 3 and its separately predeclared measurements.
