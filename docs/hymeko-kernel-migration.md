# HyMeKo kernel-consolidation migration plan (I1–I8)

**Status:** 2026-08-30, executable plan derived from the I0 audit
([`reports/2026-08-30-hnn-nagare-tensor-integration-audit.md`](../reports/2026-08-30-hnn-nagare-tensor-integration-audit.md);
ownership: [`docs/hymeko-kernel-ownership.md`](hymeko-kernel-ownership.md)).
**No phase below has been executed.** Each phase is a **separate commit with its own
rollback point and its own approval token**; none begins without explicit user
approval. This is not a flag-day migration — the order is chosen so the framework
stays green after every phase.

Governing constraints (unchanged from the task): hot-path regression `<3%`;
allocation must not grow; resident memory must not grow (except a justified
provenance/adapter need); **any change to a numerical contract is an automatic HARD
STOP** requiring a differential verdict first; A/A null control + alternating A/B on
every delegation; freeze the differential oracle *before* deleting the old code.

---

## Phase order and rationale

The dependency risk is almost entirely **Nagare → stale graph/clifford** (audit §3,
§5). So the plan front-loads *freezing oracles* and *removing dead copies*, then
re-points Nagare, then addresses the smaller hnn/graph/tensor overlaps.

| Phase | Goal | Owner touched | Approval token | Rollback |
|---|---|---|---|---|
| **I1** | Freeze differential oracles from the current implementations (no source move) | tests only | *(none — test-only)* | delete the oracle test files |
| **I2** | Remove exact vendor/copy from Nagare (github vendored graph+clifford; non-git skeleton) | Nagare (github, skeleton) | `APPROVED-NAGARE-EDIT: retire-vendored-copies` | one revert; vendored tree is in git history |
| **I3** | (Conditional) canonical shared kernel in the owner — only the dense-linear-forward decision, if adopted | `hymeko_tensor` | `APPROVED-CORE-EDIT: tensor-dense-linear-forward` | revert; consumers still hand-roll |
| **I4** | Nagare re-point: depend on canonical `hymeko/graph` + `hymeko/tensor(clifford/spline)`; delete re-implemented forwards; keep backwards | Nagare (monorepo → Gitea target) | `APPROVED-NAGARE-EDIT: repoint-canonical-graph-tensor` | revert manifest + restore local forward |
| **I5** | hnn adapter/delegation: consolidate the 4× internal `dense_transform`; route `compute_bipartite_degrees` through the core seam | `hymeko_hnn` | `APPROVED-HNN-EDIT: internal-kernel-consolidation` | revert; behaviour is bitwise-checked |
| **I6** | Graph/HNN traversal: a shared borrowed-adjacency trait (no merge); tensor-internal Chebyshev primitive | `hymeko_graph`, `hymeko_hnn`, `hymeko_tensor` | `APPROVED-GRAPH-EDIT: borrowed-adjacency-trait` | revert; both keep concrete impls |
| **I7** | Redirect old production tests onto the public API | tests across repos | *(per-repo, test-only)* | revert test files |
| **I8** | Benchmark / allocation / RSS / dependency-DAG verification | all | *(measurement-only)* | n/a |

**Not in this plan (graph-internal, separate track):** collapsing the ~22
`enumerate_top_k_*` Cartesian surface and the `build_csr`/`bs_*` ×2 duplication inside
`hymeko_graph` (audit item 15). Real, but orthogonal to cross-repo consolidation;
raise under `APPROVED-GRAPH-EDIT: topk-config-struct` when the graph repo is worked.

---

## I1 — Freeze differential oracles (do this before any deletion)

Capture the current behaviour of every kernel that will be delegated or deleted, as a
**client of the public API**, so a later delegation can be proven bit-for-bit (or, where
a numerical contract legitimately changes, so the change is *recorded and accepted*, not
silent).

- **FIR spine oracle** (the load-bearing one): fix a corpus of `(TopKCyclesBatch,
  features)` and record `clifford_fir_forward` + `clifford_fir_backward` outputs from
  **both** the monorepo-fork spine and the canonical `hymeko_graph::spine`. If they
  disagree (they will — 537 differing lines, audit §5), the re-point is a numerical
  change → HARD STOP for an explicit "canonical is the new golden" decision with the
  digest recorded.
- **Spline oracle**: `catmull_rom_{forward,backward}` on a saturating probe (spans
  outside `[-1,1]` both sides) — Nagare port vs `hymeko_spline`. (Canonical already has
  a frozen clamp oracle in `hymeko_numeric_vv`; reuse it.)
- **Rotor oracle**: `cayley_rotor_{forward,backward}` Nagare vs `hymeko_clifford`
  (already bit-identity-certified 2026-08-28 — assert it still holds).
