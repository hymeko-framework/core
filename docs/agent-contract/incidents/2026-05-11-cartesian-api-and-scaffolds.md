<!-- SYNCED from hymeko/hymeko @ ff0c013 — docs/agent-contract/incidents/2026-05-11-cartesian-api-and-scaffolds.md
     Do not edit here. The universal modules have ONE source; a fork of §6.5
     in each repository makes every citation in every report ambiguous.
     Edit upstream, then re-sync and record the revision. -->

---
source: CLAUDE.md §6.5 anti-patterns 1, 2, 3, lines 410-414, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: extracted in Phase 4 from the rules it motivates
---

# 2026-05-11 — three duplications found in one audit

The rules these produced are §6.5 **#1 Cartesian-product API surface**, **#2 algorithm
code behind a binding boundary** and **#3 per-experiment scaffold duplication**, in
[modules/30-code-health.md](../modules/30-code-health.md).

**Sixteen functions for four axes.** `hymeko_py/src/cycles.rs` carried 16
`#[pyfunction]` variants differing only by orthogonal axes — filtered, batched,
branch-and-bound, tiered, global, adaptive, signed, colour-coded, path-closure. Each new
axis had been added by inventing a new function name, and each carried its own copy of
the same `match score_kind` ladder. The axes were configuration; the names were the
symptom.

**About 1100 lines of algorithm in the binding crate.** The same file held
`build_csr`, `bfs_distances_into`, `dfs_recurse`, `enumerate_parallel`,
`canonical_cycle`, `dfs_color_coded`, `try_one_path_closure`, `dfs_walks_*` — pure Rust
graph logic, none of it about crossing the Python boundary, sitting in the crate whose
only job is numpy↔ndarray glue. The algorithm crate was one directory away.

**Ninety-eight experiment scripts.** `hymeko_neuro/` held 98 `run_*.py`, among them 8
reimplementations of `_train_val_split` and 69 independent AUC evaluation loops. Each
script had been started by copying the nearest one.

Nothing here was written by someone careless. Each duplication was locally the cheapest
next step; the cost only exists in aggregate, which is why the rules are stated as
preconditions on creation rather than as cleanup advice.

Related memory: `feedback_no_cartesian_pyfunction_dump.md`.
