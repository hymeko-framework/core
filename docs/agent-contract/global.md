---
source: CLAUDE.md §0, Operating principles, lines 51-63, 68-92, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: de-projectified (Phase 4)
---

**Contains:** Operating principles (9 of the 10 bullets; the paradigm hierarchy stays in the root CLAUDE.md), §0 Workflow

## Operating principles

These hold above any individual section. If a specific rule below appears to permit a shortcut, it does not.

- **Timestamp everything (added 2026-06-27, user policy).** Prefix every chat reply with the local time `[YYYY-MM-DD HH:MM TZ]`. Date every plan (`Created-at`) and report. Every plan carries an **ETA** (§2). When a reply reports a run's state, include the wall-clock and what time it is, so progress is legible against the estimate. Get the real local time (don't guess it) when a precise stamp matters.
- **Be systematic, not lazy.** When a procedure exists in this document, follow it. Shortcuts accumulate into broken experiments. If a rule feels inconvenient, that is a sign the rule is doing work — not a sign to skip it.
- **Write the plans down.** Plans live on disk (Section 2), not in the working memory of a single chat turn. An unwritten plan has no continuity, no review surface, and no audit trail.
- **No improvisation under pressure.** "I'll just try it and see" is the failure mode this document exists to prevent.
- **Analyze, don't declare; no premature certainty (no "superstitions").** When diagnosing a failure, interpreting a measurement, or choosing a design, do **not** assert a single "most likely" cause or "the answer" as if it were settled. Enumerate the plausible hypotheses or options, state the evidence that would distinguish them, and run the **discriminating test before concluding**. A confident-sounding single explanation that has not been isolated is a guess in a lab coat — and on this codebase (signed-link leakage, FP kernels, OOM bugs) the convenient first explanation is often wrong. Reserve "this *is* the cause" for *after* the test that rules out the alternatives; until then, present the option space and the experiment that would collapse it. Always distinguish, in writing, what is **measured**, what is **inferred**, and what is **still a hypothesis**.
- **Failed-diagnosis recovery.** If a previous agent, report, or hypothesis sent the work into a wild-goose chase and did not solve the problem, freeze that narrative. Restart from the smallest reproducible symptom: the failing command, failing test, bad artifact, or metric mismatch. Classify existing edits as `keep / suspect / generated / unknown` before building on them. A claim that did not fix the issue is a hypothesis, not context.
- **Cached facts before new runs.** Before launching measurements, campaigns, or broad test grids, grep `MEMORY.md`, `reports/`, task notes, and existing `results.json`/logs. Re-measure only when the existing artifact is missing, stale for the current code path, or contradicted by a focused check. After a code change, re-measure the affected cell first; do not rerun the whole grid by habit.
- **Search before claiming novelty or prior art (added 2026-06-28, user policy after a novelty-assessment miss).** Before asserting that an idea is novel, or that it "has been done" / "already exists" / "is incremental" / "is old," run a **brief search** (`WebSearch`, the `paper_search` MCP, a repo/literature `grep`) and either cite a concrete instance or state explicitly that a focused search found none. This is the literature-side of the discriminating-test rule: an unevidenced "it's been done" is the same failure as a "most likely cause" that was never isolated. Three hard sub-rules: (1) **never conflate a primitive existing in another field** (e.g. hypergraphs in ML, signed graphs in spectral theory) **with prior art for the specific contribution** — name the work that does *this* thing, or admit there isn't one; (2) **absence in a bounded search is "none found," not "proven novel"** — say which, and note the search was not exhaustive; (3) **distinguish the generic pattern from the specific claim** (a pattern like "one IR, many emitters" is old; a specific mechanised claim built on it may not be). The on-record 2026-06-28 miss this rule came from: [incidents/2026-06-28-novelty-search-miss.md](incidents/2026-06-28-novelty-search-miss.md).
- **The user is an experienced engineer/researcher, not a casual programmer.** Calibrate accordingly. Architectural complexity that eliminates duplication and surfaces invariants is *preferred* over flat code that repeats. Reach for traits, generics, associated types, sealed enums-with-data, builder/strategy/visitor patterns without hesitation if they reduce a Cartesian product or unify a duplicated scaffold. The reader is fluent in Rust, modern Python typing, and standard GoF/DDD patterns. The floor is "no needless repetition." The ceiling is "no easier than necessary." Dumbing code down to look beginner-friendly is the same failure mode as copy-paste — it costs the user tokens and dignity for nothing.

## 0. Workflow (top-level)

For every substantive implementation or experiment task, in this order:

1. **Read the repository's gate file and toolchain pin**, where it has them — `CORE.YAML` and `tools.yaml` at the root in `core/` (§1, §10).
2. **Discovery pass** — for any task that might introduce a new artifact (module, crate, script, plan, fixture, function), run `find` / `grep` / `ls` first to confirm no existing scaffolding covers it. This is a Section 6.1 / §6.5 #12 precondition; skipping it has cost ~1700 LOC of duplicated work in 2026-06-03 incidents alone.
3. **Ownership pass** — for any task that adds a capability, answer *which package owns this* before answering *how to build it* (Section 0.1). The discovery pass above asks whether the thing already exists; this asks where it belongs. They fail differently: a grep can come back empty and the placement still be wrong.
4. **Plan** the change (Section 2) before touching code.
5. **Implement** the change outside of `CORE.YAML`-protected files (Section 1).
6. **Write tests** at all required layers (Section 3).
7. **Run tests** and **measure** memory + latency (Sections 3, 4).
8. **Write the report** (Section 9).
9. Only then, return control to the user with a summary.

For lightweight triage, questions, and one-file mechanical edits, use the same
ordering in compressed form: read the relevant files, check whether `CORE.YAML`
is implicated, make or report the minimal finding, and state what was not
verified. Do not create plans, benchmarks, plots, or reports that add no
discriminating value to the immediate task.

If any required step fails, stop and report the failure. Do not proceed past a
failing step.
