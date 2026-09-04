<!-- SYNCED from hymeko/hymeko @ ff0c013 — docs/agent-contract/incidents/2026-06-04-kifu-and-proliferation.md
     Do not edit here. The universal modules have ONE source; a fork of §6.5
     in each repository makes every citation in every report ambiguous.
     Edit upstream, then re-sync and record the revision. -->

---
source: CLAUDE.md §6.5 closing note on items 13–16, lines 450-453, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: verbatim extraction — not yet de-projectified
---

# 2026-06-04 — the KIFÜ cascade and the v-suffix proliferation

The rules these two days produced are §6.5 items 13–16, in
[modules/30-code-health.md](../modules/30-code-health.md) and
[modules/60-hpc.md](../modules/60-hpc.md).

**Items 13–16 (added 2026-06-04 after the user spent two days on agent-induced KIFÜ warnings, v-suffix file proliferation, preamble loops, and memory-not-consulted patterns). These items are not theoretical; each maps to a specific incident in the project history. Future versions of the agent: if you find yourself about to do any of these, stop. The user has explicitly traded their time for these rules; do not waste that trade.**

---


## The proliferation, in detail

Extracted from §6.5 #13 in Phase 4.

`submit_hsikan_edge_cr_array.sh` had acquired `_v2` and `_v3` siblings, plus
`smoke_hsikan_edge_cr_v3.sh`, `sbatch_adaptive_mv_5seed.sh` and `launch_k_sweep.sh` —
five files, one experiment family. The v1 stayed on the cluster and was accidentally
re-invoked, which is what produced the three-warning resource-efficiency cascade.

The cleanup consolidated all five into one file with `smoke | full |
epinions-shuffle-rerun | k-sweep` modes. None of the duplicates should have existed:
each was created because editing the canonical file felt riskier than copying it, and
the copy is what carried the stale configuration forward into a live submission.
