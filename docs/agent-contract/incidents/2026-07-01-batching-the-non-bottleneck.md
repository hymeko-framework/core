<!-- SYNCED from hymeko/hymeko @ ff0c013 — docs/agent-contract/incidents/2026-07-01-batching-the-non-bottleneck.md
     Do not edit here. The universal modules have ONE source; a fork of §6.5
     in each repository makes every citation in every report ambiguous.
     Edit upstream, then re-sync and record the revision. -->

---
source: CLAUDE.md §6.5 anti-patterns 18 and 19, lines 444-446, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: extracted in Phase 4 from the rules it motivates
---

# 2026-07-01 — the tempting optimisation was the wrong one

The rules these produced are §6.5 **#18 optimise the measured bottleneck** and **#19
default the proven speed-win**, in [modules/30-code-health.md](../modules/30-code-health.md).

**The slow network was not slow.** An HSiKAN policy was costing 18 ms/step and read as
an expensive model. It was launch-bound at batch size 1. Batching the rollout took it to
1.5 ms/step, after which **physics at 3.3 ms became the floor**. Pruning the network —
the obvious move, and a primitive that had genuinely helped elsewhere — would have
optimised a term that was no longer bounding anything.

Costs worth knowing before touching either term: an off-policy learner does a gradient
update every environment step (about 30 minutes per 1e5 steps) where an on-policy one
batches them; and a vectorised rollout batches the *forward* pass but still steps physics
serially. Which of the three terms bounds you is a measurement, not an intuition.

**And then it was left switched off.** The vectorised rollout had shipped, measured at
5–6×, while four of six training entrypoints still ran single-environment. The fix was
to consolidate them behind the default. A measured win reachable only by passing a flag
is a win most runs will not get.
