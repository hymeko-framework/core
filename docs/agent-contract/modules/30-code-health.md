---
source: CLAUDE.md §5, §6 (6.2–6.5), §7, §8, lines 291-318, 343-449, 514-550, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: de-projectified (Phase 4)
---

**Contains:** §5 data-oriented design · §6 code health · §6.2 complexity · §6.3 static analysis · §6.4 error handling · §6.5 anti-patterns · §7 design patterns · §8 design by contract

# Code health

The redundancy rule itself (§6.1) is in [10-discovery.md](10-discovery.md), with §6.6 and
§6.7; §6.5 #1, #3, #12 and #13 below are its anti-pattern face.

## 5. Data-Oriented Design

Prefer data-oriented layouts where reasonable:

- **Struct-of-arrays** over array-of-structs for hot loops and bulk numerical state.
- **Contiguous, cache-friendly buffers.** Avoid pointer chasing in inner loops.
- Separate **cold metadata** from **hot numerical data**.
- Python: prefer NumPy / PyTorch / Polars vectorized ops over per-object Python loops. A `for` loop over `n > 10⁴` Python objects in a hot path is a code smell.
- Rust: prefer `Vec<T>` of POD types over `Vec<Box<dyn Trait>>` in hot paths. Reach for `bytemuck`, `ndarray`, or `arrow` when appropriate.
- C/C++: prefer flat arrays and indices over linked structures in inner loops.

OO inheritance is acceptable for **control flow** and high-level orchestration. It is **not** the right tool for bulk numerical state.

### Numerical stability (FP-heavy code)

For numerically sensitive code (spline and basis-function recursion, kernel evaluation, gradient updates):

- Avoid catastrophic cancellation: do not write `(a - b) / (a + b)` directly when `a ≈ b`. Reformulate.
- Use Kahan or Neumaier summation for accumulators over more than ~10⁴ terms, or anywhere magnitudes vary by orders of magnitude.
- Document conditioning assumptions in the function's contract (Section 8). State the input range over which the result is trusted.
- Test against a high-precision reference (`mpmath` or symbolic `sympy`) for boundary inputs.

---

## 6. Code Health

This section enumerates code-quality ceilings. Its subsections are gates before reporting completion.


### 6.2 Complexity Budget

Static analysis is a hard gate before reporting completion.

#### Thresholds (per function unless stated)

- Cyclomatic complexity (McCabe): warn at 10, fail at 15.
- Cognitive complexity (Sonar/Clippy): warn at 15, fail at 25.
- Function length: warn at 80 lines, fail at 200.
- Nesting depth: fail at 5.
- Module length: warn at 800 lines.

#### Tooling

- Rust: `cargo clippy -- -D clippy::cognitive_complexity` plus `rust-code-analysis` for cyclomatic.
- Python: `radon cc -a -nc <path>` and `flake8 --select=C90 --max-complexity=10`.
- C/C++: `lizard -CCN 15 -L 200 <path>`.

#### Rules

- A function over the hard ceiling cannot ship. Refactor required: extract function, table-driven dispatch, replace conditional with polymorphism, or split state machine.
- Generated code is exempt but must be marked (`// generated` header or path matching `**/generated/**`).
- Waivers must be declared in the report (Section 9) with reason.

### 6.3 Static Analysis Gate

All linters and type checkers must pass before a task is reported complete. **Warnings are errors.**

#### Required gates

- Rust: `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`.
- Python: `ruff check` and `mypy --strict` on changed code (full project where feasible).
- C/C++: `cppcheck --enable=all --error-exitcode=1`, plus `clang-tidy` where configured.

#### Rules

- New `#[allow(...)]` (Rust), `# type: ignore` / `# noqa` (Python), or `// NOLINT` (C++) requires an inline comment stating the reason, scoped to the smallest possible region (single line or single function).
- A blanket `#![allow(...)]` at crate root is a core-level decision; treat as a `CORE.YAML` edit (Section 1).
- Suppressions accumulated by a change must be listed in the report.

### 6.4 Error Handling Discipline

No silent failures. Every error path is explicit.

#### Rust

- No `unwrap()` or `expect()` in non-test code, except where preceded by an explicit invariant check that makes the panic provably unreachable. Document the invariant in a comment immediately above the call.
- Use `Result<T, E>` with concrete error types. `thiserror` for libraries; `anyhow` only at binary boundaries.
- `?` is the default propagator. Manual error mapping must add information, not strip it.

#### Python

- No bare `except:`. Catch the most specific exception type that is meaningful.
- Re-raise with context: `raise NewError(...) from err`. Do not swallow the cause.
- Logging an exception and continuing is **not** error handling unless the recovery path is documented and tested.

