# Canonical reverse-incidence navigation — report

**APPROVED-CORE-EDIT: canonical-ir-incidence-navigation**
2026-09-01 01:15 CEDT · branch `feat/core-incidence-navigation` · base `3190097` (core main after U2.2)

## What shipped

The canonical `Ir` gains the missing **node → incidence** direction so
`hymeko_graph` can traverse the hypergraph **directly**, with no materialised
SignedGraph / CSR / dense / star / clique view:

- `ir::incidence::IncidenceIndex` — a compact **CSR** reverse index
  (`DeclId → incident HyperArcIds`);
- `build_incidence_index(&Ir) -> Result<IncidenceIndex, IncidenceError>` — two
  passes, `O(incidences)`, one allocation per CSR array;
- `IncidenceIndex::incident_arcs(node) -> &[HyperArcId]` (+ `degree`,
  `incidence_count`, `bytes`) — total, allocation-free, never panics;
- `index_matches_ir(&Ir, &IncidenceIndex) -> bool` — opt-in coherence gate.

The index is **lossless** (sign/weight reachable through the arc's `refs`;
duplicate incidences preserved — an arc referencing a node *k* times appears *k*
times), **canonical-ID in and out**, with **no** pairwise/star/clique/Levi
expansion and **no** implicit dedup or flattening. It is **derived, caller-owned,
built once and reused** — never rebuilt per query, never added to the serde wire,
so it cannot silently diverge from the `Ir`.

## Stage 0 — core promotion

Core `main` fast-forwarded `67771c9 → 3190097` (the U2.2 flip + a formatting
follow-up `style(tests)`; gates green). This is the base for both this edit and
the pulled `hymeko_core` dependency in `hymeko_graph`.

## Stage 3 — reference benchmark (the decision)

`proto_incidence_bench.rs` models the canonical incidence and measures
Berge-reachability three ways so the **full** cost is visible (build + search +
memory), not just resident kernel time. Direct-Ir reverse index (**REV**) vs
materialised star-expansion CSR (**DENSE**), host x86_64 windows, rustc 1.97.1:

| workload | REV build | DENSE build | REV bytes/inc | DENSE bytes/inc | end-to-end |
|---|--:|--:|--:|--:|--:|
| uniform-small (100k inc, arity 5) | 458 ns | 1082 ns (2.4×) | 4.8 | 16.8 (3.5×) | REV 1.3× |
| uniform 1e6 (arity 5) | 16.4 µs | 24.9 µs (1.5×) | 12.0 | 24.0 (2.0×) | REV 1.5× |
| hub-skew 1e6 | 16.1 µs | 26.0 µs (1.6×) | 12.0 | 24.0 (2.0×) | ≈1.0× (tie) |
| **high-arity 1e6 (arity 20)** | 7.8 µs | 55.3 µs (**7.1×**) | 6.0 | 78.0 (**13×**) | REV **2.3×** |
| no-index scan (20k arcs) | — | — | — | — | **~650× REV** (O(V·E)) |

**Verdict:** the reverse index is justified — cheaper to build (1.5–7×), far less
memory (2–13×), lossless, faster end-to-end (up to 2.3×), decisively so at high
arity where star expansion is quadratic. Pure-search time is at parity (DENSE can
win narrow reachability, REV wins high arity), so the win is build + memory +
losslessness, stated honestly. The no-index scan (650× slower) confirms
node-major traversal **needs** the index.

## Stage 5 — why derived, not stored

The current `Ir` is append-only (no mutation/transaction), so a stored index would
only duplicate `O(incidences)`-derivable data and add serde/consistency burden for
no benefit; the algorithm layer builds it once per `Ir` and reuses it across
queries. The transaction-updated stored form for a future *mutable* IR is the
next step (Stage 14), not this phase — consistent with the brief's "no separate
mutable state" and "not rebuilt per query" rules.

## V&V

- **Golden** (`tests/…/incidence.rs`): multiway arc with +/−/~; duplicate
  incidence preserved; self-reference + multiple arcs per edge; same node in many
  hyperedges; empty + singleton.
