# Cross-repository HNN / Nagare / Tensor / Graph integration audit (I0)

**Written:** 2026-08-30 · **Author:** audit pass, read-only across five repositories.
**No production source, manifest, or lockfile was modified.** This is the I0
deliverable of the integration task: a semantic inventory + ownership decision +
executable migration plan. Companion documents: [`docs/hymeko-kernel-ownership.md`](../docs/hymeko-kernel-ownership.md)
(the ownership matrix) and [`docs/hymeko-kernel-migration.md`](../docs/hymeko-kernel-migration.md)
(the phased plan I1–I8). This report additively corrects the too-narrow "no
duplication" conclusion of [`docs/unified-ir-duplication-and-profile-audit.md`](../docs/unified-ir-duplication-and-profile-audit.md)
(see §9).

Method: five read-only inventory agents produced contract cards (pre/postconditions,
shape/layout, sign & weight semantics, duplicate-incidence behaviour, accumulation
order, serial/Rayon dispatch, allocation ownership, forward/backward split, tests)
across `hymeko_graph`, `hymeko_tensor`, `hymeko_hnn`, and three Nagare instances.
Every categorization below rests on **contracts, not function names**.

---

## 0. Verified ground truth (the stated remote SHAs were checked, not trusted)

The task's stated audit SHAs were verified against `origin/main`; the **local
checkouts of graph and tensor are stale** and were *not* used for the audit — the
audit ran against read-only worktrees pinned at the canonical `origin/main` SHAs.

| Repo | Canonical `origin/main` | Local checkout | Note |
|---|---|---|---|
| `hymeko/core` | `e2952c7` | `e2952c7` (this repo) | clean; = HEAD after the M2c.1 corrective |
| `hymeko/graph` | `5078980` | `2908785` (stale, behind) | audited at `5078980` worktree |
| `hymeko/tensor` | `8d0e546` | `c0f9719` (stale + dirty) | audited at `8d0e546` worktree; `8d0e546` **retired `hymeko_graph`** |
| `hymeko/nagare` (Gitea) | `a59f9fe` | — | **not empty**: carries migration branches `feat/graph-canonical-dependency`, `feat/phase-e-cascade-depth-retirement`, `feat/experiments-examples-integration`, … → the intended **target** repo, migration already begun |
| GitHub `nagare` | `a8ca716` | `a8ca716` | standalone; **vendors** stale graph+clifford |
| monorepo `hymeko_framework_rust/hymeko_nagare` | `5b53a92` (branch `hymeko-neuro-migration`, WIP) | `5b53a92` | richest live Nagare; path-deps monorepo siblings |
| non-git `…/nagare` | — (bare-init, **0 commits**) | — | byte-identical fmt-snapshot of the monorepo's shared core |

**Correction to the task's own framing, evidence-based:** the premise of "parallel
mathematical / traversal / message-passing implementations across hnn, graph, tensor
and Nagare" holds **strongly for Nagare** and **weakly for hnn↔graph↔tensor**. The
core/graph/tensor/hnn crates are, contract-for-contract, **largely disjoint problem
domains** with the ownership already mostly clean; the genuine cross-repo duplication
is concentrated in the **Nagare instances** (vendored/copied forwards) and a small
set of named overlaps (§2). This is stated up front because it changes what the
migration must actually do: it is 80% a Nagare-rewire + delete, not a four-way merge.

---

## 1. Canonical owners (verified)

