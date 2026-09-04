<!-- SYNCED from hymeko/hymeko @ ff0c013 — docs/agent-contract/incidents/2026-07-01-metric-integrity.md
     Do not edit here. The universal modules have ONE source; a fork of §6.5
     in each repository makes every citation in every report ambiguous.
     Edit upstream, then re-sync and record the revision. -->

---
source: CLAUDE.md §3 "Evaluation-metric integrity", lines 250-259, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: extracted in Phase 4 from the rules it motivates
---

# 2026-06-30 / 2026-07-01 — four ways a metric lied

The rules these produced are **Evaluation-metric integrity**, in
[modules/20-testing.md](../modules/20-testing.md). One session was lost to them, which
is why they are four separate rules and not one.

**The ceiling was never measured.** `single_hsikan` held-delivery scored 0.167 and was
read as a "laughing-stock failure". The scripted demonstrator, measured afterwards under
the same metric, scored 0.33. The gap was a *cloning* gap, not a task ceiling — the
panic was mis-framed, and the optimisation it triggered was aimed at the wrong term.

**The probe used the wrong horizon.** A 60-step probe on a 300-step task read the
demonstrator at **0/24** — a false zero that nearly triggered a halt. At the true
300-step horizon the same demonstrator scored 0.33–0.54. A probe on a truncated horizon
is not a noisy measurement of the task; it is a measurement of a different task.

**The filter did not equal the metric.** Demos were filtered by *momentary* `in_zone`
and graded by *held* dwell. The clone was therefore trained on touch-and-roll-out
trajectories and then graded on holding, and nothing in the pipeline reported the
mismatch.

**A failure inflated the metric (2026-06-30).** A physics blow-up ejected the box upward
and the lift-metric counted it as a success — an 0.875 "win" that was an explosion. A
metric that only drops under failure is self-announcing; one that a failure can raise is
not, and it will be optimised toward.
