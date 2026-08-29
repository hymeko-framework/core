# Unified IR — coverage baseline and delta

**Measured:** 2026-08-29 (M2b) · **Tool:** `cargo-llvm-cov 0.9.0` · **Toolchain:** `rustc 1.97.1 (8bab26f4f 2026-07-14)` stable, `llvm-tools-x86_64-pc-windows-msvc`

`tools.yaml` pins `cargo llvm-cov` at `major_version: "0"`, previously recorded as *not installed*. 0.9.0 satisfies the pin. Installed with `cargo install cargo-llvm-cov --locked --version "^0.9"` from a temporary directory **outside** the parent checkout, because that tree's `.cargo/config.toml` replaces crates.io with a vendored directory. **No manifest, lockfile, vendor directory or `tools.yaml` entry was modified.**

## Commands

```bash
cargo llvm-cov --workspace --all-features --lcov \
  --output-path reports/coverage/<name>.lcov \
  -- --skip test_load_meta_geometry --skip test_coo_construction_throughput

awk -v mode=normalize -f scripts/coverage_summary.awk in.lcov > out.lcov   # strip host paths
awk -f scripts/coverage_summary.awk out.lcov                              # grouped summary
awk -v group="hymeko_core/src/ir/" -f scripts/coverage_summary.awk out.lcov
awk -v mode=summary -v perfile=1 -f scripts/coverage_summary.awk out.lcov  # per-file
```

## Denominator and exclusions

`cargo llvm-cov` instruments **workspace members only**. The emitted report contains 77 `SF:` records, every one under `*/src` — no test, bench, example, target or vendor file appears at all. This was verified by classifying every record, not assumed:

```
classify(p): /tests/ → test | /benches/ → bench | /examples/ → example
             /target/ → generated | /vendor/ → vendor | /src/ → production
```

The summary prints one row per category. Only `production` is ever non-empty; any leakage would appear as a second row.

> **No production source is excluded.** Not one path is filtered to improve a number.

Absolute host paths are stripped by `scripts/coverage_summary.awk -v mode=normalize`, so the committed LCOV is reproducible on any machine (`SF:core/hymeko_core/src/...`).

## Two skips during profile collection — both by command line, neither in source

| Test | Reason |
|---|---|
| `test_load_meta_geometry` | The known 57/58 fixture drift. **It continues to run and fail, byte-identically, in the normal gate.** Skipped only so that one failing assertion does not abort profile collection. No source-level `#[ignore]`, no assertion change, no fixture change. |
| `test_coo_construction_throughput` | A **timed** assertion (`5M pushes must complete well under 1s`). Coverage instrumentation adds a counter to every branch, so a throughput threshold is not merely slower under it but meaningless. Measured 505 ms instrumented against a tighter budget. It passes in the uninstrumented gate. |

The second skip is an extension of what was authorised for the first, on a different and stated ground. Recording it rather than quietly widening the flag.

## Branch coverage: not available

Every `BRF` record is `0` and there are **no `BRDA` records at all**. This is not "no branches covered" — it is *no branch data emitted*.

`cargo llvm-cov --branch` exists but is marked **`(unstable)`** and requires a nightly toolchain. This host runs stable 1.97.1, and changing the toolchain is outside this phase's authorisation.

The summary prints `n/a`, never `0.00%`, so the distinction survives into the report. **No estimate is substituted.**

## Results

### Workspace production

| | Lines | Hit | Line % | Fns | Hit | Fn % | Branch |
|---|---:|---:|---:|---:|---:|---:|---:|
| baseline | 11 070 | 9 199 | **83.10 %** | 1 183 | 954 | 80.64 % | n/a |
| after | 11 070 | 9 371 | **84.65 %** | 1 183 | 973 | 82.25 % | n/a |
| Δ | | | **+1.55 pp** | | | +1.61 pp | |

Acceptance was "must not fall by more than 0.5 pp". It rose.

### Unified IR (`hymeko_core/src/ir`, 10 files)

| | Lines | Hit | Line % | Fns | Hit | Fn % |
|---|---:|---:|---:|---:|---:|---:|
| baseline | 1 125 | 1 022 | **90.84 %** | 112 | 95 | 84.82 % |
| after | 1 125 | 1 022 | **90.84 %** | 112 | 95 | 84.82 % |

Unchanged — acceptance was "must not decrease".

### Touched files

| File | Baseline | After |
|---|---:|---:|
| `util/pretty_print.rs` | **0.00 %** | **89.86 %** |
| `writers/ir_writer.rs` | **0.00 %** | **97.50 %** |
| `hymeko_hnn/.../hypergraphview.rs` | 90.00 % | **94.08 %** |
| `ir/access.rs` | 94.64 % | 94.64 % (unchanged; byte-identical) |

> **Both migrated core consumers had zero baseline coverage.** Neither `pretty_print.rs` nor `ir_writer.rs` had ever been executed by any test. The M2b edits would have landed in code nothing checked. Reading the per-file report *before* editing is what surfaced that; the aggregate number would not have.

### Unreached touched line, recorded rather than excluded

`util/pretty_print.rs` — the `(arc_rec missing)` fallback inside `fmt_edge_arcs`.

