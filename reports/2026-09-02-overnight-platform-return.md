# Overnight platform mission — morning return

**STRATEGY-ID:** HYMEKO-PLATFORM-2026-09-01-v1. 2026-09-02 02:01 CEST. Phases P1.2-promotion → P1.3 → P1.4
→ Phase-3 audit → P2, each on its own branch/commits; only the P1.2 promotion touched main. Nagare/Hotaru/
tensor/sibling repos untouched; no licence/dependency/toolchain work; user dirty checkout untouched (clean
scratch worktrees). No second hypergraph model, dense view, materialised star/clique/Levi, new algebra
framework, new CLI, or snapshot/transaction/IRDelta system was created.

## 1. Commit SHAs + remote verification
| branch | tip | remote == local | merged to main? |
|---|---|:--:|:--:|
| graph `main` | **`9655f07`** (P1.2, FF-promoted d4086a0..9655f07) | ✓ | — |
| graph `feat/pgraph-adjacency-delegation` (P1.3) | **`b234c48`** | ✓ | no |
| graph `feat/graph-query-performance-matrix` (P1.4) | **`e710e84`** | ✓ | no |
| core `feat/core-incidence-operator-precompute` (P2) | **`903c519`** | ✓ | no |
| core `main` | **`5fe2c88`** (unchanged) | ✓ | — |

## 2. Main / feature refs
graph main = `9655f07` (P1.2, the only promotion, fast-forward-only). core main = `5fe2c88` (untouched).
P1.3/P1.4/P2 are feature branches, pushed + verified, **not merged**.

## 3. Removed topological duplication (P1.3)
`axioms::build_forward_adj`, `axioms::build_producers`, and the **verbatim** `axiom_extensions::{build_forward_adj,
reaches_any}` — all deleted; one shared `axioms::reaches_any(schema,…)` (BFS over the single `schema.successors`
adjacency); `check_a2` uses `schema.predecessors`. **Net −68 LOC**. `schema.incoming/outgoing` kept as the
single non-duplicated source (its full removal needs `LoweredPGraph` to borrow the `Ir` — deferred).

## 4. PGraphView ownership & retained memory
`PGraphView<'a,'ir,'idx>` borrows `&IncidenceIndex` + classification sets; **zero owned topology**; all
adjacency/degree/directed-reachability from the incidence. Retained topology memory **unchanged** this
overnight (schema kept); the removed cost is per-`validate` **rebuild allocations** (two `BTreeMap`s +
`O(edges)` nodes). `IncidenceOperatorPlan` (P2) retains **8 bytes/incidence** (coefficient only, no topology).

## 5. Exact output / dump / .pgip / MSG/SSG/ABB parity
P1.3: full pgraph suite **235/0/3** — six dump hashes, `.pgip` round-trip, MSG/SSG/ABB, book validation,
BooleanExact closure all **byte/semantically identical**; `PGraphView` reachability/degree == schema.
P1.4: `AxiomBundle::validate` exact output identical A (P1.2) vs B (P1.3). P2: prepared `Bᵀy`/`BᵀBx`
**bitwise-identical** to the legacy operator (finite weights).

## 6. Graph operation latency
P1.4 `AxiomBundle::validate` A/B (A=P1.2 rebuilds, B=P1.3 delegated): B faster on chain (5–15%) and OR
fan-out (60–66%), provisionally slower on AND fan-in (18–75%, single-host / no A/A-null ⇒ not a proven
regression). The graph/search ops (reachable/components/Berge/Dijkstra/A*) are P1.1-measured and
P1.3-invariant (unchanged code).

## 7. A*/BooleanExact/PGRAPH time-to-first
BooleanExact closure (P1.2): chain-large 302 µs, ~O(n); vs `AxiomBundle::validate` 1.14 s (A4 per-O-node
reachability, ~O(n²) on a chain) — different operations (connectivity ≠ producibility). A* time-to-first
(P1.1): the optimal goal leaves the queue at completion; 38–147× fewer expanded vs Dijkstra on a metric grid.

