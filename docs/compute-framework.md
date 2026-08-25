# HyMeKo compute framework — Phase 0 audit and migration plan

**Created-at:** 2026-08-24 05:41 CEST
**Approval:** `APPROVED-ARCHITECTURE-EVOLUTION: hymeko-compute-framework-wgpu`
**Status of this document:** Phase 0 only. **No framework code has been written.**
Everything below is either *measured current state* or *proposed*, and each
section says which.

> **Phase 1 is ready to start, pending one authorisation.** The audit first
> appeared to find a hard blocker — `wgpu` unbuildable — but diagnosis showed
> the underlying claim (crates.io unreachable) is **stale and false**. See §7:
> cargo has full registry access, and the wgpu tree vendors in 4.6 s. The one
> outstanding item is permission to write 64 new crates into the **shared**
> `hymeko_framework/vendor/` tree, which lives outside this workspace and is
> used by five sibling repositories.

---

## 1. Checkpoint (measured)

| Item | State |
|---|---|
| Branch / HEAD | `main` @ `45b0512` |
| Uncommitted work | 22 modified files, 14 new paths — **preserved, nothing reset** |
| Diff size | +1746 / −400 lines |
| Backup taken | `scratchpad/phase0_backup/core-worktree-20260824.zip` (0.43 MiB, sources only) |
| Unit tests | **17 / 17 pass** |
| Integration tests | **146 total — 145 pass** |
| Known fixture drift | `test_load_meta_geometry` `left: 58, right: 57` — **untouched** |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |

The uncommitted CPU optimisation work (documented in
`reports/2026-08-24-cpu-optimization.md`) is intact and remains the performance
baseline any framework layer must not regress.

### 1.1 Formatting-only changes in the diff

Nine files in the diff are `cargo fmt` output only, with no semantic change,
verified with `git diff --ignore-all-space`: `tensor/mesh_nn/mod.rs`,
`tensor/mod.rs`, `tensor/tensor.rs`,
`tensor/representations/tensor_csr_representations.rs`,
`traversal/{decltreeview,graph_traversal,graphview,mod}.rs`. Declared here so a
future reader does not mistake them for logic changes.

---

## 2. Current architecture (measured)

```
                    parser (external git dep)
                          |
                          v
   hymeko_core            IR  ──►  interning / resolution / hashing
   (lockdown: full)       |
                          v
                    tensor primitives
                    ├── TensorCsr<F>   row_ptr / col_ind / val
                    ├── TensorCoo<F>   (k,i,j,v) entries
                    ├── Real, IncVal, EdgeWeight   (numeric contracts)
                    └── conv::traits   compute_degree, inv_sqrt_degree
                          |
   hymeko_hnn             v
   (non-core)       HyperGraphView<V,EW,F>
                    ├── edge-first CSR   edge_offsets / flat_edge_*
                    └── node-first CSR   node_offsets / flat_node_*
                          |
                          v
                    message_passing          conv
                    ├── gather               ├── HgnnLayer
                    ├── scatter              ├── SignedHgnnLayer
                    ├── clique_diag          └── GcnCliqueLayer
                    └── implicit_clique_step
```

There is currently **no backend abstraction**. Execution strategy is chosen by
size thresholds inside each kernel — a measured, working design that this
migration must preserve, not replace.

---

## 3. Component classification (the §3 deliverable)

### 3.1 Data types

| Type | Owns | Backend-independent? |
|---|---|---|
| `TensorCsr<F>` | `row_ptr`, `col_ind`, `val` (3 `Vec`s) | **Yes** — pure data, no handles |
| `TensorCoo<F>` | `entries: Vec<CooEntry<F>>` | **Yes** |
| `HyperGraphView<V,EW,F>` | both incidence orientations + decls + edge weights | **Yes** |
| `HgnnLayer<F>` | `theta`, `edge_weights`, `inv_sqrt_dv`, `inv_de` | **Yes** (parameters are plain `Vec<F>`) |
| `SignedHgnnLayer<F>` | dual-channel parameters | **Yes** |
| `GcnCliqueLayer<F>` | `weights` + precomputed `norm_adj: TensorCsr<F>` | **Yes** |

**No type currently holds a device handle.** That is the property the design must
protect: per the task rule *"Ne tegyél GPU handle-t a core tensorstruktúrákba."*

### 3.2 Resource ownership

- **All kernel I/O is caller-owned borrowed slices** (`&[F]` / `&mut [F]`). No
  kernel allocates its output except the deliberate `spmv` / `spmm` wrappers.