- **Message-passing / incidence oracle**: hnn `gather/scatter/clique` vs graph
  `hyper_operator` on the shared sub-domain (uniform-arity, f32) — this decides audit
  item 4 (category C if they agree, D if they don't).

These oracles are the acceptance gate for I4–I6.

## I2 — Remove exact vendor/copy from Nagare

- Delete `nagare_github/vendor/hymeko_graph` (15 421 LOC) and `vendor/hymeko_clifford`
  (776 LOC); replace the two path-deps with canonical crate deps (I4 wires them). Until
  I4 lands, the github instance may be pinned to the last vendored commit — do not leave
  it half-wired.
- Delete the non-git `…/nagare` skeleton (1 092 LOC) — it is a zero-commit fmt-snapshot
  of the monorepo's shared core (audit §4). If a thin standalone crate is still wanted,
  replace it with a facade that re-exports the monorepo owner, not a copy.
- **Deletable here (exact/stale): ≈ 17 289 LOC.** Nothing that Nagare actually *uses*
  is lost — the 4 consumed graph symbols and the clifford quats all exist canonically.

## I3 — (Conditional) dense-linear-forward owner

Only if the §8 decision adopts a canonical dense-linear owner: add
`hymeko_tensor::linalg::linear_forward(_into)` (+ optional backward) with a `*_into`
caller-owned output, proven by the I1 oracle to match the hnn and Nagare hand-rolled
forwards on their shared contract. **Default recommendation: skip I3**, do only the
hnn-internal consolidation in I5, and revisit when a second consumer appears (avoid the
speculative-abstraction trap).

## I4 — Nagare re-point (the migration's spine)

- Repoint monorepo `hymeko_nagare` (and the github instance) from the stale
  monorepo/vendored graph+clifford to the **canonical** `hymeko/graph` + `hymeko/tensor`
  (`hymeko_clifford`, `hymeko_spline`) via published-crate or path deps.
- Delete Nagare's re-implemented **forwards** that now have a canonical owner:
  `catmull_rom`/`chebyshev` (≈520 LOC → `hymeko_spline`), `cayley_rotor` forward
  (≈150 LOC → `hymeko_clifford`), and confirm `clifford_fir` is the canonical spine.
- **Keep** every Nagare closed-form **backward**, `NagareRuntime`, Adam, and holonomy —
  those are the owned band.
- Land on the Gitea `hymeko/nagare` target (branches already exist:
  `feat/graph-canonical-dependency`, `feat/phase-e-cascade-depth-retirement`).
- Gate: the I1 FIR oracle verdict must be recorded in the phase's summary JSON.

## I5 — hnn internal consolidation

- Extract the 4× row-major `X·Θ` (`hgnn`, `signed_hgnn`, `gcn_clique`, `mesh_nn`) into
  one private `dense_row_transform` helper — bitwise-identical (the accumulation order
  is already the same i→k→j); prove with the existing conv regression tests.
- Route `compute_bipartite_degrees` through the core `compute_degree_by_index` seam
  (add a `use_abs`/dual-output mode, or a `VertexSumBySign` `DegreeMode` for the
  Signed-HGNN per-sign degrees) — turns audit item 5 (B) into C.
- Fix the `mesh_nn` per-iteration `agg` allocation and add the missing traversal/mesh
  correctness tests flagged by the hnn card.

## I6 — Traversal & Chebyshev

- Introduce a borrowed-adjacency trait (`&self` neighbour iterator over caller-owned
  scratch) that both graph's `Csr` and hnn's `BergeView` implement; the BFS/DFS
  algorithm gets **one** production test. No behavioural merge (contracts stay distinct).
- Share one Chebyshev recurrence primitive within the tensor workspace between
  `hymeko_tensor::renyi::cheby_features` and `hymeko_spline` (audit item 12).
- Sweep the tensor `CORE.YAML`/`README` doc lag that still lists the retired
  `hymeko_graph` (`APPROVED-TENSOR-EDIT: sweep-retired-graph-docs`).

## I7 — Redirect old tests onto the public API

Every production test that reached into a now-deleted copy is rewritten to drive the
canonical public API (client-only; no new test framework). Coverage must not drop; the
new/redirected tests are the acceptance surface, not the diff.

## I8 — Verification

Run the full test & benchmark matrix (audit §7): Criterion A/B before/after each
delegation (alternating + A/A null), allocation counts, resident + peak construction
RSS, and a dependency-DAG check proving the target acyclic graph (audit §3) with **no**
forbidden edge. Amaterasu as a second host if available. Any `>3%` hot-path regression,
allocation growth, unjustified RSS growth, or numerical-contract change halts the phase.

---

## Deletable-LOC summary

| Category | Sites | LOC | Mechanism |
|---|---|---:|---|
| **Exact / stale-vendor** | github vendored graph+clifford; non-git skeleton | **≈ 17 289** | delete after re-point (I2/I4) |
| **Semantic (refactor, not raw delete)** | Nagare `catmull_rom` (~520) + `cayley_rotor` (~150); hnn `dense_transform` ×4 (~90 net); `compute_bipartite_degrees` (~40); tensor Chebyshev (~60) | **≈ 860** | delegate / consolidate (I4/I5/I6) |

LOC figures are estimates with the basis stated (per-file measurements in the audit).
The exact/vendor number is the high-confidence, high-value win; the semantic number is
a refactor budget, not a raw deletion.

## Approval-token ledger (proposed)

| Token | Phase | Scope |
|---|---|---|
| `APPROVED-NAGARE-EDIT: retire-vendored-copies` | I2 | delete vendored graph/clifford + non-git skeleton |
| `APPROVED-CORE-EDIT: tensor-dense-linear-forward` | I3 (optional) | new canonical dense-linear op in `hymeko_tensor` |
| `APPROVED-NAGARE-EDIT: repoint-canonical-graph-tensor` | I4 | manifest re-point + delete delegated forwards |
| `APPROVED-HNN-EDIT: internal-kernel-consolidation` | I5 | 4× dense_transform → 1 helper; degree seam routing |
| `APPROVED-GRAPH-EDIT: borrowed-adjacency-trait` | I6 | shared traversal trait; tensor Chebyshev primitive |
| `APPROVED-TENSOR-EDIT: sweep-retired-graph-docs` | I6 | CORE.YAML/README doc lag |

**HARD STOP:** this document is a plan. Execution of any phase requires the named
approval token and a green I1 oracle for the affected kernel.