Reaching it needs a `CompiledProgram` whose `Ir` names a non-existent arc. `CompiledProgram` is neither `Clone` nor constructible from a test (`ModuleKey` and `HashId` do not implement `Default`), and the only public entry point takes `&CompiledProgram`. Adding `Clone`/`Default` to production types **solely to make a test compile** would be the test dictating the design.

It is a defensive branch on a debug printer. The equivalent branch in `IrWriter` **is** covered (`writer_survives_a_dangling_child_link`), because that consumer takes `&Ir` and can be handed a corrupted one directly.

The line stays in the denominator.

## Requirements coverage

From `traceability.csv`, 44 requirements.

| Measure | Value |
|---|---|
| **Implemented-requirement coverage** — VERIFIED ÷ (VERIFIED + PARTIAL) | **31 / 32 = 96.9 %** |
| **End-state coverage** — VERIFIED ÷ all | **31 / 44 = 70.5 %** |
| PARTIAL | 1 (`UIR-U4-04`, `total_cmp` — not violated, not yet exercisable) |
| PLANNED | 5 (M2e authority flip, removal-by-ID, weight finiteness, dual identity) |
| BLOCKED | 10 (`UIR-DYN-*`, all requiring M3/M4) |

The two ratios are reported separately on purpose. End-state coverage is deliberately low and **should stay low** until M3/M4 land. A rising end-state number without new implementation would mean requirements had been dropped, not met.

## Why this is not validation

Line coverage says a line executed. It does not say the result was checked, or that a wrong result would have been noticed.

M2b's fault injection makes that concrete. Four defects were planted in `ir/access.rs`, a file at **94.64 %** coverage:

| Defect | Caught by |
|---|---|
| back-link validation removed | 2 tests |
| child iterator loses its last element | 4 tests |
| map-only and checked-composite conflated | **1 test** — and that one needed the corpus present |
| iterator ids shifted by one | 5 tests |

Coverage was identical before and after each mutation, and located none of them. The third was weak enough to justify a focused guard, which was added and the mutation re-run (now 2 tests, one corpus-independent).

**Requirements coverage is the primary measure here. Line coverage is a secondary check for *unreached* code — a different and lesser question.**

---

## Correction — 2026-08-29 (M2b V&V corrective)

Appended, not edited in place, so the earlier error stays visible. Full account in
`reports/2026-08-29-unified-ir-m2b-vv-corrective.md`.

### The requirements-coverage rates above are retracted

**96.9 %** and **70.5 %** are withdrawn. Their denominator was `VERIFIED + PARTIAL` over a row set
that mixed normative requirements with characterizations of gaps, and their numerator counted the
`bases` identity gap as satisfied. `traceability.csv` now carries a `kind` column, and the rates are:

| Measure | Value |
|---|---|
| Traceability / evidence coverage — rows citing at least one running test | **41 / 63 = 65.1 %** |
| Implemented normative satisfaction — normative PASS / normative rows whose subject exists | **31 / 31 = 100 %** |
| End-state normative satisfaction — normative PASS / all normative rows | **31 / 49 = 63.3 %** |
| Known defects | **4** |

Row kinds: 49 NORMATIVE_REQUIREMENT, 5 DESIGN_CONSTRAINT, 5 CHARACTERIZATION, 4 KNOWN_DEFECT.
Only normative rows enter the satisfaction denominator.

### Stale test reference above

The "Unreached touched line" section cites `writer_survives_a_dangling_child_link` as the covered
counterpart in `IrWriter`. That test no longer exists: surviving a dangling link *was* the defect.
It is replaced by five strict tests — `writer_reports_a_dangling_child_link`,
`writer_reports_a_dangling_sibling_link`, `writer_reports_a_sibling_cycle_instead_of_looping`,
`writer_reports_a_dangling_root_declaration`, and `writer_accepts_a_well_formed_ir_unchanged`.
The `pretty_print` line remains unreached for the reason given, and stays in the denominator.

### Line coverage after the corrective

Same tool, same command, same denominator. Artifact: `reports/coverage/after-corrective.lcov`.

| Scope | baseline | M2b | corrective |
|---|---|---|---|
| workspace production lines | 83.10 % | 84.56 % | **84.72 %** |
| workspace production fns | 80.64 % | 82.16 % | **82.28 %** |
| `hymeko_core/src/ir/` | 90.84 % | 90.84 % | **90.84 %** |
| `writers/ir_writer.rs` | 0.00 % | 97.50 % | **100.00 %** |
| `traversal/hypergraphview.rs` | 90.00 % | 90.00 % | **94.85 %** |
| `util/pretty_print.rs` | 0.00 % | 89.86 % | 89.86 % |
| `ir/access.rs` | 94.64 % | 94.64 % | 94.64 % |

Touched-line coverage is 100 %. The nine lines still uncovered in `hypergraphview.rs` — `node_edges`,
`edge_nodes`, and the `par_sort_unstable_by_key` branch above the 8192-element threshold — are
pre-existing and untouched by this phase.

The closing statement of the section above still holds, and is now literally true of the numbers:
**requirements coverage is the primary measure; line coverage is a secondary check for unreached
code.** Coverage is also what exposed two real gaps this phase — a dead error branch in `IrWriter`
and three unexercised `Display` arms — so it did diagnostic work here, but it ranked nothing.