- **Scratch is per-call and bounded**: `HgnnLayer::forward_pass` allocates 2
  buffers (`z`, edge scratch), `SignedHgnnLayer` 4. Bounded by
  `FEATURE_TILE_BUDGET` (~4 MiB f32).
- **Construction** (`from_ir`) allocates 15–18 times total, independent of
  incidence count (post `extract-ref-weight-noalloc`).

This ownership model is *already* the right shape for a device backend: explicit
output buffers, no hidden allocation, no ownership transfer. That was the
conclusion of `docs/backend-abstraction.md` §4 and it still holds.

### 3.3 Kernel primitives (the real unit of backend dispatch)

| Primitive | Location | Output indexed by | Parallel over |
|---|---|---|---|
| `row_dot` | `tensor_csr.rs` | row | rows |
| `row_axpy` | `tensor_csr.rs` | (row, feature) | row blocks |
| `edge_accumulate` | `message_passing.rs` | edge | edges |
| `node_accumulate` | `message_passing.rs` | node | nodes |
| `node_square_sum` | `message_passing.rs` | node | nodes |
| `transform_row` | `hgnn.rs` / `signed_hgnn.rs` | (node, feature) | node rows |
| `gather_tile` / `scale_tile` / `scatter_tile` | conv | (edge/node, feature tile) | edges / nodes |

Every one of these produces a **disjoint output partition**, which is why the
CPU parallelisation needed no atomics — and is exactly the property a GPU
kernel needs. This is the single most important finding for the port: **the
kernels are already in GPU-shaped form.**

### 3.4 Execution strategy (CPU-specific — stays put)

| Element | Value | Why CPU-specific |
|---|---|---|
| `PAR_SPMV_NNZ_THRESHOLD` | 81 920 | measured Rayon fork/join crossover |
| `PAR_SPMM_WORK_THRESHOLD` | 262 144 | measured; SpMM vectorises better |
| `PAR_INCIDENCE_THRESHOLD` | 32 768 | measured gather/scatter crossover |
| `PAR_BLOCKED_WORK_THRESHOLD` | 262 144 | measured dense-transform crossover |
| `FEATURE_TILE_BUDGET` | 1 << 20 | CPU cache-size driven |
| `row_chunk` / `par_chunk` | `len / (threads × 8)` | Rayon work-stealing granularity |
| edge-major small-input scatter | — | CPU latency optimisation (node-major is 0.71× at 1 thread) |

**None of these transfer to a GPU backend.** A GPU needs its own dispatch
policy (workgroup size, occupancy), which is why the plan keeps strategy
selection per-backend rather than unified.

### 3.5 Allocation boundaries

| Boundary | Current |
|---|---|
| `spmv` / `spmm` | allocate output (convenience wrappers) |
| `*_into` family | **zero allocation** |
| conv forwards | 2 / 4 allocations per call, bounded |
| `from_ir` | 15–18 total |

A GPU backend adds two new boundaries the CPU does not have — **upload** and
**readback** — which is why the benchmark plan (§6) measures them separately.

### 3.6 Backend-independent vs CPU-specific — summary

**Backend-independent** (can be described by a descriptor and implemented per
backend): operation *semantics* (SpMV, SpMM, gather, scatter, clique_diag,
HGNN/Signed-HGNN forward), tensor shapes and dtypes, CSR/incidence layout, the
numeric contracts (`Real`, `IncVal`, `EdgeWeight`).

**CPU-specific** (stays as the CPU backend implementation): everything in §3.4,
plus the `_serial` / `_parallel` bodies and the Rayon partitioning.

---

## 4. Crate structure — recommendation

The task asks whether a separate `hymeko_compute` crate is warranted, and states
that increasing crate count is not a goal in itself.

**Recommendation: do NOT create `hymeko_compute` yet.** Use:

```
hymeko_core                     (unchanged, lockdown: full, no wgpu)
  └── tensor::compute           NEW, tiny, dependency-free
        ├── ComputeError
        ├── TensorDescriptor / TensorShape
        ├── DeviceCapabilities
        └── ComputeBackend trait (generic, static dispatch)

hymeko_hnn                      (unchanged, no wgpu)
  └── implements the CPU backend over its existing kernels

hymeko_wgpu                     NEW crate — depends on hymeko_core only
        ├── WgpuBackend + builder
        ├── GpuTensor<f32>, GpuCsrMatrix<f32>
        ├── PipelineCache
        └── shaders/*.wgsl
```

