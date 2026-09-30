# HyMeKo kernel ownership — one capability, one owner

**Status:** 2026-08-30, derived from the cross-repository I0 audit
([`reports/2026-08-30-hnn-nagare-tensor-integration-audit.md`](../reports/2026-08-30-hnn-nagare-tensor-integration-audit.md)).
This is the **normative ownership map**: every mathematical, traversal, and
message-passing capability in the framework has exactly one production owner; every
other layer must reach it by dependency, adapter, or composition — never by copy.

Verified against canonical `origin/main`: core `e2952c7`, graph `5078980`, tensor
`8d0e546`.

## Ownership table

### Tensor extraction, authorized 2026-09-23

The user requested that generic tensor storage and arithmetic move to the existing
`tensor/hymeko_tensor` owner, leaving only canonical-IR adapters in Core.
The table below reflects the new numeric ownership; unrelated audit findings retain their original scope.

Migration sequence and artifact ledger:

1. Move `common`, `aggregation`, `compute`, `conv`, `representations`,
   `tensor_convert`, `arrow_schema`, `sparse_workload`, and numeric display helpers
   from `hymeko_core/src/tensor` into the existing `hymeko_tensor/src` tree.
   Split `tensor_val`: scalar algebra belongs to Tensor; reference-value extraction
   belongs to Core. Move the canonical-ID `TensorInc` carrier to Core's tensor module.
2. Remove the empty aggregation placeholder, unused `TensorBuildCfg` and degree
   forwarding wrapper, and the superseded shared-memory scaffolding. Preserve the
   canonical `ir_stream` transport. No compatibility re-export or second kernel.
3. Move numeric contract tests and the existing CSR benchmark with their owner;
   retain IR-weight extraction tests in Core. Update WGPU and Nagare imports to
   the actual owner. Tensor never depends on Core, Graph or Nagare.
4. Keep heavy numeric dependencies behind Tensor features: `sparse` for Rayon
   kernels, `interop` for Nalgebra/JAX conversion, `arrow-schema` for Arrow schemas.
   The default Core library consumes only the dependency-free numeric contracts.
5. Verify moved numeric tests, IR extraction, WGPU compilation, and Nagare's HNN
   boundary/CSR tests. Preserve arithmetic order and frozen workload identities;
   run no benchmarks. Use existing checkouts, no backup copies or new worktrees.

Registry changes use the existing `ecosystem.hymeko`, README projection and
dependency scanner. The scanner must recognize `hymeko_tensor` as dependency-free
numeric foundation within the Tensor repository, without relaxing other pillar
boundaries; extend its existing tests to reject an upward Graph dependency.

Dependency publication is separate: local verification patches the pinned Tensor
dependency to the current sibling checkout. The Tensor commit must be published
and consumers pinned to that revision before publishing this Core change.
Rollback is the reviewed Git diff, preserving all earlier workspace changes.

Local implementation/verification, 2026-09-23 23:05 +02:00:

- Existing checkouts: Core `10138bfc14fc7574461edce2192f644b136dfba4`
  (`feat/ir-runtime-updates`), Tensor `5ab2c33b6d3c4b94ec7e48ccd99264d6d4495475`
  (`main`), Nagare `8732738e931cbae8a35746d8b7dbb6236ced44d9`
  (`fix/local-tensor-paths`). Fetch succeeded before edits; all changes below are
  uncommitted. Earlier dirty Core and Nagare work was preserved. The registry
  checkout fast-forwarded to `9f679c0abf01c7cf7b50607cd713a78421ccd743` before edits;
  its unrelated untracked files were preserved. No new checkout or worktree.
- Core's tensor source tree now contains only `mod.rs` and `tensor_val.rs`,
  130 lines total (HEAD: 22 files / 3269 lines, including previously retired
  decomposition). Moving code between owners is not ecosystem-wide deduplication.
  Empty source directories remain because automatic command approval rejected
  their removal with `blocked by policy`; no source files remain inside them.
- Weight initialization declarations and implementations now share one module;
  there is no `weight_init::weight_init` forwarding namespace. Scalar conversion
  traits explicitly require `Copy`, already required by `Real`. Numerical loop
  order is preserved. No aliases restore the former Core numeric API.
