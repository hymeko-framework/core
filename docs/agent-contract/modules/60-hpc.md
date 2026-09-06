<!-- SYNCED from hymeko/hymeko @ ff0c013 — docs/agent-contract/modules/60-hpc.md
     Do not edit here. The universal modules have ONE source; a fork of §6.5
     in each repository makes every citation in every report ambiguous.
     Edit upstream, then re-sync and record the revision. -->

---
source: CLAUDE.md §6.5 #16, lines 440-441, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: de-projectified (Phase 4)
---

**Contains:** §6.5 #16 HPC array sizing

# HPC

The thinnest module: the contract has exactly one cluster rule. It is kept as its own
file because cluster work fails in its own way, and because the anchor `§6.5 #16` is
cited elsewhere — [30-code-health.md](30-code-health.md) keeps a stub at that position.

16. **HPC sizing without a per-cell-class measurement.** Never submit a SLURM array with a uniform `--time` for a heterogeneous cell grid. Before any new HPC array: (a) check actual measured walls from prior runs (JSONLs, sacct, logs), (b) set class-aware `--time` with TimeEff target ≥ 25% on the fastest class, (c) run a 1-cell `--array=N` smoke per class and verify `seff <jobid>` TimeEff before scaling. The three-warning cascade in eight hours that produced this rule — a uniform `--time=02:30:00` across 25 cells whose actual wall was 30 s, so 0.3% TimeEff, tripping the cluster threshold immediately: [incidents/2026-06-04-kifu-and-proliferation.md](../incidents/2026-06-04-kifu-and-proliferation.md).