Rationale, against the alternative of a third crate now:

- The backend-neutral vocabulary is **small** (4 items) and is naturally owned
  by whoever owns `TensorCsr` — that is `hymeko_core`.
- A separate `hymeko_compute` would force `TensorDescriptor` to reference
  `TensorCsr`, giving `compute → core`, then `hnn → compute` and
  `wgpu → compute` — three edges to express what one module in `core` expresses
  with zero.
- There is currently **one** prospective non-CPU backend. A dedicated runtime
  crate earns its place when backend *selection* becomes non-trivial, i.e. at
  Phase 5 with ≥3 backends. Revisit then.

**Caveat requiring approval:** `hymeko_core` is `lockdown: full`. Adding
`tensor::compute` to it is a core edit needing its own `APPROVED-CORE-EDIT`
token, even though it adds no dependency. If that is unwelcome, the fallback is
to put the vocabulary in `hymeko_hnn` and have `hymeko_wgpu` depend on `hnn` —
workable, but it drags the hypergraph layer into a GPU crate that does not need
it. Flagging the trade rather than choosing unilaterally.

---

## 5. Migration classification (the §5 deliverable)

### 5.1 Move (relocate existing code, no logic change)

**Nothing.** No file needs to move. This is deliberate: the task forbids copying
algorithms into new modules, and the existing layout already separates
backend-independent data (`core`) from algorithms (`hnn`).

### 5.2 Adapt (wrap existing code — no algorithm rewritten)

| Item | Adaptation |
|---|---|
| `TensorCsr::spmm_into_serial` | becomes the body of `CpuSerialBackend::spmm` |
| `TensorCsr::spmm_into_parallel` | becomes the body of `CpuRayonBackend::spmm` |
| existing thresholds | become the `CpuAuto` dispatch policy, unchanged values |
| `TensorCsr<f32>` | gains an **adapter** (in `hymeko_wgpu`) that uploads to `GpuCsrMatrix<f32>` — the core type is untouched |
| `HyperGraphView` | same: upload adapter lives in the GPU crate |

The `*_into` functions **stay** as the low-level allocation-free primitives, are
**not renamed**, and remain the CPU implementation the structured operations
delegate to — exactly as the task specifies.

### 5.3 Genuinely new

- `ComputeError`, `TensorDescriptor`, `TensorShape`, `DeviceCapabilities`
- `WgpuBackend` + `WgpuBackendBuilder`
- `GpuTensor<f32>`, `GpuCsrMatrix<f32>`
- `PipelineCache`
- `spmm.wgsl`
- `SpmmBuilder` / `PreparedSpmm`, `encode` / `submit` / `readback`

### 5.4 Deferred (explicitly not now)

| Item | Gate before it is justified |
|---|---|
| `ComputeGraph` | ≥ 2 working GPU operations (task rule) |
| Runtime `Auto` backend policy | measured CPU/GPU crossover |
| Resource repository / upload cache | a demonstrated repeat-upload workload |
| Generic erased `Tensor` type | never in round 1 — f32 GPU path stays explicit |
| CUDA / FPGA backends | a second backend must exist and be measured first |
| `hymeko_compute` crate | ≥ 3 backends |

---

## 6. Compatibility and benchmark strategy (required before implementation)

### 6.1 Compatibility contract

1. **No existing public signature changes.** `spmv`, `spmm`, `*_into*`,
   `gather_*`, `scatter_*`, `clique_diag*`, `implicit_clique_step`,
   `forward_pass`, `forward_relu` all keep their signatures.
2. **The CPU numerical guarantee is unchanged**: serial ≡ parallel **bitwise**,
   asserted by the existing 31 regression tests. The framework layer must not
   weaken any of those assertions.
3. **GPU is explicitly NOT bitwise-comparable** to CPU. GPU tests use absolute +
   relative tolerance with explicit NaN/Inf handling. The existing bitwise tier
   stays; a tolerance tier is added *beside* it, never replacing it.
4. **The 57/58 fixture drift is untouched.**

### 6.2 CPU no-regression protocol

The framework layer's own overhead is the risk. Protocol:

1. Re-run `csr_bench` / `hnn_bench` against the saved `after` baselines before
   and after each phase (`scripts/dev/criterion_report.ps1 -Baseline after -Compare <new>`).
