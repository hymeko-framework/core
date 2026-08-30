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
Nagare CANONICAL (hymeko/nagare @ feat/graph-canonical-dependency)
             → canonical hymeko_graph (../graph) + hymeko_{clifford,tensor,spline,vision} (../tensor)
               [de-vendored 2026-08-16; NOT dependent on core/hnn — parallel consumer; §12]
Nagare(monorepo, predecessor) → monorepo/hymeko_graph, monorepo/hymeko_clifford   [STALE forks]
Nagare(github, mirror/predecessor) → vendor/hymeko_graph, vendor/hymeko_clifford  [STALE vendored]
```

The **canonical** Nagare already matches the target Nagare edges below (graph→Nagare,
tensor→Nagare, no core/hnn→Nagare); the two forbidden vendored-framework edges survive
only in the predecessor instances (I2 targets those).

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

> **⚠ SUPERSEDED BY §12 (2026-08-30).** The decision below ranks the three *on-disk*
> instances and correctly rejects B and C. But the true production owner is the
> **canonical `hymeko/nagare` (Gitea) branch `feat/graph-canonical-dependency`** — a
> mature, already-de-vendored line the initial pass missed by judging `hymeko/nagare` by
> its empty `main`. Read §12 for the corrected determination; the monorepo (A) is a
> predecessor, not the owner.

**Decision (among the on-disk instances): the strongest is the monorepo `hymeko_nagare` (A).** It is a
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

---

## 11. Addendum (2026-08-30, same day) — Nagare re-verified + IR-stays-hypergraph constraint

**Nagare moved during the audit; re-checked.** The GitHub `nagare` advanced
`a8ca716 → 9432103` (30 commits). Inspection: these are **science/benchmark commits
only** — HSiKAN edge-chunk parallelization (7.3× / beats CPU PyTorch), Nagare-vs-PyTorch
closed-form KAN benchmarks, deep-holonomy learning (Adam rotor acceleration, honest
negative baseline), the Evolvent multifrontal/junction-tree series, `mesh_tensor`
(hypergraph latent space as a contraction), and article figures. **The vendoring is
unchanged:** `vendor/hymeko_graph` + `vendor/hymeko_clifford` are still present, still
the stale 2026-07-06 snapshot, deps still path-vendored. The Gitea `hymeko/nagare`
target is unchanged (`a59f9fe` + the same migration branches); the monorepo remote was
unreachable at re-check (local stays `5b53a92`).

**Net effect on the audit:** the category-E stale-vendor finding (items 1–2) and the
production-owner determination (§4) **stand**. One refinement: `nagare_github` is the
**actively developed science line** (not a frozen one) — so the migration must treat it
as a live consumer, freezing the differential oracles (I1) against its *current* head
`9432103`, and the re-point (I4) must not disturb the HSiKAN/Evolvent/holonomy results
it now carries. The commit `c578892 "integrate the real spine into the registry"`
confirms it still consumes the vendored FIR spine — exactly the re-point target.

**IR-stays-hypergraph constraint (user, 2026-08-30) — now a hard guardrail.** The
canonical `hymeko_core::Ir` and `hymeko_hnn::HyperGraphView` **remain a hypergraph**
(ragged hyperedges + signed incidence, two orientations). All dyadic forms — 2-section
clique graph, star/Levi expansion, line graph, `assemble_clique` — are **derived
views**, never the canonical representation. This directly constrains the item-4
consolidation (`hymeko_graph::hyper_operator` vs `hymeko_hnn::message_passing`): a
shared kernel is permitted **only** as an adapter over the hypergraph incidence with
hnn's ragged view canonical; it must **not** fold the hypergraph into a uniform-arity
dyadic graph to make the two kernels line up. The science direction supports this —
`nagare_github`'s own results (`F-EVO-5: hyperedge tensor beats pairwise`, `mesh_tensor:
hypergraph latent space`) depend on the hypergraph structure surviving. Recorded as
ownership invariant 8 (`docs/hymeko-kernel-ownership.md`) and a governing constraint in
`docs/hymeko-kernel-migration.md`.

---

## 12. Material correction (2026-08-30) — the canonical Nagare is `hymeko/nagare`, and it has already executed I2/I4

The user pointed to a new Nagare commit. Re-inspection **corrects the §4 production-owner
determination.** The audit judged the Gitea `hymeko/nagare` by its `main` (`a59f9fe`,
"essentially empty") and treated it as a target-only. That was wrong: the active branch
**`feat/graph-canonical-dependency`** (tip `a182c99`) is a **mature, canonical-consuming
Nagare** in the same `hymeko/*` governance namespace as core/graph/tensor — i.e. **the
canonical Nagare owner**. The on-disk instances I inventoried (monorepo `hymeko_nagare`
`5b53a92`, `nagare_github` `a8ca716`/`9432103`, non-git `nagare`) are **predecessors /
public mirrors** of this line.

**What this branch already is (verified from its tree + log):**

- **De-vendored + re-pointed to canonical crates** (my I2 + I4 — *already done*):
  `Cargo.toml` path-deps `hymeko_graph = ../graph/hymeko_graph`,
  `hymeko_{clifford,tensor,spline,vision} = ../tensor/…`; **no `vendor/` dir**; comment
  "De-vendored 2026-08-16: the shared algebra crates now come from the `tensor`
  workspace… single source of truth." Commit `444882d "Phase 4: Nagare consumes the
  canonical hymeko_graph from hymeko/graph"`.
- **Oracle-first discipline** (my I1): `26b584f "Freeze the public KAN surface as a
  behavioural oracle, before anything moves"`, `87fd896 "Capture the cascade example as
  a frozen oracle and measure it against the framework"`, `2c4ebf6 "Phase 7E-A
  (capture): freeze the pre-extraction HSiKAN before anything moves"`.
- **Traversal consolidation** (my I6, in progress): `a182c99 "Phase 7E-A: one
  aggregation traversal, one backward traversal, statically monomorphized"` (+ a
  300-line frozen-oracle test) — matches the §6.5 preference (Strategy, static
  monomorphization, no `Box<dyn>` in the hot loop).
- **Chebyshev-claim separation** (my audit item 12): `592a6fe "…separate two Chebyshev
  claims the audit ran together"`.

**Two structural facts that revise the audit model:**

1. **Nagare is a *parallel* consumer of graph/tensor, not layered on core/hnn.** This
   branch depends on `hymeko_graph` + the tensor-workspace crates but **not** on
   `hymeko_core` or `hymeko_hnn`. It carries its own `src/{core, graph, runtime}` —
   `src/graph` is Nagare-specific *modeling* (pose graphs, junction trees, Steiner,
   planar, CSP), not a topology duplicate. The target DAG's optional edge
   "core/hnn → Nagare" is currently **absent by design**; Nagare reaches the framework
   through graph+tensor only.
2. **The signed-hypergraph-incidence overlap is a TRIAD, not a pair.** Beyond graph's
   `hyper_operator` and hnn's `message_passing`, this Nagare has a **third**
   implementation, `src/ops/hg_message.rs` (`hg_node_to_edge_forward` /
   `hg_edge_to_node_forward` — signed scaled **mean**/**sum**, star expansion, fixed
   structural `s_v` scale; a port of `hymeko_neuro/hyperedge/cpml.py`
   `CapsuleHypergraphRouter`). **It is hypergraph-based** (cycles-as-hyperedges) — so it
   honors the IR-stays-hypergraph guardrail (§11). Audit item 4 is therefore a
   three-way family (graph f32-uniform-arity / hnn ragged-generic / Nagare
   cycles-as-hyperedges-with-`s_v`), and any shared kernel must be gated by a
   differential test across **all three** contracts.

