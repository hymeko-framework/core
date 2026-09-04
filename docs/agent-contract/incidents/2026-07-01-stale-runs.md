<!-- SYNCED from hymeko/hymeko @ ff0c013 — docs/agent-contract/incidents/2026-07-01-stale-runs.md
     Do not edit here. The universal modules have ONE source; a fork of §6.5
     in each repository makes every citation in every report ambiguous.
     Edit upstream, then re-sync and record the revision. -->

---
source: CLAUDE.md §6.5 anti-pattern 17, line 442, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: extracted in Phase 4 from the rule it motivates; host-specific detail kept here deliberately
---

# 2026-07-01 — a run that outlived its relevance, and children that outlived their parent

The rule this produced is §6.5 **#17 orphaned background processes / stale runs**, in
[modules/30-code-health.md](../modules/30-code-health.md). The rule is general; the
detail below is specific to this host and belongs here rather than in a module.

**Three hours past relevance.** A cartpole benchmark kept running long after its result
had stopped mattering, holding a core, while a newly launched run "felt slow". Nothing
was broken and nothing reported an error — the only symptom was a wall-clock that could
not be trusted, and every measurement taken alongside it was invalid.

**Zombie children.** Killing a run's parent process left its `ProcessPoolExecutor`
children thrashing. They starved the next launch to *zero output for 44 minutes*, which
presented as a hung new run rather than as a surviving old one.

**This host, specifically.** Listing interpreters is `Get-Process python` with
`StartTime` and `CPU` under PowerShell. The old page-file / `WinError 1455` constraint
("a lapozófájl túl kicsi a művelethez") that once forced one torch run at a time was
**resolved 2026-07-03** — the page file was enlarged and the user confirmed it — so
overlapping runs are permitted now. The remaining limit is GPU memory and compute
contention, not sparse virtual address space.

The same session also produced [the metric-integrity
cases](2026-07-01-metric-integrity.md) and [65 minutes of silence
](2026-07-01-sixty-five-minutes-dark.md).