2. Add one dedicated cell — `csr_spmm/framework/<scale>` — calling SpMM through
   the structured operation, benchmarked against `csr_spmm/alloc/<scale>` which
   calls `spmm_into` directly. **Acceptance: the structured path must be within
   noise of the direct call.** Any measurable gap means the abstraction is
   costing something and must be redesigned, per the task's performance rules.
3. The §3 10 % regression gate applies to every previously measured cell.

### 6.3 GPU benchmark decomposition

Measured separately, never merged into one number:

| Cell | Contents |
|---|---|
| `cpu_serial` | `spmm_into_serial` |
| `cpu_rayon` | `spmm_into_parallel` |
| `gpu_oneshot` | upload + dispatch + readback |
| `gpu_resident` | encode + submit + wait, CSR and input already on device |
| `gpu_resident_readback` | resident + readback |
| `pipeline_creation` | separate group — never inside a kernel cell |

Sweep: small / medium / large × `feature_dim` ∈ {1, 8, 32, 128}, warm-up first,
≥ 3 separate processes, adapter name + backend (DX12 or Vulkan) + workgroup
size + buffer sizes recorded, crossover point reported.

### 6.4 Measurement-integrity carry-over

`reports/2026-08-24-cpu-optimization.md` §7 documented a ~20 % run-to-run band on
small cells on this host under sustained load. GPU benchmarking will heat the
machine further. Small-cell GPU/CPU comparisons must therefore be run
back-to-back, and any conclusion inside that band reported as unresolved rather
than asserted.

---

## 7. Dependency acquisition — investigated, resolved

Phase 0's most important finding, and a caution about inherited assumptions.

### 7.1 The apparent blocker

`cargo add wgpu -p hymeko_hnn --dry-run` inside the workspace fails with:

```
error: the crate `wgpu` could not be found in registry index
```

`hymeko_framework/.cargo/config.toml` redirects the registry to a vendor
directory, and carries this comment:

> *"crates.io is unreachable from this machine's build tooling (cargo.exe/curl.exe
> time out while powershell.exe reaches the net fine), so the registry is
> replaced by a pre-populated ./vendor directory."*

The `vendor/` tree holds 289 crates / 417 MB and contains **no** `wgpu`, `naga`,
`ash`, `gpu-allocator`, `d3d12`, or `windows` graphics stack. Taken at face
value, that is a hard stop.

### 7.2 The claim was stale — measured

The comment is a statement about the network, so it was tested rather than
believed:

| Check | Result |
|---|---|
| `netsh winhttp show proxy` | direct access, no proxy |
| `HTTP(S)_PROXY`, WinINET proxy settings | all unset / disabled |
| `curl.exe https://static.crates.io/...` | **HTTP 403 in 5.5 s** — a *response*, not a timeout |
| same, forced IPv4 (`curl -4`) | HTTP 403 in **0.59 s** — 9× faster |
| `Resolve-DnsName static.crates.io` | returns **AAAA records only** in the first answers |
| `cargo add wgpu --dry-run`, outside the vendored config | **succeeded** — *"Updating crates.io index… Adding wgpu v30.0.1"*, features `dx12 gles metal parking_lot std vulkan web webgpu wgsl` |
| `cargo fetch` (full wgpu tree) | **exit 0 in 2.8 s** — downloaded `ash`, `naga`, `windows`, `web-sys`, `khronos_api` |
| `cargo vendor --versioned-dirs` | **exit 0 in 4.6 s**, 123 crates, 204.1 MiB |

**Cargo has full crates.io access.** What blocks `wgpu` is solely the
`replace-with = "vendored-sources"` redirection plus an incomplete vendor tree —
not connectivity.

The residual truth in the old comment is the IPv6 preference: DNS answers AAAA
first and the v6 path stalls, so tools that try v6 first pay a multi-second
penalty before falling back. That is a latency tax, not unreachability, and it
plausibly *was* a timeout under whatever conditions produced the original note.

**Lesson recorded deliberately:** an inherited comment asserting an environment
fact is a hypothesis with a timestamp, not a constant. Four commands falsified
it. Had it been trusted, this phase would have concluded "GPU work is
impossible here" and proposed hand-vendoring ~100 crates — a large amount of
wasted effort premised on a stale sentence.

### 7.3 What vendoring actually costs (measured)

Vendored from a scratch project that does *not* inherit the workspace config:

| Metric | Value |
|---|---|
| Crates in the wgpu tree | 123 |
| Already present in the shared vendor | 59 |
| **New crates to add** | **64** |
| **New content** | **154.5 MiB** |
| Wall time to vendor | 4.6 s |
| Largest additions | `windows-0.62.2` (109 MB), `khronos_api` (6.3), `ash` (4.9), `naga` (4.6) |