- Tensor: 47 unit/integration tests + 2 compile-fail doctests passed; frozen sparse
  workload digests unchanged. All-feature/all-target Clippy with `-D warnings`
  passed, including Arrow and Nalgebra adapters. Default `cargo tree --edges normal`
  shows no external production dependency.
- Core: 8 IR-weight extraction tests passed. Core and WGPU library Clippy with
  `-D warnings` passed. WGPU: 2 moved authority/source gates passed and every target
  compiled; no GPU execution or benchmark was performed.
- Nagare: `cargo test --test hnn test_tensor_representations` passed 63 tests,
  with 1 existing ignored test. Includes CSR, dense-view boundaries, convolution
  and message passing. The resolved graph contains a single local Tensor identity
  shared by Core, WGPU and Nagare. Temporary CLI patches also unify the two Core
  Git URLs and the existing parser revision; Nagare's lockfile was restored after
  this local check, not rewritten with temporary source identities.
- Ownership registry: Core/Tensor scoped manifest check passed. The complete
  provider self-check passed 184 tests, Ruff, mypy, symbol/duplication gates and
  the ecosystem parser, but failed the global dependency-map gate: the unrelated
  uncommitted `transformation/hymeko_geometry/Cargo.toml` adds `hymeko_spline`.
  That edge was not silently absorbed into this task's registry change.
- Core duplication gate passed: 458 groups / 9553 cloned lines versus its stored
  baseline 484 / 10411. This is repository-local evidence, not a claim that moving
  kernels reduced duplication across the entire ecosystem. Baselines unchanged.

The checked-in Git pin is the *pre-extraction* Tensor revision; it does not yet
contain these APIs. Thus an ordinary unpatched consumer build is not ready for
publication. The current Core lockfile records the local integration resolution.
Publish the reviewed Tensor change first, replace the Core Git pin with that
available commit, regenerate its lockfile without patches, and pin downstream
consumers. No commit, push or release was performed in this task.

From the Core checkout, the local integration check is reproducible with:

```powershell
$tensorPatch = 'patch."ssh://git@192.168.1.228:2222/hymeko/tensor.git".hymeko_tensor.path="D:/hakiko_ai_ws/03_implementation/hymeko_framework/tensor/hymeko_tensor"'
cargo check -p hymeko_core -p hymeko_wgpu --lib --offline --config $tensorPatch
```

