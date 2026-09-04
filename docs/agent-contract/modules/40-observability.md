---
source: CLAUDE.md §3 Live observability, lines 260-271, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: de-projectified (Phase 4)
---

**Contains:** §3 live observability — never run blind; both performance axes

# Observability

### Live observability — never run blind (added 2026-07-01, user-enforced: "running blind is stupid")

A long run that emits nothing until it finishes is **forbidden**. You cannot distinguish *slow-but-progressing* from *stuck* from *diverged-and-looping* without live signal — and in deep-learning runs (stochastic, hours long, silently divergent) that blindness is not a minor inconvenience, it is a defect on the same level as shipping untested code. This is mandatory, not stylistic.

- **Every training / multi-minute run emits periodic live progress to flushed stdout** — at minimum: iteration/step **count** (`k/total`), the **loss(es)**, **throughput** (`steps/s` or `it/s`), and **ETA**. The loss column is the divergence tripwire: a `NaN`/`inf` there surfaces a blow-up the instant it happens instead of after an hour of wasted compute.
- **Cadence:** a line must appear within ~1–2 min of wall time; **no run goes >10 min without output.** Tune the log interval to the step rate, not a fixed constant copied between tasks.
- **The signal must be VERIFIABLE, not decorative.** A silent hook that appends to a `history` list without printing is **not** observability; a spinner that says "running" is not observability. The line must carry real numbers a reader can check (loss magnitude, rate, buffer/epoch). Live feedback is a library default, not a per-script afterthought: the training loop prints `step | loss | steps/s | ETA | buffer` on an interval because it is the loop, not because a script remembered to ask. The 65 minutes of silence that produced this rule, and why logging being *present* did not help: [incidents/2026-07-01-sixty-five-minutes-dark.md](../incidents/2026-07-01-sixty-five-minutes-dark.md).
- **Measure BOTH performance axes, always — forward and training.** A run is not characterised by one number. Report **forward/inference performance** (B=1 *and* batched latency — the deploy cost; §10 profiler where a hot spot is claimed) **and training performance** (steps/s, per-step update cost, wall, ETA, peak RSS). One without the other is an incomplete measurement: a model can be cheap to train and slow to deploy, or vice-versa, and the pivot between on-policy/off-policy, `update_every`, and deploy fast-paths turns entirely on knowing *which* axis bounds you (§6.5 #18).
- **When a run is slow, add the signal before you add a workaround.** Do not kill-and-shrink, re-estimate, or "just wait" a dark run: instrument it first, then decide from what you see. Patching around blindness instead of fixing it is how the same lapse recurs.

This is the run-time sibling of §6.5 #18 (profile the real bottleneck before optimizing) and the evaluation-metric-integrity block above: **you cannot optimize, trust, or debug what you cannot see.**

