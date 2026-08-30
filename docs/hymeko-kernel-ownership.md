# HyMeKo kernel ownership — one capability, one owner

**Status:** 2026-08-30, derived from the cross-repository I0 audit
([`reports/2026-08-30-hnn-nagare-tensor-integration-audit.md`](../reports/2026-08-30-hnn-nagare-tensor-integration-audit.md)).
This is the **normative ownership map**: every mathematical, traversal, and
message-passing capability in the framework has exactly one production owner; every
other layer must reach it by dependency, adapter, or composition — never by copy.

Verified against canonical `origin/main`: core `e2952c7`, graph `5078980`, tensor
`8d0e546`.

## Ownership table

| Capability | Owner crate (repo) | Public entry point | Contract summary | Non-owners must |
|---|---|---|---|---|
| Canonical IR | `hymeko_core` (core) | `Ir`, `try_from_ir` seams | one canonical IR; dense views are derived | build derived views, never a second IR |
| Sparse tensor formats | `hymeko_core` | `TensorCoo`, `TensorCsr`, `spmv/spmm/*_into` | serial+Rayon row-disjoint bitwise-identical | call `*_into`; do not re-implement SpMM |
| Degree computation | `hymeko_core` | `compute_degree_by_index`, `DegreeMode`, `inv_sqrt_degree` | generic over an index projection `P: Fn(I)->usize` | adapt via `\|d\| d.as_usize()` (hnn already does) |
| WGPU backend base | `hymeko_core` (+ `hymeko_wgpu`) | resident SpMM/activation | PoW-0 frozen hash `00d4c01a` | consume; no second backend |
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
| HGNN / Signed-HGNN / GCN-clique / Mesh layers | `hymeko_hnn` | `*Layer::forward` | compositions over core seam + core SpMM + hnn mp | consume |
| Dense row transform `X·Θ` (internal) | `hymeko_hnn` (4 copies) | `hgnn`, `signed_hgnn`, `gcn_clique`, `mesh_nn` | internal duplication | consolidate to one private helper (**note D**) |
| Runtime / training step / optimizer / model composition | Nagare (**owner = monorepo `hymeko_nagare`**) | `NagareRuntime`, `adam_step`, `FsrMixer` | closed-form (autodiff-free) backwards; Adam-only today | the genuine Nagare band; do not vendor kernels into it |
| Closed-form backward passes | Nagare | `*_backward` per op | hand-derived adjoints; **the Nagare-specific asset** | keep in Nagare even where the forward is delegated |
| Holonomy learning subsystem | Nagare (monorepo) | `EntropyPoolLocalLearner`, `projection`, `features` | experiment/science layer | Nagare-owned |

## Notes (the overlaps that need a decision, not a mechanical move)

**A — `hymeko_graph::hyper_operator` vs `hymeko_hnn::message_passing`.** Both apply a
signed hypergraph incidence (`B·x`, `Bᵀ·x`, clique). They are a **responsibility
overlap, not a drop-in equivalence**: graph is uniform-arity/f32/edge-major/allocating;
hnn is ragged/generic-`F`/two-orientation/`*_into`/threshold/sign+abs+edge-weight. A
shared kernel is permitted **only after a differential test** proves numerical
agreement on the shared sub-domain (uniform-arity, f32, no threshold). Until then both
stand as measured Strategy variants.

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
2. No crate re-implements the `hymeko_core` degree seam or SpMM.
3. No `hymeko_graph` copy inside `hymeko_tensor` (retired at `8d0e546`; keep it retired).
4. No vendored framework crate inside any Nagare instance.
5. Friedler lives only in `hymeko_pgraph`.
6. `hymeko_numeric_vv` is downstream-only; no production crate depends on it.
7. The Nagare closed-form backwards stay in Nagare; only forwards may be delegated.
