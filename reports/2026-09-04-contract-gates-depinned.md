# 2026-09-04 — the contract gates stop being vendored here

**Created-at:** 2026-09-04 15:34 CEDT
**Commit:** `e827d1a`
**Pinned to:** `hymeko/hymeko` @ `c9856d7`

## Summary

This repository held its own copy of the contract tooling: 1403 lines across three
Python modules, plus a second, byte-identical copy of the CI workflow under
`tools/ci/`. It has been deleted. The hooks now run one implementation, from
`hymeko/hymeko`, pinned to a revision.

The copy had already drifted — it was the pre-rework version — and it was measuring
two things wrong.

## Discovery

```
DISCOVERY LEDGER — de-vendoring the contract gates
  git:      status --porcelain      -> DIRTY, but only with unrelated webxr work;
                                       staged contract paths only, verified after
  grep:     CORE.YAML               -> `.github/**` is in the ALLOWLIST, not locked;
                                       nothing else I touch appears
  grep:     tools.yaml              -> pins none of these tools; no §1 substitution
  cmp:      tools/ci/*.yml vs .github/workflows/*.yml -> IDENTICAL
  verdict:  DELETE the vendored modules; hooks call the pinned upstream.
            No §1 escalation required, checked rather than assumed.
```

## What the vendored version was measuring wrong

Both measured read-only against this tree before anything was changed.

### The index overstated the public surface

| | packages | symbols |
|---|---:|---:|
| committed, regex-based | 3 | **674** |
| same tree, tree-sitter | 3 | **667** |

Seven false positives, **zero** false negatives: `Producer`, `Sealed`,
`TensorAccess`, `empty_table`, `msg`, `payload`, `v`.

A regex over `pub (fn|struct|trait|enum)` counts `pub(crate)` as public surface, and
counts a `pub fn` inside a private `mod` as reachable — reachability is a property of
the parse tree, and a line matcher has no tree. The error runs in the direction that
hurts: someone consulting the index for a candidate to extend can be sent to a symbol
nothing outside the crate can reach.

(The committed index is now **633** symbols, not 667 — the three vendored Python
modules contributed 34 of their own and are gone.)

### The duplication baseline could not fail

```
committed  (line-hash):     2 clones /   52 cloned lines / 196 files
measured   (token, type-2): 417 clones / 8045 cloned lines / 186 files
```

**A 155× gap.** A line-hash detector only sees copies that nobody edited afterwards.
Every duplication in this project's recorded history was a copy that was then renamed
— which is exactly what a token detector sees and a line detector does not. The gate
as it stood would have passed almost any change, including the shape of change it
exists to catch.

Re-baselined at the real number, **fixing nothing in the same commit**.

**Read it as a starting point, not an indictment.** The worst files:

| file | cloned lines |
|---|---:|
| `hymeko_core/src/ir_stream/assembler.rs` | 663 |
| `hymeko_core/benches/ir_bench.rs` | 425 |
| `hymeko_wgpu/tests/activation.rs` | 346 |
| `hymeko_wgpu/tests/arena.rs` | 344 |
| `hymeko_wgpu/src/fanout_tests.rs` | 284 |

Four of those five are benches and tests, where repetition is frequently intentional
and a 60-token window is small for Rust. **`assembler.rs`, in `src/`, is the one that
deserves a look** — and that is a separate task, deliberately not this commit.

## The fast path, measured

`uv tool run --from <git spec>` re-resolves on every invocation. The hook makes three
calls, so that overhead is paid three times:

| | per call | hook total |
|---|---:|---:|
| `uv tool run --from <git spec>` | ~785 ms | 2.7 s |
| installed entry point on PATH | ~320 ms | **1.7–1.8 s** |

Budget is 2 s. The hooks now prefer `contract-symbols` on PATH and fall back to
resolving from git, and `install.sh` runs `uv tool install` so the fast path is the
default rather than a thing someone has to know about. Measured over three runs after
installation: 1819, 1711, 1734 ms.

## Files

```
deleted   tools/gen_symbols.py                 648
deleted   tools/preflight_new_artifact.py      358
deleted   tools/duplication_gate.py            297
deleted   tools/ci/duplication-gate.yml        100   (byte-identical to .github/)
rewritten tools/hooks/pre-commit                     pinned SPEC, fast path
rewritten tools/hooks/commit-msg                     pinned SPEC, fast path
rewritten tools/hooks/install.sh                     installs the pinned tool
rewritten .github/workflows/duplication-gate.yml     uses the pinned SPEC
regenerated SYMBOLS.md                               633 symbols, tree-sitter
re-established .duplication-baseline.json            tokens: 417 / 8045
```

1403 lines of vendored tooling removed; nothing added in its place.

## Verification

| check | result |
|---|---|
| `sh -n` on all three hooks | pass |
| workflow parses (PyYAML) | 2 jobs: `contract`, `language-gates` |
| hooks installed in `.git/hooks` | `pre-commit`, `commit-msg` |
| `contract-symbols --check` | exit 0 on the regenerated index |
| `contract-duplication` | 417 / 8045 = baseline, exit 0 |
| pre-commit wall time | 1.7–1.8 s (budget 2 s) |
| unrelated working tree preserved | 6 webxr entries, verified before and after |

## What was NOT verified

- **The workflow has never run.** No Actions runner is configured for this repository.
  It parses; that is all that can be said.
- **A cold `uv` cache.** Every timing here is with the tool already installed. The
  first commit after a fresh clone must reach amaterasu over SSH, and if amaterasu is
  down the hooks fall back to `uv tool run`, which will also fail — they then degrade
  to "uv not found" behaviour only if `uv` itself is missing, not if the *fetch* fails.
  That failure mode is untested and is the real cost of the pinned-remote model.
- **The 8045 number has not been triaged.** Nothing was inspected beyond the five
  worst files, and no judgement is offered about how much of it is intentional.
- **This repository's `CLAUDE.md` is still the 724-line monolith**, alongside a partial
  `docs/agent-contract/` (6 files against `hymeko/hymeko`'s 24 — no `60-hpc`, no
  `repo/`, no `incidents/`, no `README`/`MIGRATION`/`ROLLOUT`). Both a monolith and a
  split now exist here and the monolith is the one read first. That is the other half
  of this migration and it was not attempted.

## Follow-up

1. Finish the document split: replace the 724-line `CLAUDE.md` with the short root and
   sync `docs/agent-contract/` against `hymeko/hymeko`, or delete the partial copy and
   point at it. Two sources with the stale one authoritative is the worst state.
2. Triage `hymeko_core/src/ir_stream/assembler.rs` — 663 cloned lines in `src/`.
3. Configure an Actions runner, or delete the workflow rather than keep a gate that
   has never executed.