| Capability | Owner crate (repo) | Public entry point | Contract summary | Non-owners must |
|---|---|---|---|---|
| Canonical IR | `hymeko_core` (core) | `Ir`, `try_from_ir` seams | one canonical IR; dense views are derived | build derived views, never a second IR |
| Sparse tensor formats | `hymeko_tensor` (tensor) | `TensorCoo`, `TensorCsr`, `spmv/spmm/*_into` | serial+Rayon row-disjoint bitwise-identical | call `*_into`; do not re-implement SpMM |
| Degree computation | `hymeko_tensor` (tensor) | `compute_degree_by_index`, `DegreeMode`, `inv_sqrt_degree` | generic over an index projection `P: Fn(I)->usize` | adapt via `\|d\| d.as_usize()` (hnn already does) |
| WGPU backend base | `hymeko_wgpu` (core repo), consuming `hymeko_tensor` | resident SpMM/activation | PoW-0 frozen hash `00d4c01a` | consume; no second backend |
| Hypergraph incidence operator | `hymeko_graph` (graph) | `SignedHyperIncidence::{node_to_edge, edge_to_node, star_matvec, assemble_clique}` | uniform-arity, f32, edge-major, allocating, serial, forward+adjoint | see **note A** (overlap with hnn message passing) |
| CSR + BFS/DFS traversal | `hymeko_graph` | `traversal::{Csr, dfs_visit, bfs_distances, Dfs/BfsScratch}` | u32 CSR, caller-owned scratch, callback, depth-bounded | borrow the adjacency; do not re-implement (see **note B**) |
| Top-k cycle enumeration | `hymeko_graph` | `enumerate_top_k_*`, scorers, `TopKCyclesBatch` | SoA batch, canonical-form dedup | consume the batch |
| Signed-cycle FIR spine | `hymeko_graph::spine` | `clifford_fir_forward/backward`, `CliffordFIR`, `SignedCycleFIR`, `GradRequest`, `FirBackward` | f32, Rayon-forward deterministic, closed-form Clifford backward, golden digests | depend on it (Nagare must re-point off its stale fork — **note C**) |
| Line-graph edge→cycle incidence | `hymeko_graph::incidence` | `build_edge_incidence` → `IncidenceCoo/Csr` | `1/\|N\|` values, COO/CSR, bitset fast path | consume |
| Signed structural (balance/pruner/tuples) | `hymeko_graph` | `CyclePruner` trait, `signed_tuples` | trait-based, Sync | implement the trait |
| Friedler / P-graph axioms | `hymeko_pgraph` | `FriedlerAxiomPruner` | sole owner; `CORE.YAML` forbids Friedler in generic graph | never reintroduce into `hymeko_graph` |
| Dense-linear **solvers** | `hymeko_tensor::linalg` (tensor) | `cg_matrix_free`, `dense_cholesky_solve` | f32, pinned summation order, typed errors | consume |
| Dense-linear **layer forward** (`X·Θ`) | **— none (gap) —** | (hnn/Nagare hand-roll) | no `matmul`/`linear_forward` exists | see **note D** |
| Rényi-α=2 estimators | `hymeko_tensor::renyi` | `s2_entropy`, `mi_coeff`, `renyi2_mi_grad`, `linear_gram`, `stable_rank`, `cheby_features` | α=2 only; analytic MI gradient; `1e-12` floors | consume; do not copy (Nagare copy retired 2026-08-19) |
| Parameter / stream layout | `hymeko_tensor::stream` | `HyMeKoStream`, `LayerParams` | flat arena, bit-exact round-trip | consume |
| Spline / Chebyshev / KB | `hymeko_spline` | `catmull_rom_*`, `chebyshev_{cr,deploy}_*`, `kb_*` + caches | clamp-Jacobian interior mask; bit-identical across threads | delegate (Nagare must — **note C**) |
| Clifford / rotor / holonomy / Cayley | `hymeko_clifford` | `Multivector`, `blade_product`, `cayley_rotor_*`, `rotor_holonomy_*`, quats, `autodiff::RotorChain` | f64 core / f32 rotor fast path; bit-identity-certified moves out of Nagare | delegate (Nagare must — **note C**) |
| Finite-diff / gates / frozen oracles | `hymeko_numeric_vv` | `fd`, `gate`, frozen clamp oracle | downstream-only, `publish=false` | no production crate may depend on it |
| Ir→view adapter + dense mapping | `hymeko_hnn` (core repo) | `HyperGraphView::{try_from_ir, from_ir}` | two-orientation incidence CSR, canonical↔dense provenance, typed `ViewBuildError` | consume the view |
| Hypergraph NN message passing | `hymeko_hnn` | `gather_edges_from_nodes`, `scatter_nodes_from_edges`, `clique_diag(_into)`, `implicit_clique_step`, `build_explicit_a` | ragged, generic-`F`, two-orientation, implicit, sign/abs/edge-weight, `*_into`, Rayon-threshold | **note A** |
| HGNN / Signed-HGNN / GCN-clique / Mesh layers | `hymeko_hnn` | `*Layer::forward` | compositions over IR adapters + Tensor SpMM + hnn mp | consume |
| Dense row transform `X·Θ` (internal) | `hymeko_hnn` (4 copies) | `hgnn`, `signed_hgnn`, `gcn_clique`, `mesh_nn` | internal duplication | consolidate to one private helper (**note D**) |
| Runtime / training step / optimizer / model composition | Nagare (**owner = `hymeko/nagare` Gitea, branch `feat/graph-canonical-dependency`** — the monorepo/github/non-git are predecessors; see audit §12) | `NagareRuntime`, `adam_step`, `FsrMixer`, HSiKAN/KAN/rotor/SLAM/vision ops | closed-form (autodiff-free) backwards; already de-vendored + consumes canonical graph/tensor by path-dep; **parallel consumer of graph+tensor, not layered on core/hnn** | the genuine Nagare band; keep it de-vendored |
| Closed-form backward passes | Nagare | `*_backward` per op | hand-derived adjoints; **the Nagare-specific asset** | keep in Nagare even where the forward is delegated |
| Holonomy learning subsystem | Nagare (monorepo) | `EntropyPoolLocalLearner`, `projection`, `features` | experiment/science layer | Nagare-owned |

