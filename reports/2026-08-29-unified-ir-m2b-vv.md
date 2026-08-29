# Unified IR M2b — V&V baseline and the first consumer migration

**Date:** 2026-08-29
**Authorisation:** `APPROVED-CORE-EDIT: unified-ir-m2b-vv-core-consumers`
**Scope:** durable V&V structure, coverage baseline, then read-only consumer migration in `hymeko_core` and `hymeko_hnn`.

---

## 0. Result

| Gate | Result |
|---|---|
| V&V structure | 63 tests preserved exactly through `git mv`; 4 duplicated corpus walkers consolidated to 1 |
| Coverage tool | `cargo-llvm-cov 0.9.0` installed, satisfies the `tools.yaml` `0.x` pin |
| Workspace production coverage | 83.10 % → **84.65 %** (+1.55 pp) |
| Unified IR coverage | 90.84 % → 90.84 % (unchanged) |
| Fault injection | 4/4 mutants killed; source restored bit-identically |
| Core migration | 9 consumer sites; output byte-identical over 96 fixtures |
| HNN migration | 7 sites; incidence coverage identical over 96 fixtures |
| Performance | **+0.18 %** on `HyperGraphView::from_ir`, alternating A/B — within the 3 % gate |
| Known drift | unchanged, byte-identical (`left: 58, right: 57`) |

---

## 1. Preflight

| Item | Value |
|---|---|
| Local HEAD at start | `bd41186750452807d4fe426c1f9d0e737fd9c43a` |
| `origin/main` at start | `bd41186750452807d4fe426c1f9d0e737fd9c43a` — equal |
| Worktree | clean except the three untracked IFAC artifacts, preserved throughout |
| Host | median 3–9 % across the phase; recorded per measurement |

---

## 2. V&V structure

### 2.1 What the audit found before restructuring

Three phase-numbered directories held 63 tests and, between them:

- **4 copies** of the same `.hymeko` corpus walk (`depth.rs`, `coherence_tests.rs`, `frozen_v1.rs`, `accessors.rs`), differing only in variable names;
- **3 copies** of the intrusive child-list wiring;
- **2 copies** of `attach_arc_decls`;
- 4 separate fixture builders.

`CLAUDE.md` §6.1: the same traversal in three or more places is a refactor trigger.

### 2.2 The structure

```
hymeko_core/tests/unified_ir/
  support/       builder · oracle · corpus            (no tests)
  verification/  ids · access · hierarchy · coherence
                 identity · frozen_hash · hash_coverage
  validation/    corpus · consumer_equivalence · derived_views
```

**Verification** asks *does the implementation meet its specification* — synthetic inputs, known answers, no corpus dependency. **Validation** asks *does it hold on real programs and for real consumers*.

The split carries information: verification passing while validation fails means the *specification* is wrong; the reverse means the *implementation* is. A single `tests/` directory discards that, and so does a phase-numbered layout once the phase ends.

Every file moved with `git mv`. Phase provenance moved into `docs/verification/unified-ir/README.md` and each module header.

### 2.3 Test counts by layer

| Layer | Module | Tests |
|---|---|---:|
| verification | `access` | 20 |
| | `coherence` | 10 |
| | `ids` | 10 |
| | `identity` | 9 |
| | `frozen_hash` | 4 |
| | `hierarchy` | 4 |
| | `hash_coverage` | 3 |
| | **subtotal** | **60** |
| validation | `corpus` | 5 |
| | `consumer_equivalence` | 6 |
| | `derived_views` | 3 |
| | **subtotal** | **14** |
| | **total** | **74** |

63 before, 74 after: **+11**, of which 2 are focused guards added after fault injection, 6 are consumer equivalence and 3 are derived-view equivalence. **No test was split to raise the count**, and none was lost — the pre/post count of the moved tests is exactly 63.

---

## 3. Coverage

Full detail, exclusions and the toolchain limit: `docs/verification/unified-ir/coverage-baseline.md`.

| Scope | Baseline | After |
|---|---:|---:|
| Workspace production (line) | 83.10 % | **84.65 %** |
| Workspace production (function) | 80.64 % | 82.25 % |
| `hymeko_core/src/ir` (line) | 90.84 % | 90.84 % |
| `util/pretty_print.rs` | **0.00 %** | **89.86 %** |
| `writers/ir_writer.rs` | **0.00 %** | **97.50 %** |
| `hypergraphview.rs` | 90.00 % | **94.08 %** |
| `ir/access.rs` | 94.64 % | 94.64 % |
| Branch | **n/a** | **n/a** |

**Branch coverage is unavailable, not zero.** `cargo llvm-cov --branch` is marked `(unstable)` and needs nightly; this host is stable 1.97.1. Every `BRF` is `0` with no `BRDA` records at all. The summary prints `n/a` so "no data" is never mistaken for "nothing covered". No estimate substituted.