#### C/C++

- Every function returning an error code has its return value checked.
- No discarding of `errno`. No silent `NULL` returns from functions that allocate.

Reports must list any new error-handling waivers (`unwrap`, broad `except`, ignored return value) introduced by the change, with justification.


### 6.5 Anti-Patterns (mandatory avoidance)

The user is an experienced researcher and engineer (see Operating Principles). Code that *looks simple* but propagates by copy-paste is rejected. Code that *looks complex* (traits, generics, config structs) but eliminates a Cartesian product or a duplicated scaffold is the right answer. Concretely, the following anti-patterns are forbidden — refactor before adding to them, not afterward:

1. **Cartesian-product API surface.** When a function family differs only by orthogonal axes (mode × scorer × pruner × filter × ABB × batched × tiered × …), do **not** create one named function per Cartesian cell. Use Strategy traits + a config struct + one entry point per family, with internal dispatch matching on the config. If you would add `enumerate_old_name_with_X_rs` to differentiate, instead add a config field `enable_X` (or analogous) to the existing entry. The presence of `_filtered_`, `_batched_`, `_bb_`, `_tiered_`, `_global_`, `_adaptive_`, `_signed_`, `_color_coded_`, `_path_closure_` etc. in a function name is a *signal* that the next axis belongs in config — not in another wrapper. The 16-variant case this rule came from: [incidents/2026-05-11-cartesian-api-and-scaffolds.md](../incidents/2026-05-11-cartesian-api-and-scaffolds.md).

2. **Algorithm code behind a Python boundary.** Pure-Rust algorithm logic (DFS, BFS, scorers, pruners, graph utilities, canonical-form helpers) belongs in the *algorithm* crate, not in the binding crate. A binding crate is a thin wrapping layer plus type-conversion glue — PyO3, wasm-bindgen, a C shim, any of them. The ~1100 lines of pure graph algorithm found sitting behind a PyO3 boundary: [incidents/2026-05-11-cartesian-api-and-scaffolds.md](../incidents/2026-05-11-cartesian-api-and-scaffolds.md).

3. **Per-experiment scaffold duplication.** When N `run_<thing>.py` files each re-implement `train_val_split`, the train loop, the AUC eval loop, argparse + JSON output — refactor to a single `Experiment` / `train_<task>(config, model)` framework. Each new experiment script should be a ~20-line config plus model construction; everything else is shared. The 98 scripts, 8 reimplemented splitters and 69 independent eval loops this rule came from: [incidents/2026-05-11-cartesian-api-and-scaffolds.md](../incidents/2026-05-11-cartesian-api-and-scaffolds.md).

4. **Long single-file modules.** Past ~400 LOC AND ≥ 2 distinct concerns (separable nouns: "shells" vs "composers", "config" vs "model", "ablations" vs "variants") → split into a package with `__init__.py` re-export. CLAUDE.md §6.2's 800-LOC warning is the *outer* limit; this 400-LOC heuristic kicks in earlier. See memory `feedback_decompose_long_files.md`. Tests can stay in one file longer (read top-to-bottom less often).

5. **Adding a new axis by inventing a new function name.** Sub-case of #1, but the most common entry point. Every time you reach for a new function name that is "old_name + one new word", stop. Add a config field instead.

6. **`#[allow(clippy::too_many_arguments)]` as a band-aid.** If you're at >7 parameters and reaching for the allow, write a config struct first. The allow is acceptable on the *one* outer wrapper that maps to a Python kwargs surface (where the kwargs flat-list IS the API); not on internal helpers.

7. **String-typed config that should be an enum.** `mode: &str` with valid values `{"none", "balance", "unbalanced"}` is a missed `enum Mode { None, Balance, Unbalanced }`. The string is fine at the Python boundary (kwargs); the moment it crosses into Rust internals, it becomes an enum with a `from_str` parser at the boundary. Mismatched strings should fail at parse, not at the `_ => panic!("unknown mode")` arm in the inner DFS.

8. **Forward-time flags for what should be different code paths.** Inverse of the duplication anti-patterns: when an *ablation* or *variant* truly differs structurally (e.g., one shell removed, head replaced with MLP), construct a different model class. Don't toggle behavior at `forward()` time with `if self.no_outer:` branches — those degrade type safety, hide structural differences from the reader, and gum up profile traces. (Configuration is fine when the difference is parametric; class-per-structural-variant when the difference is structural.) The line is: parametric differences → config; structural differences → class.

