<!-- SYNCED from hymeko/hymeko @ ff0c013 — docs/agent-contract/incidents/2026-06-03-duplicate-scaffolding.md
     Do not edit here. The universal modules have ONE source; a fork of §6.5
     in each repository makes every citation in every report ambiguous.
     Edit upstream, then re-sync and record the revision. -->

---
source: CLAUDE.md §6.1 "Why the search precondition", lines 336-342, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: verbatim extraction — not yet de-projectified
---

# 2026-06-03 — two duplicates that a grep would have prevented

The rule these cases motivate is §6.1, in
[modules/10-discovery.md](../modules/10-discovery.md).

**Why the search precondition.** The repository has substantial history: ~200 reports, ~80 plan dirs, multiple Rust crates (`hymeko_core`, `hymeko_hre`, `hymeko_query`, `hymeko_formats`, `hymeko_graph`, `hymeko_pgraph`, `hymeko_compute`, `hymeko_monitor`, `hymeko_py`, `parser`), several Python packages (`hymeko_neuro/{core,cycle_cache,mixed_arity_signedkan,triton_kernels,arch_search,...}`), and a dense fixture set under `data/`. The probability that a "new" concept already has scaffolding somewhere is high. Writing without the search pass duplicates work the user remembers doing, erodes trust ("ezt már csináltunk"), and creates merge debt later when the two implementations must be reconciled. Two precedents from 2026-06-03:

1. Built a `ReservoirSampler` + ABB DFS framework from scratch under `hymeko_neuro/hyperedge/{reservoir,abb_walks,path_scorers}.py` before checking that `hymeko_graph/src/{friedler,pruner,cycle_enum,topk_cycles}.rs` already contained the `BoundedScorer` trait, the Friedler axiom pruner, and the rayon-parallel B&B enumerator. ~1700 LOC + 56 tests of net-new work where ~600 LOC of bindings would have served.
2. About to `mkdir hymeko_neuro/pgraph/` for the Pimentel benchmark before checking that `hymeko_pgraph` crate already exists with full schema / A1–A5 axioms / MSG / SSG / ABB / `book_validation` test suite that already passes Pimentel's Examples 3.2 / 6.1. The duplicate package was prevented by the user's explicit reminder ("a meglévő pgraph-hymeko könyvtárra építkezzünk"). Without that reminder, ~800 LOC of duplicate would have shipped.

The search is cheap (seconds); the duplicate is expensive (hours of incidental work, plus the dignity cost of being told "ezt már megcsináltuk"). Treat the search as a hard precondition on every `mkdir`, every `Write` of a new file, every `add_function` impulse — not a suggestion.