Much of the tail is platform variants that this host never compiles
(`objc2-*`, `ndk-sys`, `glow`, `web-sys`). `cargo vendor` has no platform
filter; carrying them is normal and harmless.

The prepared tree already exists at
`scratchpad/nettest/vendor-out/` and can be copied in directly.

### 7.4 The one outstanding authorisation

`hymeko_framework/vendor/` sits **outside this workspace**
(`hymeko_framework/core/`) and is shared by at least five sibling repositories
(`graph`, `network`, `parser`, `transformation`, `hotaru`). Writing into it is
therefore a change to shared infrastructure, which is why Phase 0 stops here
rather than proceeding unilaterally.

The proposed operation is **strictly additive**:

- copy only the **64 directories that do not already exist**;
- **modify no existing entry**, so no sibling repo's dependency resolution can
  change;
- reversal is `Remove-Item` on those 64 directories.

Two alternatives, both worse:

- **Regenerate the whole vendor tree** with `cargo vendor` from the core
  workspace — rejected: it rewrites all 289 entries and can drop crates that
  only the sibling repos depend on.
- **Drop the vendored-sources redirection** and let cargo fetch normally —
  rejected for now: it changes build behaviour for all five repos and discards
  the offline-reproducibility the vendor tree provides.

**Recommendation:** additive copy of the 64 new crates, leaving the
`vendored-sources` architecture and every existing entry untouched.

---

## 8. What is implemented vs planned

| Item | Status |
|---|---|
| Phase 0 audit, component map, classification | **done** |
| Backup of uncommitted work | **done** |
| CPU baseline verification | **done** |
| Compatibility + benchmark strategy | **done** (§6) |
| Migration plan | **done** (§5) |
| `wgpu` feasibility | **done — resolved, not blocked** (§7) |
| Vendor extension (45 crates, additive) | **done** — see §9 |
| Core vocabulary (`hymeko_core::tensor::compute`) | **done** — 4 types, 7 tests |
| `hymeko_wgpu` Phase 1 skeleton | **done** — 8 GPU tests pass on RTX 5080 |
| Phase 2 — CSR SpMM kernel | not started |
| Phases 3–5 | not started |

Details of the WGPU crate live in `docs/wgpu-backend.md`.

---

## 9. Vendor extension — executed record

Performed under the 2026-08-24 authorisation, strictly additive.

### 9.1 Feature minimisation changed the number

The authorisation named 64 crates, measured with wgpu's **default** features.
The same instruction also required minimal features for the target platforms.
Re-resolving with `default-features = false, features = ["dx12", "vulkan",
"wgsl", "std"]` shrank the tree, so **fewer** crates were copied than authorised:

| | default features | minimal features |
|---|---:|---:|
| crates in tree | 123 | **97** |
| already vendored | 59 | 52 |
| **new crates copied** | 64 | **45** |
| **new content** | 154.5 MiB | **140.9 MiB** |

Dropped by minimisation (26 crates): the GLES stack (`glow`, `khronos_api`,
`khronos-egl`, `gl_generator`, `glutin_wgl_sys`), Apple (`block2`,
`objc2-core-graphics`, `objc2-io-surface`, `wgpu-core-deps-apple`), Android
(`ndk-sys`, `jni-sys`), WebAssembly (`wgpu-core-deps-emscripten`), and assorted
build-time helpers.

### 9.2 `windows-0.62.2` (109 MB) — verified necessary

Checked with `cargo tree -i windows --target x86_64-pc-windows-msvc` rather than
assumed:

```
windows v0.62.2
├── gpu-allocator v0.28.0
│   └── wgpu-hal v30.0.1
└── wgpu-hal v30.0.1
```

It is a genuine transitive dependency of the DX12 path and of the memory
allocator, reached two independent ways. It was **not** pulled in by an
unnecessary feature and is therefore retained, as instructed.

### 9.3 Safety verification

| Check | Result |
|---|---|
| Destination collisions before copy | **0 of 45** |
| `.cargo-checksum.json` present in every new crate | **45 / 45** |
| Pre-existing entries modified | **0 of 289** (file count + byte size compared before/after) |
| Entries removed | **0** |
| Vendor entry count | 289 → **334** (+45) |
| Vendor size | 377.9 → **518.8 MiB** (+140.9) |
| `cargo metadata --offline` | exit 0 |
| `cargo check --workspace --offline` | exit 0 |
| `cargo build -p hymeko_wgpu --offline` | exit 0 |