9. **Bypassing existing Strategy traits at a layer boundary.** When the inner library defines `trait Scorer { … }` with 4 impls, do not re-introduce `match score_kind { "balance" => …, "fraction_negative" => … }` at every API surface. Centralize the dispatch in one `pick_scorer(&str) -> Box<dyn Scorer>` (or `fn pick_scorer(...) -> &'static dyn Scorer`) and reuse it. The `cycles.rs` 16-variant case had 16 copies of the same `match` ladder.

10. **`ulimit -v` on CUDA workloads.** See §4. Use `systemd-run --user -p MemoryMax=16G` (cgroups v2 RSS) instead. Cross-listed here because it is a memory-side anti-pattern, and the same class of "wrong tool, plausibly defensible at first glance" failure as the others above.

11. **Global variables / module-level mutable state.** No `static mut`, no `lazy_static!`/`once_cell::sync::Lazy` holding mutable runtime state, no module-level Python dicts updated at runtime, no singletons, no "set a flag in an environment variable and read it from deep inside a hot loop." State that crosses function boundaries is passed explicitly — as a parameter, a struct field, a closure capture, or a context object threaded through the call site. The cost of globals is testability collapse, threadsafety landmines, hidden coupling between unrelated modules, and order-of-import determinism bugs. Acceptable narrow exceptions: (a) genuinely-immutable program constants (`const` / `static` of POD types), (b) logger / tracing subscriber initialised once at `main` entry, (c) feature-detection caches that are populated lazily, never mutated. Anything else: pass it as a parameter. If the call chain is long, that is a *signal* that you need a context struct, not a global. Environment-variable-driven feature flags read at deep call sites (`os.environ.get("HSIKAN_...")` inside a forward pass) are forbidden — parse all env at process startup into a typed config and pass that config explicitly.

12. **Creating a new artifact without a discovery pass first.** Before `mkdir <new_pkg>`, before `Write` of a new module, before `cargo new` of a new crate, before adding a new `.py` script next to existing ones, before drafting a fresh plan dir: run the `find` + `grep` + `ls` discovery sequence required by §6.1 and confirm no existing scaffolding covers the concept. Without that pass, the creation is unauthorised. A repository with a few hundred reports, dozens of plan directories and a dozen packages will almost always already have scaffolding for a "new" concept; the search is seconds and the duplicate is hours. The two 2026-06-03 cases — ~1700 LOC of net-new work, and a near-miss duplicate package caught only because the user remembered the existing crate: [incidents/2026-06-03-duplicate-scaffolding.md](../incidents/2026-06-03-duplicate-scaffolding.md).

13. **`<thing>_v2`, `<thing>_v3`, `<thing>_new`, `<thing>_<YYYY-MM-DD>` file proliferation.** Git tracks history; if a file needs to change, **edit the canonical file in place**. Creating `submit_X.sh` next to `submit_X_v2.sh` next to `submit_X_v3.sh` is forbidden — same for `report_v2.md`, `plan_new.tex`, `module_2026_06_04.py`, etc. If a script genuinely needs multiple modes (smoke/full/rerun), use **one file with a mode argument**, not three files. The five files for one experiment family, and the stale v1 that was accidentally re-invoked off the cluster: [incidents/2026-06-04-kifu-and-proliferation.md](../incidents/2026-06-04-kifu-and-proliferation.md).

14. **Preamble before execution.** When the user gives a concrete action ("fix X", "delete Y", "submit Z"), execute the tool call. Do **not** write a plan paragraph, a "first I'll..., then I'll..." outline, a status report, or a four-option `AskUserQuestion` before the action. Reply with **what changed** AFTER the tool returns, in one short paragraph. The single allowed pre-execution sentence is a clarifying question when the action is genuinely ambiguous — and only one sentence, not a structured question with options. Documented 2026-06-04: ~30 minutes of token waste explaining a 3-line `Edit` before performing it; user explicitly called it out as "lekezelés" (condescension).

15. **Writing a new memory or report instead of consulting the existing ones.** Memory files under `~/.claude/projects/.../memory/` and reports under `reports/` are **only worth the tokens if they get read at decision time**. Before writing a new `feedback_*` memory: grep the existing memory dir for the keyword and **update or delete**, do not add a near-duplicate. Before writing a new report: check `reports/` for an existing report on the same incident class and append/update there. Documented 2026-06-04: the agent wrote `feedback_check_komondor_quotas_before_fanout` on 2026-06-02, then on 2026-06-04 submitted a 40-cell uniform-time array with zero `seff` smoke — exactly what the memory forbade. The memory was loaded but not consulted. New memories/reports added in this state are token-waste placeholders for a future reform that does not come.

