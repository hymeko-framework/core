<!-- SYNCED from hymeko/hymeko @ ff0c013 — docs/agent-contract/incidents/2026-07-01-sixty-five-minutes-dark.md
     Do not edit here. The universal modules have ONE source; a fork of §6.5
     in each repository makes every citation in every report ambiguous.
     Edit upstream, then re-sync and record the revision. -->

---
source: CLAUDE.md §3 "Live observability", line 266, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: extracted in Phase 4 from the rule it motivates
---

# 2026-07-01 — sixty-five minutes with zero output

The rule this produced is **Live observability — never run blind**, in
[modules/40-observability.md](../modules/40-observability.md).

An off-policy training run went **65 minutes with zero output**. Logging existed: it
fired at seed boundaries, which on that run meant almost never. One periodic hook also
existed — and it was silent, and it had been left defaulting to a metric from an
entirely different task. Three separate blind waits happened before it was fixed.

The lesson is narrower than "add logging", because logging was present. It is that a
hook which accumulates into a list without printing is indistinguishable, from outside,
from no hook at all; and that a signal nobody has watched fire is not a signal. The
remedy was to make the training loop itself print `step | crit/act loss | steps/s | ETA
| buf` on an interval by default, so that live feedback is a property of the library and
not something each script has to remember.

The same session also produced [the metric-integrity
cases](2026-07-01-metric-integrity.md) and [the stale-run
cases](2026-07-01-stale-runs.md). It was an expensive day.