**Revised plan status.** I2 (de-vendor) and I4 (canonical re-point) are **substantially
already executed on this branch** — the migration plan's role shifts from "propose" to
"verify + finish + merge to `main`": confirm the FIR differential oracle against the
canonical `hymeko_graph::spine` (the §5 divergence still applies — the branch's
`../graph/hymeko_graph` must be the canonical `5078980`, not a stale checkout), complete
the Phase-7 traversal consolidation, and reconcile the incidence triad. **I did not
re-inventory this ~40-op framework** (HSiKAN, KAN, rotor, SLAM, vision) — a full
contract-level audit of the canonical `hymeko/nagare` line is the natural next step if
you want it; this correction records the ownership fact and the already-done phases so
the plan is not built on a stale snapshot. **No Nagare source was touched.**

---

## 13. Spec coverage map (this deliverable ↔ the formal task)

| Spec item | Where satisfied |
|---|---|
| Entry conditions (corrective done, tree clean, HEAD==origin/main, IFAC untouched) | verified before audit; corrective `e2952c7`; audit commits are docs-only |
| Verify remote SHAs, don't trust | §0 (found graph/tensor local stale; audited at canonical worktrees) |
| Prove the production Nagare instance | §4 (on-disk ranking) **+ §12 (corrected owner = `hymeko/nagare` canonical branch)** |
| Canonical ownership rules (graph/tensor/core/hnn/Nagare) | §1 + `docs/hymeko-kernel-ownership.md` |
| **Part A** — semantic inventory of every named site | §1/§2 (all 13 named items: hnn gather/scatter/clique-diag/implicit-clique; graph `SignedHyperIncidence` 4 ops; hnn Berge BFS/DFS; graph CSR/scratch; hnn dense_transform(_dual); Nagare linear fwd/bwd; graph `fir_cycle_scatter_mean`; Nagare scatter_mean fwd/bwd; Nagare signed_scatter; tensor linalg/renyi/stream; spline+Nagare Chebyshev; clifford+Nagare rotor; LCG/activation/loss/workspace) |
| Categories A–F, by contract not name | §2 matrix (9 comparison axes applied) |
| Known distinction (graph uniform-arity vs hnn ragged; scatter_mean ≠ signed scatter; shared abstraction only after differential test) | §2 item 4 (now the triad, §12) + ownership note A |
| **Part B** — minimal common kernel layer (dense transform / gather-scatter / traversal / Nagare rewire) | `docs/hymeko-kernel-migration.md` **Part B** (B.1–B.4, preferred-form design) |
| **Part C** — execution order I0–I8, per-phase commit + rollback | migration doc phase table + I1–I8 |
| **Part D** — mandatory test list | §7 test matrix (all listed cases incl. f32/f64, serial/Rayon parity, FD, adjoint, star-vs-clique, PoW-0, the 7 source-first workloads, ≥4 mutants + hash restore) |
| **Part E** — Criterion A/B, all axes + thresholds + A/A null | §7 benchmark matrix |
| **Part F** — target acyclic dependency DAG + forbidden edges | §3 (current vs target; canonical Nagare matches target, §12) |
| **Part G** — deliverables | this report + `docs/hymeko-kernel-ownership.md` + `docs/hymeko-kernel-migration.md`; DAG §3; ownership matrix §2; exact duplicate counts §2; Nagare responsibility map §4/§12; minimal public APIs §6; per-phase approval tokens (migration ledger); test+bench matrix §7; deletable LOC exact vs semantic §2 + migration summary |
| Additive correction of the prior duplication audit | §9 + appended addendum in `docs/unified-ir-duplication-and-profile-audit.md` |
| HARD STOP after audit; no source/manifest/lockfile; no dep added; no Nagare source moved | honored — all commits docs-only; §10 |