Inventories and fingerprints: `scratchpad/phase1_checkpoint/`
(`vendor-inventory-before.txt`, `vendor-new-crates.txt`,
`vendor-fingerprint-{before,after}.csv`).

### 9.4 One incident, contained

The first copy attempt used `Copy-Item` and hit the Windows `MAX_PATH` limit on
a deeply nested test-snapshot file in `codespan-reporting-0.13.1`, leaving that
crate **partially copied** (69 of 115 files) — a corrupt vendor entry.

It was detected by comparing per-crate file counts against the source,
removed (it was not pre-existing, so removal restored the prior state exactly),
and the copy was redone with `robocopy`, which handles long paths natively. The
final verification confirms 45/45 complete and 289/289 untouched.

Recorded because a partially-copied vendor entry is exactly the kind of failure
that would surface much later as an inexplicable build error.

---

## 10. Phase 1 CPU regression check — measured

The framework layer must not cost the CPU path anything. Phase 1 adds a module
to `hymeko_core` and a sibling crate, neither referenced by any CPU kernel, so
the expectation was "no change" — but the §3 gate requires measurement, not
expectation.

### 10.1 First result: two cells flagged

| Cell | `after` baseline | Phase 1 | delta |
|---|---:|---:|---:|
| `csr_spmv/alloc/medium` | 93.49 µs | 118.22 µs | **+26.5 %** |
| `csr_spmm/alloc/medium/fd1` | 140.34 µs | 164.74 µs | **+17.4 %** |

Both past the 10 % gate. Default attribution is "a bug was introduced".

### 10.2 Repetition showed the measurement was unstable

Three consecutive runs of **identical code**:

| run | `csr_spmv/alloc/medium` | `csr_spmm/alloc/medium/fd1` |
|---|---:|---:|
| 1 | 122.82 µs | 182.93 µs |
| 2 | 102.30 µs | 165.81 µs |
| 3 | 123.09 µs | 187.85 µs |

Criterion reported −16.95 % then +21.84 % *between repeats of the same binary*.

### 10.3 Cause: the machine was not quiet

