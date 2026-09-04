# The agent contract

An operating contract for agents working in this ecosystem, split out of a single
724-line `CLAUDE.md` so that a reader can open the part that governs the work in
front of them instead of all of it. That was the whole point: a 724-line file does
not get read.

**The universal half is synced from `hymeko/hymeko` @ `ff0c013`, not authored here.**
`global.md`, `modules/` and `incidents/` each carry the source revision in their first
line — edit them upstream and re-sync. `repo/` is this repository's own, because §1,
§4, §10 and §11 describe *this* project.

Start at [`../../CLAUDE.md`](../../CLAUDE.md). It is the only file read first: the
persona, the non-negotiables, this repository's own rules, and a pointer to
everything here. Nothing in this directory restates it.

```
CLAUDE.md                  persona, non-negotiables, this repo, the module list
docs/agent-contract/
  global.md                §0 the order of work; the nine principles in full
  modules/
    10-discovery.md        §0.1 §0.2 §6.1 §6.6 §6.7  — search, ownership, ledger, gate
    20-testing.md          §3   — layers, coverage, determinism, benchmarks, metrics
    30-code-health.md      §5 §6.2–§6.5 §7 §8  — layout, complexity, lint, errors,
                                anti-patterns, patterns, contracts
    40-observability.md    §3   — never run blind
    50-reporting.md        §2 §9 — the plan bundle, the report, graphical output
    60-hpc.md              §6.5 #16 — cluster array sizing
  incidents/               what the dated rules cost, kept out of the rules
  repo/                    §4 §10 §11 — this repository's own
```

What moved where, and what was deliberately not dropped, is in
`reports/2026-09-04-contract-split.md`.

**The numbering is the original.** Modules keep `§6.1`, `§6.5 #16`, `§9` as headings
rather than renumbering, so a citation written in a report before this split still
resolves. That is why a module's internal ordering looks arbitrary — `10-discovery.md`
runs §0.1, §0.2, §6.1, §6.6, §6.7 — and why each module opens with a contents line.

**A module is binding.** It is not background reading. The root file carries the three
gates you must not have to open a module to discover; everything else is read when its
subject comes up, before the work rather than after it.

**The incidents are not rules.** They are the dated cases the rules came from, moved
out so that a rule reads as a rule. A module that cites one links to it.