**Both migrated core consumers had 0 % baseline coverage** — neither had ever been executed by a test. The edits would have landed in unchecked code. The aggregate number would not have shown that; the per-file report read *before* editing did.

### Requirements coverage

| Measure | Value |
|---|---|
| Implemented-requirement (VERIFIED ÷ VERIFIED+PARTIAL) | **31 / 32 = 96.9 %** |
| End-state (VERIFIED ÷ all) | **31 / 44 = 70.5 %** |
| PARTIAL / PLANNED / BLOCKED | 1 / 5 / 10 |

Reported separately on purpose. End-state coverage should stay low until M3/M4; a rise without new implementation would mean requirements were dropped.

**No dummy tests were written** for the dynamic API. `UIR-DYN-01`…`10` (add/remove, stale IDs, cascade, rollback, batch atomicity, snapshot isolation, distributed replay) are BLOCKED against M3/M4.

---

## 4. Fault injection

Four defects, one at a time, in `ir/access.rs` — a file at 94.64 % coverage.

| # | Defect | Caught by |
|---|---|---|
| 1 | back-link validation removed | **2** — `access::mapping_pointing_at_a_foreign_record…`, `ids::backlink_validation_survives_the_optimisation` |
| 2 | child iterator loses its last element | **4** — `access::child_order_matches…`, `hierarchy_root_nested_siblings_and_leaf`, `last_child_of_agrees…`, `corpus::accessors_agree…` |
| 3 | map-only / checked-composite conflated | **1** — only `corpus::accessors_agree…`, which needs the corpus |
| 4 | iterator ids shifted by one | **5** — including both writer equivalence tests |

**Defect 3 was too weakly guarded**, so `access::arc_for_decl_checks_presence_only_and_agrees_with_as_arc` and `access::node_and_edge_for_decl_validate_the_backlink_but_as_node_does_not` were added, and the mutation re-run: now **2** tests, one corpus-independent.

Coverage was identical before and after every mutation and located none of them.

Mutated source was never committed. Restoration verified by hash: `0761f93917efcc4e07b7f68e5217f5b5da236b5f597ae7939b67b995afc2c5c7` before and after each round, with `git diff --stat` empty.

---

## 5. Direct-site inventory and migration

### 5.1 Before

| File | decl[] | rec[] | map[] | iter/len | Classification |
|---|---:|---:|---:|---:|---|
| `ir/lower.rs` | 16 | 1 | 3 | 3 | **construction** — retained |
| `ir/coherence.rs` | 9 | 0 | 0 | 1 | **malformed-state detector** — retained |
| `ir/ir.rs` | 7 | 0 | 0 | 1 | **storage implementation** — retained |
| `ir/canonical_hash.rs` | 1 | 1 | 0 | 6 | **representation-owner** — retained |
| `ir/hash_pass.rs` | 1 | 1 | 0 | 1 | **representation-owner** — retained |
| `util/pretty_print.rs` | 0 | 2 | 1 | 3 | **consumer → migrated** |
| `writers/ir_writer.rs` | 1 | 0 | 0 | 1 | **consumer → migrated** |
| `generators.rs` | 0 | 2 | 0 | 8 | **not an IR consumer** — `self.edges` is the generator's own `Vec<Vec<usize>>`; grep false positive |
| `hnn/hypergraphview.rs` | 0 | 2 | 1 | 4 | **consumer → migrated** |

### 5.2 After

| File | Consumer sites before | after |
|---|---:|---:|
| `util/pretty_print.rs` | 6 | **0** |
| `writers/ir_writer.rs` | 3 | **0** |
| `hnn/hypergraphview.rs` | 7 | **0** |
| **total migrated** | **16** | **0** |

**Unexplained remaining: 0.** Every retained site has a category above.

### 5.3 Contracts applied

| Old | New | Where |
|---|---|---|
| `ir.decl_nodes.iter().enumerate()` | `ir.decls()` | `ir_writer::write_all` |
| `&ir.decl_nodes[did.0]` | `ir.decl_node(did)` | `ir_writer::write_decl` |
| `ir.decl_children(d)` | `ir.child_decls(d)` | `ir_writer::write_children` |
| map + record read | `ir.edge_for_decl(d)` | `pretty_print::fmt_edge_arcs` |
| `&ir.arcs[aid.0]` | `ir.arc(aid)` | `pretty_print`, `hypergraphview` |
| `.len()` | `*_count()` | all three |
| `ir.edges.iter().enumerate()` | `ir.edge_records()` | `hypergraphview::collect_triples` |
| **`ir.decl_to_node[target.0]`** | **`ir.as_node(target)`** | `hypergraphview` — **map-only** |