`Get-CimInstance Win32_Processor` reported **31 % background CPU load** —
Chrome (three processes), VS Code, Task Manager, `StartMenuExperienceHost`,
`DCv2`, `iGoSwServer`. No stale benchmark or cargo processes of this session
were present (CLAUDE.md §6.5 #17 checked first).

CLAUDE.md §3 is explicit: *"Run on a quiet machine. Background CPU contention
invalidates the run."* The comparison against a baseline taken under different
ambient conditions is therefore **not a valid regression measurement**.

### 10.4 Discriminating A/B — the module is innocent

"The machine was busy" explains the noise but does not prove the code is
harmless. A direct A/B was run **back-to-back under the same ambient load**,
toggling only `pub mod compute;` in `hymeko_core/src/tensor/mod.rs`:

| variant | `csr_spmv/alloc/medium` |
|---|---:|
| **A** — with `tensor::compute` | 100.01 µs |
| **B** — without it | 104.84 µs |
| **A2** — with it again | 104.70 µs |

A2 and B differ by **0.1 %**; A and A2 — the *same* configuration — differ by
**4.7 %**. The between-configuration difference is an order of magnitude smaller
than the within-configuration spread, so the module has **no measurable effect
on CPU codegen**.

**Verdict: no Phase 1 CPU regression.** The absolute level (~100–105 µs against
a 93.5 µs baseline) reflects the loaded machine, not the change.

**Consequence for Phase 2:** GPU/CPU crossover measurement must be taken on a
quiet machine, with background load checked and recorded before the run. A
crossover point derived under 31 % ambient load would be meaningless. This is
now a precondition in the benchmark protocol (§6.3), not a hope.

---

## 11. Phase 2 status (historical)

| Item | Status |
|---|---|
| CSR SpMM WGSL kernel | **done** — `hymeko_wgpu/shaders/spmm.wgsl` |
| `OperationFactory` / `SpmmBuilder` / `PreparedSpmm` | **done** |
| `PipelineKey` widened to 2D workgroups | **done** — minimal Phase 1 extension, see `docs/wgpu-backend.md` §5a.4 |
| `upload_csr_from(&TensorCsr)` adapter | **done** |
| Correctness suite (18 required cases) | **done** — 13 tests, all pass on RTX 5080 / Vulkan |
| Benchmark cells A–F + workgroup sweep + prepare | **done** |
| Kernel-only timestamp (cell G) | capability **granted** (period 1 ns); reported, not folded into wall-clock cells |
| Second host (amaterasu) reproduction | **blocked** — see §12 |

Operation detail lives in `docs/wgpu-backend.md` §5a.

---

## 12. Second benchmark host (amaterasu) — access findings

Read-only discovery only. No SSH configuration was changed on the remote, no
package installed, no foreign process touched.

### 12.1 What was established

| Probe | Result |
|---|---|
| `amaterasu.local` DNS | resolves (AAAA, IPv6 only in the first answers) |
| TCP :22 | open |
| TCP :2222 | open |
| `git ls-remote ssh://git@amaterasu.local:2222/hymeko/parser.git` | **succeeds** — key authenticates, `HEAD = 284c490d` |
| `ssh -p 2222 git@… "uname -a"` | rejected: `error: Invalid repository path` |
| `ssh -p 22 kyber@…` | `Permission denied (publickey,password)` |
| key offered on :22 | `id_ed25519`, `SHA256:Op1S8VGJ5w1m+9SoqzrUGxdlfu0wNn0TR8Wcphzv3TU` |

**Conclusion: git access yes, interactive shell no.** Port 2222 runs a
git-only shell (Gitea/Forgejo style) that refuses arbitrary commands, and port
22 does not accept the available key for user `kyber`. Password authentication
is offered but was deliberately not attempted — the brief forbids guessing
credentials.

### 12.2 What this blocks

Everything the remote protocol asks for needs command execution:
hardware discovery (`uname`, `lscpu`, `free`, `nvidia-smi`, `vulkaninfo`),
toolchain probing (`rustc -Vv`, `cargo -V`), `cargo check`/`build`, the
correctness suite, and every benchmark cell. None of it is reachable over a
git-only shell.

Per the brief — *"Ha nincs már működő interaktív SSH-hozzaferes, dokumentald
pontosan a hiányzó adatot és folytasd a lokális implementációt"* — the remote
comparison is deferred and local work continued.

### 12.3 What would unblock it

Any **one** of:

1. authorise `SHA256:Op1S8VGJ5w1m+9SoqzrUGxdlfu0wNn0TR8Wcphzv3TU` for a
   shell-capable account on `amaterasu.local:22`;
2. supply the username whose key is already authorised there;
3. supply a different host/port that offers an interactive shell.

Nothing else is missing: the transfer protocol is ready. The source manifest
generator (`scripts/dev/source_manifest.ps1`) already produces the deterministic
hash both hosts must cite, so a remote run can be made comparable the moment a
shell exists.

**Current manifest hash:** `92420f3292b5a524c7253d9a5980f1b07adbc7d966aa84d3659b0e5b8c37afcc`
(155 files, 850 645 bytes).

### 12.4 One local side effect, disclosed

Probing port 22 with `StrictHostKeyChecking=accept-new` appended
`amaterasu.local` (ED25519) to the **local** `~/.ssh/known_hosts`. That is a
local trust-store entry, not a change to any SSH configuration and not a remote
change, but it is a file this session modified outside the workspace and is
recorded here rather than left silent.

---

## 13. Phase 2 CPU regression check

Phase 2 added a GPU crate and one WGSL kernel; it modified **no CPU code**.
Verified structurally before measuring:

- `PAR_SPMV_NNZ_THRESHOLD` = 81 920, `PAR_SPMM_WORK_THRESHOLD` = 262 144,
  `PAR_INCIDENCE_THRESHOLD` = 32 768, `PAR_BLOCKED_WORK_THRESHOLD` = 262 144,
  `FEATURE_TILE_BUDGET` = 1 << 20 — all unchanged;
- no file under `hymeko_core/src/tensor/representations/`,
  `hymeko_hnn/src/tensor/` or `hymeko_hnn/src/traversal/` is in the Phase 2 diff;
- the CPU bitwise tier (serial ≡ parallel) still passes in full.

### 13.1 First pass flagged six cells

| cell | `after` baseline | Phase 2 run | delta |
|---|---:|---:|---:|
| `csr_spmm/alloc/small/fd128` | 98.51 µs | 128.97 µs | +30.9 % |
| `csr_spmm/alloc/small/fd8` | 12.30 µs | 14.55 µs | +18.4 % |
| `csr_spmm/alloc/small/fd32` | 28.94 µs | 33.94 µs | +17.3 % |
| `csr_spmm/alloc/small/fd1` | 8.66 µs | 10.16 µs | +17.3 % |
| `csr_spmm/alloc/large/fd32` | 7.949 ms | 9.133 ms | +14.9 % |
| `csr_spmm/alloc/large/fd1` | 996.9 µs | 1.106 ms | +11.0 % |

### 13.2 Repetition disproves the regression

Three further runs of the same binary:

| cell | baseline | flagged | repeat 1 | repeat 2 | repeat 3 | mean vs baseline |
|---|---:|---:|---:|---:|---:|---:|
| `small/fd32` | 28.94 µs | 33.94 µs | 24.39 | 25.96 | 25.80 | **−12 % (faster)** |
| `small/fd128` | 98.51 µs | 128.97 µs | 108.36 | 118.34 | 105.57 | +12 % |

**Identical code moved in opposite directions on two cells of the same
benchmark.** A real regression moves consistently; ambient variance does not.
Both flagged values fall outside the range of the three repeats, i.e. that run
was the outlier. Combined with the structural evidence above — no CPU file
touched, no threshold changed — there is **no Phase 2 CPU regression**.

This is the second time this host has produced a double-digit phantom
regression (Phase 1 §10). The pattern is now well established and the protocol
for it is fixed: repeat before attributing.

## 14. Phase 3 status (historical) — resident activation chain

> Phases 4 and 5 are documented in `docs/wgpu-backend.md` sections 5d and 5e,
> and in the reports for those phases. This document was not extended past
> Phase 3; the sections below are kept as the record of their phase.

### 14.1 What Phase 3 was for

Phase 2 proved one GPU operation works. Phase 3 answers the question that
actually decides whether a GPU backend is worth having: **does a second
operation run on the first one's output without a host round trip?**

If every operation must read back, the PCIe transfer dominates and the whole
layered design collapses into "a slower CPU with extra steps". The deliverable
is therefore not the activation kernel — it is this:

```rust
let mut enc = backend.encoder("chain");
spmm.encode(&mut enc);      // Y = A × X
act.encode(&mut enc);       // Y = f(Y)
backend.submit(enc);        // ONE submit, ONE encoder
```

Asserted by `test_spmm_then_activation_one_encoder` and
`test_two_chains_one_submit`, and measured as benchmark cells D–G.

### 14.2 Scope discipline — what was deliberately not built

| not built | why |
|---|---|
| general `ComputeGraph` | out of scope by the phase brief; the decision document (`docs/compute-graph-decision.md`) recommends a **buffer arena**, not a graph, and states the three conditions that would change that |
| runtime shader-string generator | kinds are entry points in one module; the only substitutions are the same two workgroup constants every kernel uses |
| fused SpMM+activation as the default | cell H exists as a measured comparison only; fusion is a code-generation problem and stays hand-written and explicitly selected |
| any change to a CPU file | verified by modification time: every file touched this phase is under `hymeko_wgpu/` |
| `ActivationKind` in `hymeko_core` | the core-vocabulary rule requires a concrete CPU *and* GPU consumer. The CPU layers hard-code their slopes and take no activation parameter, so there is no CPU consumer to share with — promoting it would be an abstraction with one user |

### 14.3 The one genuine design decision: in-place

In-place was investigated, as the brief required, and it **is** achievable —
but not the obvious way. Binding one buffer as both the read and the read_write
binding of the out-of-place shader violates WebGPU's usage-scope rule: it is a
validation error, not a portability grey area.

The resolution was to remove the aliasing rather than work around it: a separate
module with a **single `read_write` binding**. No unsafe, no backend-specific
escape hatch — both of which the brief explicitly forbade as workarounds.

The cost is one duplicated line per kind across two WGSL files, bounded and
covered by `test_in_place_and_out_of_place_agree`, so divergence fails a test.

### 14.4 A device limit that a real workload crosses

The activation dispatch is 1-D, so a 256-wide workgroup addresses at most
65 535 × 256 = 16.7M elements. The large workload at 128 features is 25.6M
elements — **inside the intended range, past the hardware ceiling.**

`prepare()` caught it as `IndexOverflow` on the first benchmark run rather than
producing truncated output, which is the validation behaving as designed. The
fix is a grid-stride loop (the "several elements per invocation" case the brief
permits), striding by the whole grid so coalescing survives, and costing nothing
when the grid already fits. See `docs/wgpu-backend.md` 5b.6.

The lesson generalises: a limit a legitimate workload crosses should be
accommodated, not merely reported.