- **Independent oracle**: an `O(V·E)` scan that does **not** use the index;
  `incident_arcs` equals it exactly (order + multiplicity) on every golden shape,
  and `index_matches_ir` holds.
- **Metamorphic**: reverse-index BFS reachability ≡ edge-major scan BFS from every
  start on an 8-node/3-arc graph.
- **Fault**: a dangling target → `IncidenceError::TargetOutOfRange` (exact
  variant, named catcher).
- Suite: **112 lib + 210 integration + 7 doctests** green; fmt + clippy
  `-D warnings` clean.

## Materializations avoided

The index replaces, on the standard path, the construction of: SignedGraph CSR,
dense/star pairwise adjacency, clique/Levi expansion. None is built as a search
precondition. `bytes/incidence` measured: 6–12 B (REV) vs 16.8–78 B (dense).

## Own mistakes / retracted hypotheses

- I initially expected the reverse index to also **search** faster than a dense
  CSR; the benchmark showed search parity (dense can win pure reachability). The
  index's real, honest win is build cost + memory + losslessness + end-to-end —
  reported as measured, not as a search-speed claim.
- The `3a0ad1e` promotion carried one unformatted test file; caught by the fmt
  gate at promotion and fixed with a `style(tests)` follow-up before rolling main
  forward.

## Scope not covered this phase (honest)

The `hymeko_graph` **direct-Ir algorithm suite** (Stage 4–11: `berge_bfs`,
`reachable_nodes`, `connected_components`, `s_connected_components`,
`enumerate_berge_paths/cycles`, `dual_reachability`), the full 11-workload
benchmark matrix vs the live `HyperGraphView`, the SignedGraph/SignedHyperIncidence
retirement, and the graph/Hotaru/Nagare ownership map are a **separate repo and
approved edit** (`graph-direct-ir-algorithms`) that depends on this core API being
on `main`. This report delivers the load-bearing foundation (the incidence
navigation the brief said is the only thing that belongs in the canonical Ir API)
+ the decisive direct-vs-materialization evidence; the algorithm layer is the
next phase, now unblocked.

## Return-report items (this phase)

1. **Commit SHAs** — below (at push).
2. **Ir traversal API** — `build_incidence_index` + `IncidenceIndex::incident_arcs`
   (node-major), atop the existing edge-major `EdgeRec.arcs`/`ArcRec.refs`.
3. **Removed/avoided materializations** — dense/star/clique CSR as a search
   precondition; measured 2–13× memory saved.
4. **Reverse-incidence representation + memory** — CSR (u32 row + HyperArcId col),
   6–12 bytes/incidence, lossless, derived.
5. **Algorithms** — none in core (navigation only, per the brief); the graph
   suite is next.
6. **Canonical-ID results** — `incident_arcs` returns `HyperArcId`s; targets are
   `DeclId`s; no dense relabelling leaks.
7. **Tests/coverage** — 112 lib + 210 integration + 7 doctests; the incidence
   module fully exercised (golden + oracle + metamorphic + fault).
8. **Fault-injection** — `TargetOutOfRange` caught by a named test.
9. **Benchmark** — direct REV vs dense CSR above (vs the live `HyperGraphView`:
   next phase).
10. **Peak RSS / allocation** — index build is one alloc per CSR array;
    `incident_arcs` is allocation-free.
11. **Parallel scaling** — N/A this phase (algorithms next).
12. **Duplication removed** — N/A (new module; no algorithm bodies yet).
13. **Own mistakes** — above.
14. **Next: dynamic transaction index update** — when the mutable/transactional IR
    lands, promote `IncidenceIndex` to a stored index rebuilt/patched at
    transaction commit, coherence-validated, so repeated node-major queries need
    no rebuild.

## Provenance

Branch `feat/core-incidence-navigation` @ base `3190097`; rustc 1.97.1,
x86_64-pc-windows-msvc.