| Layer | Crate(s) | Owns (by contract) | Deps |
|---|---|---|---|
| **IR / primitives** | `hymeko_core` | canonical `Ir`; `TensorCoo`/`TensorCsr` + `spmv`/`spmm`/`*_into` (serial+Rayon, row-disjoint bitwise-identical); `aggregation`; **the degree seam** `compute_degree_by_index`/`DegreeMode`/`inv_sqrt_degree`; WGPU base | none (leaf) ✓ |
| **Topology / graph** | `hymeko_graph` | `SignedHyperIncidence` (`node_to_edge`/`edge_to_node`/`star_matvec`/`assemble_clique` — uniform-arity, f32, edge-major, allocating, serial, forward+adjoint); CSR + BFS/DFS (scratch-reused, callback, depth-bounded); ~22 top-k cycle enumerators; **signed-cycle FIR spine** (`clifford_fir_forward/backward`, `CliffordFIR`, `SignedCycleFIR`, `TopKCyclesBatch`, golden digests); line-graph `incidence`; balance/pruner/`signed_tuples`; `rand_lcg` | `rayon`, `dashmap` |
| **P-graph** | `hymeko_pgraph` | Friedler axioms (sole owner — `CORE.YAML` forbids Friedler in the generic crate) | `core`, `graph`, `parser` |
| **Numeric kernels** | `hymeko_tensor` | linalg **solvers** (`cg_matrix_free`, `dense_cholesky_solve`); Rényi-α=2 (`s2_entropy`, `mi_coeff`, `renyi2_mi_grad`, `linear_gram`=A·Aᵀ, `stable_rank`, `cheby_features`); `stream` flat param arena. **No dense `matmul`/`linear_forward`** | none (leaf) ✓ |
| **Splines** | `hymeko_spline` | `catmull_rom` fwd/bwd, `chebyshev_{cr,deploy}` fwd/bwd, `kochanek_bartels` fwd/bwd; clamp-Jacobian interior mask; determinism contract (bit-identical across threads) | `rayon` (optional) |
| **Geometric algebra** | `hymeko_clifford` | `Multivector`/`Signature`/`blade_product`; rotor (`cayley_to_unit_quat`, quats, `rotor_holonomy` fwd/bwd, `cayley_rotor` fwd/bwd — bit-identity-certified moves out of Nagare); `autodiff::RotorChain` reverse-mode adjoint (`Backend` strategy) | none (leaf) |
| **V&V** | `hymeko_numeric_vv` | finite differences, scientific gates, **frozen clamp oracle** | `spline` (publish=false) |
| **View adapter + HNN** | `hymeko_hnn` | `Ir`→`HyperGraphView` (dense canonical↔view provenance, two-orientation incidence CSR, typed `ViewBuildError`); hypergraph NN message passing (`gather`/`scatter`/`clique_diag`/`implicit_clique` — ragged, generic-`F`, two-orientation, implicit, sign/abs/edge-weight); HGNN / Signed-HGNN / GCN-clique / Mesh layers; `dense_transform` (X·Θ) | `core` only |
| **Runtime** | Nagare (owner = monorepo `hymeko_nagare`, §4) | training step, optimizer (Adam), model composition, **hand-written closed-form backwards**, holonomy science | `graph`, `clifford` (currently *stale* copies — §4/§5) |

---

## 2. Site-by-site duplication / overlap matrix (categorized A–F)

Categories: **A** byte-identical · **B** same math, different API/layout · **C**
shared kernel + domain adapter · **D** intentionally different numerical contract ·
**E** stale/vendor/generated copy · **F** genuinely HNN/Nagare-specific.

