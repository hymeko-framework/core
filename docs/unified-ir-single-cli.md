# Single-CLI inventory and consolidation note

**Written:** 2026-08-30 · **Scope:** audit + plan only. No CLI is created, moved, or
merged by this task (`HARD STOP` forbids new CLI/subcommands). This records the
current entrypoint inventory and the target shape so the consolidation is a
scoped, later production change.

## Invariant

There must ultimately be **one** user-facing CLI — `hymeko`, with subcommands.
Benchmarks, tests, and examples are development targets, not additional product
CLIs. A new entrypoint is justified only by a *proven* different deployment or
runtime boundary.

## Current inventory (across the six framework repositories)

| entrypoint | repo / path | kind | classification |
|---|---|---|---|
| `hymeko` | core `hymeko_core/src/main.rs` (bin = `hymeko_core`) | product CLI | **keep** — the one product CLI. Currently minimal: `hymeko <file.hymeko>` → compile → pretty-print. No subcommands yet. |
| `pgraph` | graph `hymeko_pgraph/src/bin/pgraph.rs` | product CLI (subcommands `read`/`transform`/`solve`/`generate`) | **fold in** — becomes `hymeko pgraph <sub>` (or `hymeko solve …`). Its subcommand shape is the model for `hymeko`'s. |
| `hymeko_pgraph_dump` | graph `hymeko_pgraph/src/bin/hymeko_pgraph_dump.rs` | debug/dump CLI (MSG/SSG/ABB) | **fold in or dev-only** — becomes `hymeko pgraph dump …`, or is marked a dev target. |
| `parser` | parser `parser/src/main.rs` | stub (`println!("Hello, world!")`) | **remove** — a placeholder `main`, not a product CLI. |
| `ir_depth_probe` | core `hymeko_core/examples/ir_depth_probe.rs` | example (dev) | not a product CLI — a stack-depth probe used by a test. Keep as example. |
| `alloc_count`, `arena_memory`, `crossover_sweep`, `phase7_probe`, `phase8_readback_bench`, `sustained_chain` | hnn/wgpu `examples/` | examples (dev) | measurement probes; not product CLIs. Keep as examples. |

Verified by: `find … -name main.rs` + `[[bin]]` grep across `core`, `graph`,
`network`, `parser`, `transformation`, `hotaru` (network/transformation/hotaru
have no bins).

## Verdict

**Two product CLIs (`hymeko`, `pgraph`) + one debug CLI (`hymeko_pgraph_dump`) +
one stub (`parser`).** The invariant (one `hymeko` CLI) is **not yet met**
(`UIR-CLI-04` = PARTIAL). The `hymeko` CLI's three externally-observable outcomes
are now under test (`UIR-CLI-01..03` = PASS).

## Target shape (later production change — NOT done here)

```
hymeko <file.hymeko>              # current: compile + pretty-print (default)
hymeko check   <file>            # compile only, exit code = validity
hymeko emit    <file> [--format hymeko|sysml|…]
hymeko pgraph  {read|transform|solve|generate|dump} <file> …   # from graph::pgraph
```

Consolidation is a `clap`-style subcommand dispatch in `hymeko_core/src/main.rs`
(a core edit, requires its own `APPROVED-CORE-EDIT`), with the graph P-graph
engine linked **as a library** — never a second binary. `parser`'s stub `main`
is deleted. No new binary is added; the count of product CLIs goes 2 → 1.