## Notes (the overlaps that need a decision, not a mechanical move)

**A — signed-hypergraph-incidence TRIAD.** Three implementations apply a signed
hypergraph incidence (`B·x`, `Bᵀ·x`, clique): `hymeko_graph::hyper_operator`
(uniform-arity/f32/edge-major/allocating), `hymeko_hnn::message_passing`
(ragged/generic-`F`/two-orientation/`*_into`/threshold/sign+abs+edge-weight), and
Nagare `src/ops/hg_message.rs` (cycles-as-hyperedges, signed scaled mean/sum, star
expansion, fixed structural `s_v` scale — a port of `hymeko_neuro/.../cpml.py`). All
three are **hypergraph-based** (they honor invariant 8). They are a **responsibility
overlap, not a drop-in equivalence**. A shared kernel is permitted **only after a
differential test** proves numerical agreement across all three contracts on the shared
sub-domain (uniform-arity, f32, no threshold). Until then they stand as measured
Strategy variants, and the shared kernel — if adopted — must adapt over the hypergraph
incidence, never fold it into a dyadic graph.

**B — traversal.** hnn's Levi-graph `bfs_dense`/`dfs_preorder_dense` (bipartite
Node\|Edge state, allocating `Vec`) is an **intentionally different contract** from
graph's u32-CSR scratch-reused callback traversal. Consolidation, if any, is a shared
*borrowed-adjacency trait* both implement — not a merge.

**C — Nagare re-point.** Nagare currently reaches `hymeko_graph`/`hymeko_clifford`
through **stale forks** (monorepo) or **vendored copies** (github). The canonical
owners now provide every symbol Nagare uses (FIR spine, rotor, spline). Nagare must
depend on the canonical crates; the FIR re-point is numerically gated (audit §5).

**D — dense-linear-forward gap.** No canonical `X·Θ` owner exists. Near-term:
de-duplicate the 4× copies **inside hnn** into one private helper. A canonical
`hymeko_tensor::linalg::linear_forward` is deferred until a second consumer justifies
it (approval-gated; do not create speculatively).

## Anti-duplication invariants (enforceable gates)

1. No crate other than `hymeko_graph` provides graph topology/traversal/incidence.
2. No crate re-implements the `hymeko_tensor` degree seam or SpMM.
3. No `hymeko_graph` copy inside `hymeko_tensor` (retired at `8d0e546`; keep it retired).
4. No vendored framework crate inside any Nagare instance.
5. Friedler lives only in `hymeko_pgraph`.
6. `hymeko_numeric_vv` is downstream-only; no production crate depends on it.
7. The Nagare closed-form backwards stay in Nagare; only forwards may be delegated.
8. **The canonical IR stays a hypergraph** (user constraint, 2026-08-30). The
   canonical `hymeko_core::Ir` and the `hymeko_hnn::HyperGraphView` remain a
   **hypergraph** representation — ragged hyperedges + signed incidence, two
   orientations. Dyadic/pairwise forms (2-section clique graph, star/Levi expansion,
   line graph, `assemble_clique`) are **derived views**, never the canonical form. No
   consolidation may collapse the IR to a plain graph: when the incidence operator is
   shared with `hymeko_graph::hyper_operator` (note A), the shared kernel must operate
   on hypergraph incidence and hnn's ragged view stays canonical — the uniform-arity
   dyadic operator is an adapter over it, not a replacement for it. Consistent with the
   unified-IR invariant (one canonical Ir; derived rebuildable views).
