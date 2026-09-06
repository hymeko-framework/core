# Part 2 — incidence operator plan adoption + Bx profile

**STRATEGY-ID:** HYMEKO-PLATFORM-2026-09-01-v1 · **APPROVED-CORE-EDIT:** incidence-operator-plan-adoption.
2026-09-02. Branch `feat/incidence-operator-plan-adoption` @ base P2 `52c2649`; **not merged** (core main
`5fe2c88`).

## 1. Adoption finding — no consumer to adopt (verified)
A grep across **every** crate (`hymeko_core`, `hymeko_wgpu`, `hymeko_graph`, `hymeko_pgraph`) shows the
**only** callers of `node_to_arc`/`arc_to_node`/`star_operator`/`par_arc_to_node`/`prepare_operator` are the
operator methods themselves (`star_operator` → `node_to_arc` + `arc_to_node`). **There is no repeated
Bᵀy/BᵀBx production consumer** (no spectral / power-iteration / iterative solver) to adopt the prepared plan
into. Per the operating contract (search-before-claiming; do not fabricate a consumer), the P2
`IncidenceOperatorPlan` therefore stays a **ready opt-in library primitive** — the single-shot operators do
not pay the ~15 ms prepare, and a future repeated-Bᵀy consumer will `prepare_operator()` once outside its
loop and reuse it (break-even ≈1.2 applies, 8 B/incidence, bitwise-identical, serial==Rayon — all proven in
P2). **No production code was changed** (there is nothing to wire).

## 2. Bx profile (§4) — measured, second array NOT built
After the Bᵀ precompute, `Bx` (`node_to_arc`, arc-major) dominates `BᵀBx`. `bx_profile` (~1e6 incidences):

| variant | ms | note |
|---|--:|---|
| (A) current Bx | 9.3 | resolves `sign()`/`scalar_weight` per incidence |
| (B) prepared arc-major coeff | 4.6 | **2.0×**; still reads `target` from the ref (no topology copy) |
| (C) scatter floor (coef=1) | 3.6 | memory-traffic bound (scattered `x[target]`) |

**Decomposition:** 62% of Bx is `sign()`/`scalar_weight` resolution; 38% is the scatter/traversal floor. So a
**second, arc-major coefficient array** would give Bx ~2× (and `BᵀBx` toward ~2× more). **Trade-off:** it
**doubles** the plan's memory (8 → 16 B/incidence). **Decision (mission §4, "no second array without
measurement"):** with the measured 2× gain, +8 B/incidence cost, and **no consumer** to justify either, the
second array is **not built**. The measured trade-off is recorded so a future `BᵀBx` consumer can adopt it as
an **opt-in** (`prepare_operator_both` / an arc-major coefficient) with a stated break-even. `Bx` is left
untouched (0% regression), honoring §4.2's "do not slow the fast Bx path / do not precompute two orientations
unless measured + justified".

## 3. Own mistakes / retracted
- No consumer was invented to create adoption work; the honest finding (zero repeated consumer) is reported
  as the result.
- The 2× Bx precompute is real but deliberately **not** shipped (no consumer + memory doubling) — measured,
  documented, deferred as opt-in rather than built speculatively.

## Provenance / HARD STOP
rustc 1.97.1. Core V&V: clippy `-D warnings` green; the Bx profile passes (wired into `tests/mod.rs`,
`autotests=false`). No production `hymeko_core` API changed (the P2 plan is unchanged; only a profile test
was added). Branch pushed + verified, **not merged**. **HARD STOP.**