16. **HPC sizing without a per-cell-class measurement.** Moved verbatim to
    [60-hpc.md](60-hpc.md); the anchor `§6.5 #16` resolves here.

17. **Orphaned background processes / stale runs.** Before launching any long compute run, list the running interpreters with their start time and CPU, and confirm no stale run is still grinding — a forgotten run steals cores silently and invalidates every wall-clock measured alongside it. Two hard sub-rules: (a) **killing or `TaskStop`-ing the parent does NOT kill spawned children** — process-pool workers and the like survive as zombies; kill them by PID. (b) **When running concurrent accelerator jobs, watch device memory and compute contention.** How to list them on this host, the run that outlived its relevance by three hours, the zombie children that starved the next launch for 44 minutes, and the page-file constraint that was lifted 2026-07-03: [incidents/2026-07-01-stale-runs.md](../incidents/2026-07-01-stale-runs.md).

18. **Optimizing before profiling the real bottleneck (RL / long runs).** Measure the per-step cost split — **physics vs network-forward vs gradient-update** — before choosing an accelerator; optimizing the non-bottleneck is the "defensive optimization" §3 forbids, restated for runs. Corollary costs to budget before touching them: off-policy does a **gradient update every env step** (~30 min / 1e5 steps) that on-policy batches; a vectorized rollout batches the *forward* but steps physics serially. Know which term bounds you. The run where the tempting optimisation was the wrong one: [incidents/2026-07-01-batching-the-non-bottleneck.md](../incidents/2026-07-01-batching-the-non-bottleneck.md).

19. **Leaving a measured optimization as an opt-in flag instead of the default (user directive 2026-07-01).** An optimization that is *measured* to improve — with the measurement on record — must be wired into the **default** entrypoint, not left as a scratchpad flag, an opt-in kwarg, or one call-site out of five. Conversely: do **not** flip a default that changes the **model or the metric** (not pure speed) without the A/B verdict that says it wins — that is a structural change, gated on evidence, not convenience. "Don't optimize for nothing" cuts both ways — default the proven speed-wins; withhold the unproven model-changes. The measured 5–6× rollout that four of six entrypoints were not getting: [incidents/2026-07-01-batching-the-non-bottleneck.md](../incidents/2026-07-01-batching-the-non-bottleneck.md).

**How this section is used:** before adding a new function, file, or HPC submission, sweep your proposed change against this list. If any apply, the action is *refactor first* (or *consult the existing artifact first*), not *add now and defer cleanup*. The cleanup never happens; that's the whole reason this list exists. Reports (§9) should explicitly note "no §6.5 anti-patterns introduced" or list any waivers with justification.

Where items 13–16 came from: [incidents/2026-06-04-kifu-and-proliferation.md](../incidents/2026-06-04-kifu-and-proliferation.md).

## 7. Design Patterns

Apply standard patterns where they reduce coupling and clarify intent. Examples appropriate to this codebase:

- **Strategy** — swappable algorithms (optimizers, kernels, activation backends, scoring functions).
- **Builder** — complex configuration objects (training configs, model specs).
- **Adapter / Facade** — bridging core (read-only) APIs to new code without modifying core.
- **Observer / Pub-Sub** — telemetry, training callbacks, monitoring hooks.
- **Command** — reproducible, replayable experiment steps; pairs well with experiment logging.
- **Visitor** — traversals over heterogeneous IR / AST / hypergraph node types.
- **Repository** — abstracting dataset / checkpoint / artifact storage.

"Do not over-pattern" governs *speculative* abstraction — a Strategy trait
with one impl, a Builder for a two-field struct. It never licenses a second
copy of existing logic. Duplication is not the conservative choice; it is a
structural change with no rollback, made without a plan.

---

## 8. Design by Contract

Every new public function specifies:

- **Preconditions** — what must hold on inputs (types, shapes, dtypes, ranges, invariants).
- **Postconditions** — what is guaranteed about outputs and side effects.
- **Invariants** — what state is preserved across the call.

### Implementation

- **Rust:** `debug_assert!` for preconditions and invariants in debug builds; document contracts in rustdoc under `# Preconditions`, `# Postconditions`, `# Invariants`, `# Panics`, `# Errors`.
- **Python:** `assert` statements, or `icontract` / `pydantic` validators, plus explicit docstring sections (`Preconditions`, `Postconditions`, `Invariants`). Do not rely on type hints alone for shape/range constraints.
- **C/C++:** `<cassert>` macros, or contract attributes where available. Document in headers, near the declaration.

Contracts are evaluated in debug and test builds. A violated **precondition** is a bug in the **caller**. A violated **postcondition** is a bug in the **function**. Reports must distinguish the two.