| # | Site(s) | Responsibility | Discriminating contract difference | Cat | Action |
|---|---|---|---|---|---|
| 1 | `nagare_github/vendor/hymeko_graph` (15 421 LOC) | full graph topology stack | **verbatim stale snapshot** (commit `162a5c7`, 2026-07-06), never edited; Nagare uses **4 symbols** (`CliffordFIR`, `TopKCyclesBatch`, `clifford_fir_forward/backward`) | **E** | delete, re-point to canonical `hymeko_graph` |
| 2 | `nagare_github/vendor/hymeko_clifford` (776 LOC) | geometric algebra | verbatim stale snapshot; canonical `hymeko_clifford` now exists | **E** | delete, re-point |
| 3 | `…/nagare` (non-git, 824 src LOC) | runtime shared core | **byte-identical to monorepo `hymeko_nagare` modulo rustfmt + crate name**; zero git commits | **A/E** | delete (or thin facade over the owner) |
| 4 | `hymeko_graph::hyper_operator` (`node_to_edge`/`edge_to_node`/`star_matvec`/`assemble_clique`) vs `hymeko_hnn::tensor::message_passing` (`gather`/`scatter`/`implicit_clique`/`clique_diag`) | signed hypergraph incidence apply `B·x`, `Bᵀ·x`, clique | graph: **uniform-arity, f32, edge-major, allocating, serial**; hnn: **ragged, generic-`F`, two-orientation, `*_into`, Rayon-threshold, sign+abs+edge-weight projection, implicit** | **C-pending / D** | shared kernel **only after** a differential test on the shared sub-domain (uniform-arity, f32); else keep both as measured Strategy variants |
| 5 | `hymeko_hnn::tensor::compute_bipartite_degrees` vs `hymeko_core::…::compute_degree_by_index` | vertex/edge degree | same `degree_mass` arithmetic; hnn adds dual-output + `use_abs` | **B** | route hnn through the core seam (add `use_abs`/dual mode), or a new `DegreeMode` |
| 6 | `hymeko_hnn` degree seam usage (HGNN/GCN) | degree normalization | — (already shared kernel + `\|d\| d.as_usize()` adapter) | **C (resolved)** | none — this is the target pattern, done under `dense-view-index-compute-degree` |
| 7 | `hymeko_hnn` `dense_transform`/`_dual` (X·Θ) ×4 sites (`hgnn`, `signed_hgnn`, `gcn_clique`, `mesh_nn`) | dense row transform | **no canonical owner exists** (tensor has only `linear_gram`=A·Aᵀ); replicated 4× **inside hnn** | **F (internal-dup)** | extract one private `dense_row_transform` helper in hnn; separately, decide whether a canonical `hymeko_tensor` dense-linear-forward should exist (§8) |
| 8 | Nagare `linear_forward/backward` vs (nothing) | dense GEMM+bias | **no canonical `matmul` anywhere**; Nagare owns the backward | **F (fwd) / gap** | keep in Nagare unless a `hymeko_tensor` dense-linear-forward is created (approval-gated) |
| 9 | Nagare `catmull_rom`/`chebyshev` (≈520 LOC, monorepo + github) vs `hymeko_spline` | spline/Chebyshev eval | Nagare forward is a port of the Python `hymeko_neuro.core.splines`; **canonical `hymeko_spline` now owns fwd+bwd with a stronger determinism contract** | **C/E** | delegate forward to `hymeko_spline`; keep only Nagare-specific composition |
| 10 | Nagare `cayley_rotor`/rotor forward vs `hymeko_clifford::rotor` | Cayley rotor + quats | **already moved to `hymeko_clifford` 2026-08-28 with bit-identity certification**; Nagare re-exports/pending rewire | **C/E** | delete Nagare copy, depend on `hymeko_clifford` |
| 11 | Nagare `clifford_fir` (FIR spine) | signed-cycle FIR fwd+bwd | **not Nagare code** — vendored/path-dep `hymeko_graph::spine`; monorepo graph's spine **diverges** from canonical (695 vs 978 lines, 537 differing) | **E + divergence** | re-point to canonical `hymeko_graph::spine`; freeze a differential FIR oracle first (I1) |
| 12 | `hymeko_tensor::renyi::cheby_features` vs `hymeko_spline::…::chebyshev_*` | `T_{k+1}=2xT_k−T_{k−1}` recurrence | two independent impls, different purpose (MI feature map vs spline basis) | **B** | share one Chebyshev primitive within the tensor workspace |
| 13 | `hymeko_hnn::traversal::{bfs_dense,dfs_preorder_dense}` vs `hymeko_graph::traversal::{dfs_visit,bfs_distances}` | BFS/DFS | hnn: bipartite Levi state (Node\|Edge), allocating `Vec`, no scratch/depth-bound; graph: u32 CSR, scratch-reused, callback, depth-bounded | **D** | keep separate; optionally a borrowed-adjacency trait both implement (no merge) |
| 14 | LCG: `hymeko_hnn::workloads::Lcg`, `hymeko_graph::rand_lcg::Lcg`, `hymeko_tensor` test LCGs (×3), Nagare bench LCG | MMIX LCG | same multiplier `6364136223846793005`; different seeding/bit-extraction/stream; **all fixture/test-scoped** | **B (fixture)** | low value; optional shared test-support LCG |
| 15 | `hymeko_graph` internal: ~22 `enumerate_top_k_*`, `build_csr` ×2, `bs_*` ×2 | cycle enumeration / CSR build | Cartesian-product API surface (§6.5 #1) + acknowledged bit-identical dup | **B (internal)** | graph-internal refactor (config-struct Strategy); out of cross-repo scope |

**Exact duplicate counts.** Byte-identical / stale-vendor copies (category A/E) that
can be **deleted outright** after re-pointing: `nagare_github/vendor/hymeko_graph`
**15 421 LOC**, `nagare_github/vendor/hymeko_clifford` **776 LOC**, non-git `nagare`
**1 092 LOC** (824 src + 268 test/bench). **Exact/vendor deletable ≈ 17 289 LOC.**
Semantic duplicates (category B/C, *refactor* not raw delete): Nagare `catmull_rom`
≈520 LOC → delegate; Nagare `cayley_rotor` ≈150 LOC → delegate; hnn `dense_transform`
×4 → one helper (≈90 LOC net); `compute_bipartite_degrees` ≈40 LOC → seam; tensor
Chebyshev recurrence ≈60 LOC → shared. **Semantic consolidation ≈ 860 LOC.**

---

## 3. Dependency DAG — current vs target

**Current (verified from manifests):**

```
hymeko_core  ── (leaf)
hymeko_graph ── (leaf: rayon,dashmap)      hymeko_pgraph → core, graph
hymeko_tensor/clifford/spline ── (leaves)  hymeko_numeric_vv → spline
hymeko_hnn   → hymeko_core                 (does NOT depend on graph/tensor)
Nagare(monorepo) → monorepo/hymeko_graph, monorepo/hymeko_clifford   [STALE forks]
Nagare(github)   → vendor/hymeko_graph, vendor/hymeko_clifford        [STALE vendored]
```

**Target (acyclic):**

```
hymeko_graph ─┐
hymeko_tensor ┼──> hymeko_hnn ──> higher-level models
hymeko_core ──┘
hymeko_graph ─────> Nagare
hymeko_tensor ────> Nagare
hymeko_core/hnn ──> Nagare   (only if the runtime genuinely starts from a canonical Ir)
```

**Forbidden edges (all currently satisfied except where noted):** `core→hnn` ✓,
`core→Nagare` ✓, `tensor→Nagare` ✓, `graph→Nagare` ✓, graph-copied-into-tensor ✓
(retired at `8d0e546`), **vendored framework crate in Nagare ✗ (github instance
violates — items 1,2)**, `production→V&V` ✓ (numeric_vv is downstream-only, enforced).

**The only DAG violations are on the Nagare side:** stale/vendored graph+clifford
instead of the canonical crates. Fixing them is the migration's spine.

---

## 4. Nagare responsibility map (which instance is the production owner)

Proven by evidence, not chosen by convenience:

| | monorepo `hymeko_nagare` (A) | non-git `nagare` (B) | GitHub `nagare_github` (C) |
|---|---|---|---|
| src LOC | **3 736** | 824 | 3 846 own + **16 197 vendored** |
| tests | **55 / 19 files** | 15 / 6 | 45–46 / 6 files |
| last code commit | 2026-07-08 (newest) | — (**0 commits**) | 2026-07-07 |
| deps mechanism | **path-dep siblings** | path-dep into monorepo | **vendored stale copies** |
| ops library | signed_scatter, cayley_rotor, catmull_rom, fsr_mixer, fused_entropy, project_alpha_mix | shared core only (subset of A) | full own-backward library |
| holonomy subsystem | **yes (812 LOC)** | absent | learner + science layer |
| provenance | committed history | **none** | committed history |
| distinctive weakness | monorepo-graph is a **stale fork** of canonical; empty `backend/`; Adam-only | zero-commit snapshot | ~15 k LOC dead vendored graph |

**Decision: the production Nagare owner is the monorepo `hymeko_nagare` (A).** It is a
strict superset of B (B is a fmt-only extraction snapshot with no git history — delete
or convert to a thin facade), and unlike C it does not vendor a 15 k-LOC dead graph
copy. The **genuinely Nagare-owned asset across all instances is the hand-written
closed-form backward pass library** (autodiff-free gradients for `scatter_mean`,
`signed_scatter`, `linear`, `cayley_rotor`, `project_alpha_mix`, `catmull_rom`,
`fused_entropy`, `fsr_mixer`, `bce`), the `NagareRuntime` training band, the Adam
optimizer state, and the holonomy science layer. Even where a *forward* duplicates a
canonical kernel, the *backward* is Nagare's — that split governs every categorization
in §2 (delegate the forward, keep the backward).

**The empty Gitea `hymeko/nagare`@`a59f9fe` is the migration target**, not a source —
and it already carries the migration branches, so I2–I5 land there.

---

## 5. Migration-critical divergence (independently verified)

Production Nagare (A) path-depends on the **monorepo's** `hymeko_graph` and
`hymeko_clifford`, which are **older forks** of the canonical crates:

- monorepo `hymeko_graph::spine` = **695 lines** vs canonical = **978 lines**; **537 differing lines**.
- monorepo graph **lacks** `hyper_operator` and `signed_tuples`, which canonical already absorbed from tensor.
- the FIR symbols Nagare consumes (`CliffordFIR`, `TopKCyclesBatch`, `clifford_fir_forward/backward`) **all still exist in canonical** → API-compatible re-point, but the **numerics differ** (thread-count reduction semantics, `GradRequest::{ParamsOnly,Full}` split, the 2026-08-16 atomic→reduction correction).

**Consequence for the plan:** re-pointing Nagare from monorepo/vendored graph to
canonical `hymeko/graph` is a *numerical* change, not a mechanical one. It **must** be
gated by a frozen differential FIR oracle (I1) proving `clifford_fir_forward/backward`
agree on a fixed corpus, or an explicit, recorded acceptance that the canonical spine
is the new golden.

---

## 6. Minimal public APIs the migration needs

- **`hymeko_graph`** (already public): `SignedHyperIncidence` + the four operators; `spine::{clifford_fir_forward, clifford_fir_backward, CliffordFIR, TopKCyclesBatch, GradRequest, FirBackward}`; `traversal::{Csr, dfs_visit, bfs_distances, Dfs/BfsScratch}`.
- **`hymeko_tensor`** (public): `linalg::{cg_matrix_free, dense_cholesky_solve}`; `renyi::*`; `stream::HyMeKoStream`. **Gap:** a dense `linear_forward`/`matmul` does not exist — see §8.
- **`hymeko_spline`** (public): `catmull_rom_{forward,backward}`, `chebyshev_{cr,deploy}_{forward,backward}`, `kb_{forward,backward}` + caches.
- **`hymeko_clifford`** (public): `cayley_rotor_{forward,backward}`, `rotor_holonomy_{forward,backward}`, quats, `autodiff::RotorChain`.
- **`hymeko_hnn`** (public): `HyperGraphView::{try_from_ir, from_ir}`; the layer `forward` passes; the message-passing kernels.

No **new** public API is strictly required to remove the Nagare duplication (re-point +
delete). One **optional** new API is the dense-linear-forward decision (§8).

---

## 7. Test & benchmark matrix (for I1/I7/I8 — clients of the public API only)

**Tests (freeze as differential oracles before any deletion):**

| Oracle | Public API exercised | Discriminates |
|---|---|---|
| parser → `Ir` → `HyperGraphView` → message passing | hnn | end-to-end view build + gather/scatter |
| FIR forward+backward corpus | `hymeko_graph::spine` | monorepo-fork vs canonical FIR (§5) |
| spline fwd/bwd bit-parity | `hymeko_spline` | Nagare `catmull_rom` vs canonical |
| rotor fwd/bwd bit-identity | `hymeko_clifford` | Nagare `cayley_rotor` vs canonical (already certified) |
| ragged & uniform hypergraph; empty/singleton/duplicate/repeated-member; ± / ~ signs; weighted/unweighted; non-contiguous canonical ids; permuted decl/incidence order; f32 (+ f64 where supported) | hnn message passing + graph `hyper_operator` | item-4 differential (C vs D) |
| serial vs Rayon bitwise parity | graph spine, hnn mp, core spmm | dispatch equivalence |
| finite-difference / adjoint identity | Nagare backwards, `clifford::autodiff` | gradient correctness |
| star-vs-clique operator identity | graph `star_matvec` vs `assemble_clique` | operator agreement |
| CPU/WGPU PoW-0 regression | core/wgpu | frozen hash `00d4c01a` |
| Steiner / sunflower / Mycielski / random-hub / network / robot / biomedical source-first workloads | hnn `HyperSpec` + parser | scenario coverage |
| ≥4 fault-injection mutants + hash-checked restore | each consolidated kernel | catch-then-restore |

**Benchmarks (Criterion A/B before/after each delegation, alternating + A/A null):**
dense transform; gather/scatter; HGNN/Signed-HGNN forward; Nagare linear/scatter/runtime;
BFS/DFS; small/medium/large (≥1M incidence); allocation count; resident + peak
construction memory; Amaterasu as a second host if available. Thresholds: hot-path
regression <3%; allocation must not grow; resident memory must not grow (except a
justified provenance/adapter need); **any numerical-contract change is an automatic
HARD STOP.**

---

## 8. Open design decision — the dense-linear-forward gap

Neither `hymeko_tensor` nor anywhere in the framework owns a dense `X·Θ` / `matmul` /
`linear_forward`. It is hand-written **4× inside hnn** and **once in Nagare** (with a
backward). Two options, each approval-gated:

1. **Create `hymeko_tensor::linalg::linear_forward(_into)`** (+ optional backward) as
   the canonical dense-linear owner; hnn and Nagare delegate. Removes 5 copies; but the
   contracts differ (hnn: fused-dual + Rayon-tiling + `X·Θ`; Nagare: `X·W+b` + backward)
   so the shared kernel must be a `*_into` primitive both adapt, proven by a
   differential test. Token: `APPROVED-CORE-EDIT: tensor-dense-linear-forward`.
2. **Keep them separate** (category D by contract divergence); only de-duplicate the
   4× **inside hnn** into one private helper. Lower blast radius, no cross-repo API.

Recommendation: **(2) now** (hnn-internal helper — cheap, no cross-repo coupling),
**defer (1)** until a second genuine consumer justifies a canonical dense-linear owner.
Do not create the shared kernel speculatively (§3-forbidden defensive abstraction).

---

## 9. Additive correction to the prior duplication audit

[`docs/unified-ir-duplication-and-profile-audit.md`](../docs/unified-ir-duplication-and-profile-audit.md)
concluded "**No TRUE DUPLICATION … was found in editable (non-core) production code.**"
That conclusion is **correct but scope-limited**: it audited only the **core workspace**
(`hymeko_core`, `hymeko_hnn`, `hymeko_wgpu`) and explicitly did not cover the canonical
`hymeko_graph`, `hymeko_tensor`, or the Nagare instances. Across the **full framework**,
duplication does exist — concentrated in Nagare (≈17.3 k LOC of stale/vendored copies,
items 1–3, 9–11) and a handful of B/C overlaps (items 4, 5, 12). The core-workspace
verdict stands; it simply never claimed the cross-repo scope. A correcting note has
been appended to that document.

---

## 10. HARD STOP

I0 audit + migration plan produced. **No production source, manifest, or lockfile was
modified.** No Nagare source was chosen without evidence (§4). No dependency was added
or code moved. The phased execution (I1–I8), each with its own approval token and
rollback point, is specified in [`docs/hymeko-kernel-migration.md`](../docs/hymeko-kernel-migration.md);
none of it begins without explicit user approval per phase.
