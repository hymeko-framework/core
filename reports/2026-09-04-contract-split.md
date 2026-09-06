# 2026-09-04 — the 724-line CLAUDE.md becomes 150

**Created-at:** 2026-09-04 16:41 CEDT
**Commit:** `063cce2`
**Synced from:** `hymeko/hymeko` @ `ff0c013`

## Why

Stated by the user, and it is the whole justification: *a 724-line CLAUDE.md does not
get read.* This repository had ended up in the worst possible state for that — **both**
a 724-line monolith and a partial `docs/agent-contract/` (6 files), with the monolith
being the file an agent opens first.

## What is here now

```
CLAUDE.md              150 lines   persona, non-negotiables, paradigm hierarchy,
                                   §1, §12, the gates, the module list
docs/agent-contract/   21 files    global + modules 10-60 + 10 incidents + repo/
```

Down from 724 lines in one file to 150 in the file that is read first.

## One source, verified

The universal half — `global.md`, `modules/`, `incidents/` — is **synced from
`hymeko/hymeko`, not authored here.** Each of the 17 files carries
`SYNCED from hymeko/hymeko @ ff0c013` as its first line.

**All 17 are byte-identical to upstream once that banner is stripped** — checked with
`diff`, not asserted. A fork of §6.5 per repository would make every `§`-citation in
every report ambiguous, which is the failure the split exists to avoid.

`repo/` is this repository's own, because §4, §10 and §11 describe *this* project:

- **§11** links `docs/hymeko-language.md`, which exists here. Upstream that link had to
  be removed because the file lives in this repository; here it is live again.
- **§10** drops upstream's "Python projects are uv projects" section — this repository
  has no `pyproject.toml` and does not need one. It describes the pinned-tooling model
  instead, and records what the vendored copy cost when it drifted
  (`reports/2026-09-04-contract-gates-depinned.md`).

## Coverage

Measured against the pre-split file, whitespace-normalised:

```
source lines (non-blank, non-rule, non-fence):  480
appear verbatim in the new tree:                430
deliberately reworded:                           50 distinct
lost accidentally:                                0
```

The 50 are Phase 4's de-projectification and today's template updates, each traceable to
a commit that states what it changed:

| was | now |
|---|---|
| `make symbols`, `rg` over `pub fn` | a description of the tool that now exists |
| §6.5 #1, #2, #3, #12, #13, #16 narratives | moved to `incidents/`, rule keeps a reference |
| "Both subsections are gates" | "Its subsections" — §6 has had seven since §6.7 |
| `G-SPHF kernels, KAN basis evaluation` | generalised |
| `PowerShell Get-Date` | the system clock, with the shell as an example |
| §0.2 / §6.7 "specified, not built" | "built and enforced" |

The upstream tree was verified **480/480 with 0 dropped** against this exact file before
any rewording; the rewordings are commits, not losses.

## The source was the working tree, not HEAD

`core/CLAUDE.md` carried an uncommitted 88-line edit. The split was taken from the
working tree at 03:26, and before writing anything the source was re-verified
**content-identical modulo line endings** to the current file. That edit is included in
this commit rather than discarded.

## Verification

| check | result |
|---|---|
| `CLAUDE.md` | **150 lines**, ceiling 150 |
| every link in `CLAUDE.md` | resolves |
| every link inside `docs/agent-contract/` | resolves |
| 17 synced files vs upstream | byte-identical modulo banner |
| every synced file names its source revision | 17/17 |
| `contract-symbols --check` | exit 0 |
| `contract-duplication` | 417 / 8045 = baseline, exit 0 |
| unrelated working tree preserved | 5 webxr entries, before and after |

## What was NOT verified

- **That the modules still read correctly in their new neighbours.** The coverage check
  is a text-identity check. It proves nothing was lost; it cannot prove a rule reads
  right where it now sits, and nothing here was read back by a second party.
- **Re-syncing.** There is no `--check` for the synced files. Keeping them in step with
  `hymeko/hymeko` is manual, and the banner records the revision precisely so drift is
  *detectable* — not so that it is prevented. A sync gate is the obvious next tool and
  does not exist.
- **The `§`-citation corpus.** `reports/` and `docs/` here carry hundreds of `§N`
  references. The numbering was preserved specifically so they keep resolving, but they
  were not enumerated and checked one by one.
- **Whether a 150-line root actually gets read.** That is the entire premise and it is
  not something this report can establish.

## Follow-up

1. A sync gate: `contract-docs --check` comparing the 17 synced files against their
   recorded revision, wired into the hook beside the index check.
2. Merge `hymeko/hymeko`'s `chore/contract-tooling` into `main` and re-pin. The hooks
   here point at `c9856d7`, which exists only on that branch.
3. Roll the same split into `tensor`, the other repository with a `CLAUDE.md`.

## 2026-09-04 consumer-tooling migration

The provider was merged to `hymeko/hymeko` `main` at
`78825abc2911c694bb47fc7a35b645fc970fac08`. This consumer now pins the immutable
implementation commit
`515f285886495fdcb5866f2ddafe1889a99757ab`, which introduces the registered
`contract-self-check` and the role-aware hook template.

The aggregate self-check remains provider-only. The consumer pre-commit carries
the canonical `tools/contract_tools/self_check.py` guard, which is deliberately
false here; core continues to run only the portable symbol, duplication and
preflight gates from the installed pinned package.

Canonical-clone discovery under `hymeko_framework/` found only one adopting
consumer: core. The other checked-out repositories had none of `SYMBOLS.md`,
`.duplication-baseline.json`, `tools/hooks/`, or `docs/agent-contract/`, so they
were not silently converted into adopters by this upgrade.

| check | result |
|---|---|
| provider `contract-self-check` | 8/8 registered gates passed; 134 tests passed |
| new consumer `contract-symbols --check` | 613 symbols, exit 0; no regeneration |
| new consumer `contract-duplication` | 403 / 7800 = baseline, exit 0 |
| tracked vs installed hooks | pre-commit and commit-msg installed, current |
| synthetic preflight candidate | 0.96 match against this report |
| commit-msg without `DISCOVERY:` | blocked, exit 1 |
| commit-msg with `DISCOVERY:` | released, exit 0 |
| pre-commit wall time | 2.125 s cold; 2.079 s warm |

The warm hook exceeds the rollout's 2 s budget by 79 ms and is therefore a
small, measured performance defect rather than a passing claim. The synthetic
file and both message files were removed after the end-to-end test.

The earlier follow-up item 2 is closed: the tooling is now on provider `main`
and core no longer pins the branch-only `c9856d7`. Follow-up item 1 is still
open: provider commit `515f285` does **not** implement `contract-docs --check`,
so the 17-file document synchronization remains banner-detectable but manual.
