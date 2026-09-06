# P2 — core incidence operator precompute (prepared Bᵀ)

**STRATEGY-ID:** HYMEKO-PLATFORM-2026-09-01-v1 · **ROADMAP-TASK:** P2 · **APPROVED-CORE-EDIT:**
incidence-operator-precompute. 2026-09-02. Branch `feat/core-incidence-operator-precompute` @ `903c519`
(remote-verified); **not merged** (core main = `5fe2c88`). Additive; no `Ir` layout / wire / dependency
change; no dense/star/clique/second model.

## 1. Problem — profiled/isolated BEFORE implementing (§4.1)
Hypothesis: the node-major `Bᵀy` (`arc_to_node`) re-resolves each incidence's `(arc,slot)` (a node-major →
arc-major double indirection into the fat `SignedRefR`), `sign()` (enum match) and `scalar_weight` (Option +
`Vec<ValueR>` fold) on **every** call. **Isolation test** (`bench_operator_plan_decision_gate`, weighted
arity-5 window, ~1e6 incidences): current `Bᵀy` **13.3 ms** vs a prepared coefficient version **0.68 ms** —
**~20×**, bitwise-identical. The hypothesis holds decisively; the cache is justified (it would not have been
built on a null result).

## 2. Target architecture (§4.2)
`IncidenceOperatorPlan<'idx,'ir>` — a caller-owned precompute:
- **borrows** `&IncidenceIndex` (hence the `Ir`); **not** part of the canonical `Ir`; does **not** serialize;
  **cannot pair with another `Ir`/index** (the borrow enforces it at compile time);
- stores **exactly one `f64` per incidence** (`sign · scalar_weight`) in node-major order;
- **copies no topology or `(arc,slot)` coordinate** — the arc index per incidence is read from the borrowed
  index;
- `prepare_operator()` type-validates (rejects non-finite coefficients); a warm apply is **allocation-free**.
- Only the expensive `Bᵀ` orientation is prepared; `Bx` (`node_to_arc`, arc-major) is **not** slowed or
  precomputed (measured 10.5 ms and left untouched — §4.2's "don't precompute two orientations unless
  measured").

## 3. Semantics (§4.3)
The coefficient is `f64::from(sref.sign()) * scalar_weight(sref)` — preserving sign, duplicate incidence
(one coefficient per incidence, none merged), none-weight → 1, the documented `Num`-product weight policy,
non-numeric components per the current policy, and `-0.0`. `NaN`/`±∞` are **rejected at prepare** (a
deliberate contract stricter than the un-prepared operator, which propagates them) — `OperatorPlanError::
NonFiniteCoefficient`. Deterministic node-major fold order. `node_to_arc = Bx`, `plan.arc_to_node = Bᵀy`,
`plan.star_operator = BᵀBx`. Serial and Rayon partition the output by node (disjoint, **no atomics**), so
they are **bitwise-identical** to each other and to the legacy operator for finite weights.

## 4. Tests (§4.4)
`incidence_precompute_profile` (wired into `tests/mod.rs`; `autotests=false`): **prepared == legacy ==
independent dense-Bᵀ oracle** across `+/-/~`, none/single/multi/non-numeric weight, a duplicate incidence,
`-0.0`; **serial == Rayon** bitwise; empty / single self-incidence; irregular + high-cardinality (one arc
over 64 nodes); `NaN`/`±∞` rejected at prepare; the decision-gate bench asserts bitwise `Bᵀy` identity
inline. **Plan provenance:** pairing with another `Ir` is a **compile error** (the `'idx`/`'ir` borrow),
proven by construction, not a runtime test. **Fault sweep** (no mutant committed; git-clean restore):
**sign-drop** and **node-major-reorder** caught by the bitwise parity; weight-drop is the same
coefficient-error class the same assertion catches. (On-record mistake: a mid-sweep `git checkout` reverted
the *uncommitted* plan — recovered and re-applied; the plan is now committed before any further sweep.)

## 5. Benchmark & decision gate (§4.5)
Weighted arity-5, ~1e6 incidences, median-of-9, single host PROVISIONAL:

| quantity | value |
|---|--:|
| legacy Bᵀy | 13.3 ms |
| **prepared Bᵀy** | **0.68 ms (≈20×)** |
| Rayon Bᵀy | 0.13 ms (≈5× more) |
| legacy BᵀBx | 23.8 ms |
| **prepared BᵀBx** | **11.7 ms (2.0×)** — Bᵀ half; Bx floor 10.5 ms |
| Bx (untouched) | 10.5 ms |
| prepare cost | 15.3 ms |
| **break-even** | **≈1.2 applies** |
| retained | **8 bytes/incidence** |

**Acceptance:** exact output bitwise-identical ✓; Bx regression **0%** (untouched) ✓; prepared warm apply
allocation-free ✓; memory overhead **8 B/incidence** (reported); no topology duplication ✓; `Bᵀy` **≥1.5×**
(measured ≈20×) ✓; break-even **≈1.2 applies**; the 8 B/incidence overhead is repaid after ~2 applies, so
the plan is **recommended as an opt-in prepare** (caller-owned; prepare once, reuse) — not forced on
single-shot `Bᵀy`. f32 is out of the current f64-only operator scope (noted, not built).

## 6. Own mistakes / retracted
- Retained the **profile-first** discipline: measured before implementing (would have stopped on a null).
- The `git checkout` fault-restore footgun recurred (reverting the uncommitted plan) — recovered; lesson
  re-applied (commit before checkout-restore sweeps).
- The un-prepared `Bx` was deliberately **not** optimized (measured not to be the bottleneck) — avoids the
  §4.2-forbidden two-orientation precompute.

## 7. Next minimal phase
`incidence-operator-plan-adoption`: wire `prepare_operator` into the repeated-`Bᵀy` call sites (iterative
solvers / spectral) behind the opt-in, measure the end-to-end win, and evaluate an f32 coefficient variant
if an f32 operator path lands. Promotion of any of the overnight feature branches is a separate,
FF-only decision.

## Provenance / HARD STOP
rustc 1.97.1, x86_64-pc-windows-msvc. Core V&V: fmt clean; clippy `--workspace --all-features --all-targets
-D warnings` green; workspace tests **543/0/4** (+5 plan tests); doctests 5/0. Branch pushed + verified,
**not merged**. CORE.YAML unchanged (additive under the token; no pinned-dep/lockdown change). **HARD STOP.**
