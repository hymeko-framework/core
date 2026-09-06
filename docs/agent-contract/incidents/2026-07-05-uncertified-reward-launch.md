<!-- SYNCED from hymeko/hymeko @ ff0c013 — docs/agent-contract/incidents/2026-07-05-uncertified-reward-launch.md
     Do not edit here. The universal modules have ONE source; a fork of §6.5
     in each repository makes every citation in every report ambiguous.
     Edit upstream, then re-sync and record the revision. -->

---
source: CLAUDE.md §3 Rules, "Oracle-certify the TRAINING reward", line 214, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: extracted in Phase 4 from the rule it motivates
---

# 2026-07-05 — an overnight launched on an uncertified reward

The rule this produced is **Oracle-certify the training reward before queuing any RL
run**, in [modules/20-testing.md](../modules/20-testing.md).

2026-07-05 02:50. A 3×200k TD3+BC overnight was launched on the *documented* farming
baseline, whose reward spec certifies `delivers=False`. 135k steps were wasted before
the user caught it.

What makes this case worth keeping is which gate failed. The procedural gates all
passed — the production-scale smoke ran, observability was live, the run was
verifiable on disk. Every check that asks *is this running correctly* said yes. The
missing gate was semantic: *is this optimising the thing we meant*. No amount of
process discipline substitutes for it, which is why the remedy was to make the
entrypoint refuse an uncertified reward rather than to add another checklist item.