That last row is the trap the migration rules exist for: `node_for_decl` would additionally resolve the record and validate its back-link — more work and different behaviour on malformed input. A mechanical search-and-replace would have changed it silently. It was migrated as map-only, deliberately.

### 5.4 One incomplete migration, caught by a test

`ir_writer::write_decl` was migrated first, but `write_children` still used the panicking `decl_children`. `writer_survives_a_dangling_child_link` failed and located it. Without that test the migration would have shipped half-done.

---

## 6. Equivalence evidence

| Property | Evidence |
|---|---|
| Writer output byte-identical | 96 fixtures, 170 402 bytes, compared against a direct-index reference walk |
| Writer root selection | identical to the pre-M2b form on all 96 |
| `edge_for_decl` ≡ map-then-index | 1 841 edge declarations |
| Counts ≡ `Vec::len` | all four tables, 96 fixtures |
| `HyperGraphView` incidence coverage | identical to a direct-access reference on 96 fixtures |
| `node_decl` / `edge_decl` back-pointers | preserved in order, 96 fixtures |
| Canonical hash | untouched; frozen vectors and corpus digest still pass |

Behavioural gains, tested: the writer and the view now **skip** a dangling id instead of panicking.

---

## 7. Performance

`ir/access.rs` is **byte-identical** to its M2a.1 state, so the M2a.1 verdicts carry forward and the accessor microbenchmark grid was not re-run.

Only `HyperGraphView::from_ir` changed inputs. Criterion first reported **+21.3 %** against its stored baseline — but ambient load had risen from 7 % to 30 % during that run, and the stored baseline was of unknown age.

Rather than explain it away, the discriminating test: alternate the pre- and post-migration source on the same quiet host, three rounds.

| Round | PRE | POST |
|---|---:|---:|
| a | 11.139 ms | 11.467 ms (+2.9 %) |
| b | 11.353 ms | **11.056 ms (−2.6 %)** |
| c | 10.890 ms | 10.919 ms (+0.3 %) |
| **mean** | **11.127 ms** | **11.147 ms** |

**+0.18 %**, signs alternating — noise, not regression. Well inside the 3 % gate. The original reading was Criterion comparing against a stale baseline, not against the pre-migration code.

Verdict: **NO REGRESSION**. No new allocation on any read path; no `unsafe`; no output, ordering, hash or floating-point change.

---

## 8. Gates

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-features --all-targets -- -D warnings` | PASS, no warnings |
| `cargo check --workspace --all-features` | PASS |
| `cargo build --workspace --all-features` | PASS |
| `cargo test --workspace --all-features --no-fail-fast` | **502 passed, 1 failed** |
| `cargo test --workspace --all-features --doc` | 22 passed |
| focused unified-IR | 74 passed |

The single failure is the known drift, byte-identical: `meta kinematics node count drifted, left: 58, right: 57` at `parse_files.rs:59`. Untouched and unsilenced.

---

## 9. Mistakes and false starts

1. **I trusted Criterion's stored baseline** and nearly reported a 21 % regression that did not exist. The alternating A/B on one host refuted it. The lesson is the brief's own: reproduce before concluding.
2. **I wrote a test that could not compile** — constructing a `CompiledProgram` to exercise a malformed-arc path. `ModuleKey` and `HashId` are not `Default`, and the type is not `Clone`. Adding those impls to production types to satisfy a test would be the test dictating the design, so the line is recorded as unreached with justification instead.
3. **My first `arc_for_decl` guard failed** because the shared fixture never created arc *declarations* — the same fixture defect M0.6 hit. Fixed by moving `attach_arc_decls` into the shared builder, which also removed a second copy of it.
4. **`sed` mangled escape sequences twice** while editing production strings, corrupting `pretty_print.rs`. Repaired with exact-match editing. Line-oriented tools are the wrong instrument for Rust string literals.
5. **A timed throughput test fails under coverage instrumentation.** Not a functional failure; recorded as a second documented skip rather than widening the authorised one silently.

---

## 10. Compliance

- **Not touched:** ID representation, `Ir` storage, public fields, mutation APIs, lowering, construction, serde layout, canonical hash algorithm, wire protocol, dependencies, `Cargo.lock`, `tools.yaml`, sibling repositories.
- **No `unsafe`, no `get_unchecked`**, no new unchecked public API.
- **The three IFAC artifacts** remain untracked and untouched.
- **`plan.pdf`** waived per the standing authorisation; `.tex`/`.tikz`/`.mmd` provided. No TeX installed.
- **Coverage artifacts** are path-normalised; no machine-specific absolute paths committed.

---

## 11. Next

**Next M2b repository: `../network`** (~4 dense-index sites expected; to be re-measured, not trusted). Entry condition: this repository green and pushed, `network` clean and non-diverged, its dependency path resolving to the published core.