## 8. Incidence operator prepare/apply latency (P2)
~1e6 incidences: legacy `Bᵀy` 13.3 ms → **prepared 0.68 ms (≈20×)**; Rayon 0.13 ms (≈5× more); prepared
`BᵀBx` 11.7 ms (2.0×; `Bx` floor 10.5 ms untouched); prepare 15.3 ms; **break-even ≈1.2 applies**.

## 9. Bytes/incidence & peak RSS
P2 plan: **8 bytes/incidence** (one `f64` coefficient; no topology copied). Peak RSS for every measured cell
(1e6-incidence `Bᵀ`, 4000-material closure) ≪ 16 GiB.

## 10. Allocation
P1.2 AND/OR warm loop: **0 allocations**. P2 prepared apply (`Bᵀy`/`BᵀBx`): **0 allocations** warm. P1.3
removed the per-`validate` rebuild allocations.

## 11. Serial/Rayon agreement & scaling
P2 `par_arc_to_node` is **bitwise-identical** to serial (node-partitioned output, no atomics); ~5× the
serial-prepared throughput at 1e6 incidences.

## 12. Coverage
`cargo llvm-cov` is not installed on this host (tools.yaml). Per the mission, **no install, no estimate →
UNMEASURED**. Correctness rests on the bitwise-parity + independent-oracle + fault-injection suites instead
(all green), which exercise every new public function.

## 13. Fault mutants (no mutant committed; git-clean restore)
P1.3 (`view.rs`): input-sign swap, producer/consumer swap, dropped slot, wrong arc-owner, reachability-
direction — all caught; dedup + compat-field-as-authoritative structurally guarded. P2 (`incidence.rs`):
sign-drop, node-major-reorder caught by the bitwise parity; weight-drop same class. (A `git checkout`
reverted the uncommitted P2 plan mid-sweep once — recovered + committed before continuing.)

## 14. Own mistakes / retracted hypotheses
- The P2 `git checkout` fault-restore footgun (reverting uncommitted work) recurred from P1 — recovered;
  now commit-before-sweep.
- P1.3 reachability-parity test first mismatched on start-inclusion (`forward_reachable` includes the start,
  the schema BFS excludes it) — fixed by excluding the start on both sides.
- P1.4 AND-fan-in showed B provisionally slower — reported honestly as PROVISIONAL (no A/A control), not
  hidden.
- P2: profiled first and the hypothesis held (≈20×) — no unjustified cache; `Bx` deliberately not optimized.

## 15. Implemented vs rejected abstractions
**Implemented:** one shared `reaches_any` (P1.3); the `IncidenceOperatorPlan` (P2, justified by profile).
**Rejected/deferred (recorded, not opportunistically refactored):** the full `schema.incoming/outgoing`
removal (needs `LoweredPGraph<'ir>` borrowing the `Ir`); a generalized visited-generation workspace shared
across the graph/pgraph boundary; a shared `<isa>` bench-source helper; an f32 operator-coefficient variant.
Each is a next-phase proposal requiring its own parity + perf proof.

## 16. Next minimal roadmap phase
`pgraph-lowered-borrows-ir`: lifetime-parameterize `LoweredPGraph<'ir>` to borrow the `Ir`+`IncidenceIndex`,
remove `schema.incoming/outgoing` (delegate all adjacency to `PGraphView`), gated on the same byte-identical
dump/pgip/MSG/SSG/ABB re-verification — the change that reduces *retained* topology memory (P1.3 removed
rebuild *allocations*). Then `incidence-operator-plan-adoption` (wire P2's prepare into repeated-`Bᵀy` call
sites behind the opt-in). Promotion of any overnight feature branch is a separate, **fast-forward-only**
decision.

## Provenance / HARD STOP
rustc 1.97.1, x86_64-pc-windows-msvc. Both repos: fmt clean; clippy `--workspace --all-features --all-targets
-D warnings` green; graph pgraph suite 235/0/3; core workspace 543/0/4 + doctests 5/0. All feature branches
pushed + remote-verified, none merged to main. Remote benchmark worktrees / build trees preserved. **HARD
STOP after this report.**
