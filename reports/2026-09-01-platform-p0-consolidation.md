# P0 — platform evidence & branch consolidation

**GOVERNING-STRATEGY:** docs/hymeko-platform-strategy.md · **STRATEGY-ID:** HYMEKO-PLATFORM-2026-09-01-v1
· **ROADMAP-TASK:** P0. 2026-09-01 18:15 CEDT.
**APPROVED-CORE-EDIT:** reconcile-core-manifest-after-hnn-relocation (the CORE.YAML edit only).

Integration / governance / V&V / documentation only. No new algorithm, storage, transaction, GPU, MCU,
or WebXR feature was developed (strategy §6 forbidden list honored). The Strategy ID is **not** bumped —
this records implementation status, not an architecture change.

## What was consolidated to main (fast-forward only)
- **Core main → the p0 consolidation tip** (built on perf `584b9eb`, which contains the full webxr
  `fb6b783` and incidence `70b4f36` history) + three additive commits: the strategy roadmap, the CORE.YAML
  reconciliation, and this report. Clean fast-forward from `70b4f36`.
- **Graph main → `cbaadd4`** (graph performance survey, which contains direct-Ir `98c6a93`). Clean
  fast-forward from `5078980`. No content-modifying graph commit was made.

## Evidence-only (NOT merged)
- Core `feat/core-generational-arenas` `093eab3` — generational-arena deferral + architecture audit.
  Kept as decision evidence; **generational arena remains DEFERRED** (the best prototype showed +56%
  valid-lookup regression vs the append-only Vec and deletion does not yet exist; the next step is a
  deletion-capable storage study, strategy §4.2, roadmap P4).

## Governing status recorded (per strategy §11)
- **Standard graph path = direct canonical `Ir` + exact sparse `IncidenceIndex`** — proven: `direct_ir`
  uses `hymeko_core::ir::Ir` (7 files), defines **no** own canonical hypergraph model (0), materialises
  no clique/2-section on the standard path (the only `clique` mention in src is the doc-negation "no
  second hypergraph model"), and the flat-star/clique are **test-only benchmark controls** (18 in the
  bench, 0 in src). Index built once + reused (`ensure_capacity`/`begin`).
- **CBOR is not the main wire format** — the sectioned `ir_stream` snapshot is; the only CBOR artefact is
  a dormant unwired full-copy struct (MCU architecture doc §12).

## CORE.YAML reconciliation
Removed the stale `hymeko_hnn` crate declaration (the crate was relocated 2026-08-31 to hymeko/nagare;
Cargo.toml members = [hymeko_core, hymeko_wgpu]; the directory is gone). Corrected the scope list, the
compute-vocabulary rationale, and the hymeko_wgpu note to current ownership. **Unchanged:** hymeko_core
lockdown, pinned dependencies, all other entries. Diff: CORE.YAML only, +11/-15. Separate commit with the
approval token in its footer.

## Core V&V (p0 tip)
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings`: **clean**.
- `cargo test --workspace --all-features --no-fail-fast`: **532 passed / 0 failed / 4 ignored** (+7 vs
  the 525 seen without `--all-features`, which enables more test cfgs).
- doctests: **27 passed / 0 failed**.
- Named modules (from the run): webxr 27, incidence 13, ir_stream 88, namespace 8, hierarchy 12, canonical
  hash KAT present.
- Benchmark targets build (smoke, `--no-run`, no numbers published): ir_bench, ir_stream_bench, csr_bench,
  interning_bench, spmm_bench, chain_bench, arena_bench — all built.
- **57/58 `meta_geometry` drift:** established to **not exist** in the current core workspace
  (`cargo test meta_geometry` → 0 tests, all filtered) — it is a stale monorepo-era reference in
  CORE.YAML's comment; nothing to touch or hide. (Not "fixed" — the fixture/test simply is not here.)
- Coverage: P0 touches **zero production lines** (docs + CORE.YAML manifest only), so touched-production-
  line coverage is vacuously satisfied; the module rates are unchanged from the cited artefacts (webxr
  95.3% lines / 100% fn, contract-completion report). `cargo llvm-cov` is installed but re-running it would
  reproduce the existing numbers since no code changed — cited as the coverage of record.

## Graph V&V (cbaadd4)
- fmt clean; `clippy -p hymeko_graph --all-targets -D warnings`: **clean** (the touched crate).
- `cargo test --workspace --all-features --no-fail-fast`: **443 passed / 0 failed / 6 ignored**; doctests 2.
- direct-Ir suite (vv/optional/families/coverage/alloc): 27 passed / 0 failed, incl. serial==Rayon parity,
  duplicate/self-incidence/sign, malformed-state faults.
- **Pre-existing failure, precisely identified (NOT introduced, NOT fixed here):**
  `cargo clippy --workspace --all-features` fails 2× `clippy::explicit_counter_loop` in
  `hymeko_pgraph/src/pgip_io.rs:368` (`next_id`) and `:389` (`next_uid`). `pgip_io.rs` is **unchanged since
  graph main `5078980`** (the perf survey touched 0 pgraph files), so this predates the consolidation and
  the FF does not introduce it. It surfaces only under `--all-features` (prior phases ran `-p hymeko_graph`).
  Left untouched per the phase's no-fix rule; a `bench-harness-consolidation` / lint-cleanup follow-up.

## Fault-injection evidence (no mutant committed; hash-verified restore)
Representative real mutation: short-circuiting `IncidenceIndex::validate` to `Ok(())` made **4 of 5**
incidence fault tests FAIL (`duplicated_coordinate`, `out_of_range_slot`, `swapped_coordinate_keeping_degree`,
`truncated_row`), proving they catch bad-`(arc,slot)` / target-mismatch / missing-duplicate / dangling
corruption. File restored sha256-identical, git clean, tests green again (5/5). The other fault classes are
covered by named tests that ran green: WebXR cycle/cardinality/invalid-numeric (16 negatives), ir_stream
offset/length overflow + reject (88), graph serial/Rayon parity, and module/bench-target registration.

## Own mistakes / retracted assumptions
- First ran the incidence fault mutation against the integration binary (`--test mod`) where those lib
  unit tests do not live (0 matched); corrected to `--lib`, where the catch is demonstrated.
- No architecture claim changed; the strategy's §11 retractions (HyperGraphView-standard-path, CBOR-next-
  codec, generational-arena-migration, Bᵀy≈10×, clique≠dense-star) stand and are the recorded status.

## Next phase (P1) entry conditions
`benchmark-harness-consolidation`: dedup `SparseSpec`/`Lcg` (core `benches/sparse_fixture.rs` vs
`wgpu/src/testing.rs`), the CPU-SpMM double (`csr_bench` vs `spmm_bench` A/B), the three edge→arc→ref
generators, and the cross-repo Steiner/sunflower (reuse `hymeko_core::generators`), with every old KAT and
result identical after; and clear the pre-existing `pgip_io.rs` `explicit_counter_loop` lint under
`--all-features`. Enter only with core+graph mains at the consolidated SHAs (this report) and the strategy
on main.

## Provenance / HARD STOP
rustc 1.97.1, x86_64-pc-windows-msvc. Refs, ancestry, and remote verification in the return summary.
Nagare and other siblings unchanged; the user-owned dirty checkout untouched. **HARD STOP** after promotion.
