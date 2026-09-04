---
source: CLAUDE.md §2, §9, lines 149-192, 551-599, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: de-projectified (Phase 4)
---

**Contains:** §2 the plan bundle · §9 the report · graphical output

# Planning and reporting

## 2. Plan Before You Act

For any non-trivial implementation, experiment, architecture change, or
core-adjacent change, produce a **plan document** before writing code. A
single-file local fix, typo, formatting-only change, read-only investigation, or
small forensic classification does not require the full plan bundle unless it can
change research conclusions or persistent artifacts. Commit plans to:

```
docs/plans/<YYYY-MM-DD>-<slug>/
```

The plan must be produced in **all four** formats:

- `plan.tex`   — LaTeX source, compilable standalone (`pdflatex` or `lualatex`).
- `plan.pdf`   — built from `plan.tex`.
- `plan.tikz`  — TikZ figure(s) for architecture / dataflow / module boundaries.
- `plan.mmd`   — Mermaid diagram(s) for sequence / state / dependency views.

The plan must state, at minimum:

- **Created-at timestamp** — `YYYY-MM-DD HH:MM` (local), at the top of the plan (added 2026-06-27, user policy).
- **Estimated time of completion (ETA)** — a wall-clock estimate to finish the work, *with the basis* (e.g. "≈45 min: 3 supervised cells × ~12 min each + write-up"); for runs, the expected wall and seed/step budget. State it up front so progress can be tracked against it (added 2026-06-27, user policy).
- **Scope** and goal.
- **Affected files** (full list).
- **CORE.YAML items touched** — must be the empty list, or escalate per Section 1.
- **Interface changes** (signatures, types, contracts).
- **Test strategy** (which tests at which layer; see Section 3).
- **Performance budget** (peak RSS, wall time, inference latency).
- **Rollback path**.
- **Risk anticipation** — what could go wrong at production scale that won't show up in unit tests? Specifically:
  - **Performance-contract preservation** — does this change preserve every existing contract on the touched code path? Caps (e.g. `max_cycles`, batch_size), memory budgets, time bounds, and sample-size limits are contracts. Adding a new branch beside an existing one is the highest-risk case: the new branch must inherit all the contracts the old one enforced. Grep the file you are about to edit for aspirational comments ("should never materialise", "respect cap X", "TODO: push the cap into …"). Those are flags for latent contract gaps — if the new branch doesn't honor the comment, write the test that proves it does, or escalate the comment into a real precondition check.
  - **What does the worst-case input look like?** State the production-scale dataset, cap, and config the plan is intended to run under, not just the unit-test fixture.
- **Empty-plan-dir hygiene** — if a plan dir is created but the work is abandoned, delete the dir before the next session. Empty plan dirs are noise that look like in-progress work.

**For planned work, no implementation begins until the plan exists on disk and
compiles.** This is non-negotiable for substantive work. Back-dating a plan
(writing it alongside or after the implementation) defeats the purpose — the plan
exists to surface risk *before* code is touched. A plan→report gap shorter than
the time it would take to honestly implement the change is a back-dating red
flag.

---

## 9. Reporting

Every completed task produces a report at:

```
reports/<YYYY-MM-DD>-<slug>.md
```

The report contains, at minimum:

- **Summary** of the change.
- **Files touched** (full list, with line counts added/removed).
- **CORE.YAML items touched** — must be empty, or reference the approval thread.
- **Test results** — per layer: counts, pass/fail, durations.
- **Performance results** — peak RSS, wall time, inference latency, vs. the budget declared in the plan and vs. the previous baseline if available.
- **New / removed dependencies.**
- **Open issues** and **follow-up items.**
- **Experiment provenance** (for runs that produce data, models, or measurements):
  - Git SHA (working tree must be clean, or list dirty files explicitly).
  - Environment dump (`cargo tree`, `pip freeze`), OS, kernel, CPU model, RAM.
  - GPU device, driver version, CUDA / ROCm version where applicable.
  - Random seed(s) used.
  - Dataset hash(es) or fixture content hashes.
  - **For in-flight experiments referenced in the report:** the corresponding log file path and PID (or jobspec). ID-only references with no on-disk anchor are not acceptable — they evaporate when the chat session closes and leave the next session unable to verify.

A task with no report is an incomplete task. The report is the unit of acceptance, not the diff.

### Graphical output (mandatory for experiments)

Numbers alone underserve the audience. Every experiment or campaign that produces measurements emits its result in
**three forms**, not one:

1. **Numerical** — the values (JSON / journal / a markdown table in the report). Always.
2. **Plotted** — the comparison as a figure (curves, grouped bars, median/IQR), saved as a `.png`/`.svg` next to
   the report. Always, when there is more than one number to compare.
3. **Animated** — for any task with a spatial / temporal / control character (robotics, RL, simulation, a policy
   acting), a **`.gif`** of the behaviour (the trained policy and/or the demonstrator). Render it; do not describe
   it.

This is an *inherent strategy*, not a nicety: a result that can be watched and plotted is legible to a graphical
audience and presentation-ready; a bare scalar is not. Wire the three outputs into the campaign/experiment script
itself (a `--gif` / `--plot` path, on by default where cheap) so they are produced **every run**, not bolted on
afterward. Reuse the shared rendering, plotting and campaign-visualisation helpers the project already has —
never re-implement either (§6.1). Prefer **higher resolution** for anything that
may reach a slide (≥ 960×720 for GIFs). Artifacts live next to the report (`reports/…` or a `results/{figures,gifs}/`
tree) and are referenced from it.
