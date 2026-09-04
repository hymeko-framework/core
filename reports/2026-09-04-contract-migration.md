# Agent-contract migration into `hymeko/core`

**Date:** 2026-09-04
**Branch:** `chore/agent-contract` (not pushed, no PR)
**contract-source:** `cd926c87ceda63acfe77a66e31b4366cb878de3a` (`hymeko/hymeko`, clean tree)
**Commits:** `b7bd022` documents, `c00a038` tooling
**Base:** `7019940` feat(core): add canonical CBOR codec

Both layers landed. This repository now has enforcement, not only text — see
[Verification](#verification) for what was actually exercised, and
[Conflicts](#conflicts-not-resolved) for what the migration surfaced and did not fix.

---

## Discovery

Copied verbatim from the chat reply, per §6.6.

```
DISCOVERY LEDGER — agent contract (documents + tooling)
  find:    find . -iname '*agent-contract*' -o -iname '*agent_contract*'
             -> 0 hits (only .git refs for the new branch)
  find:    find . -iname 'gen_symbols*' -o -iname 'preflight*'
             -o -iname '*duplication*' -o -iname 'SYMBOLS*'
             -> 3 hits: docs/plans/2026-09-01-benchmark-harness-consolidation/
                        duplication-inventory.md,
                        docs/unified-ir-duplication-and-profile-audit.md,
                        hymeko_core/src/writers/cbor_writer/preflight.rs
  find:    hooks / .github / pre-commit / commit-msg   -> 0 hits outside .git/
  symbols: SYMBOLS.md does not exist in this repo      -> 0 hits
  read:    tools/ (build_ifac_materials.ps1, cuda_prototype/), docs/ listing,
           hymeko/tools/*
  verdict: EXTEND tools/ (exists; contract scripts added beside the existing two)
           NEW docs/agent-contract/; no candidate found
           NEW SYMBOLS.md, .duplication-baseline.json; no candidate found
           rejected hymeko_core/src/writers/cbor_writer/preflight.rs because it is
             CBOR allocation preflight on a decode path — same word, unrelated
             concept, core-owned
           rejected docs/*duplication*.md because both are one-off audit prose,
             not a gate
```

The installed hook independently reached the same conclusion on
`tools/preflight_new_artifact.py`, scoring it `1.00` against
`cbor_writer/preflight.rs` and blocking until the `DISCOVERY:` line was present.
The gate fired on its own installation.

---

## Repository characterisation

Measured, not assumed.

| | |
|---|---|
| languages | **Rust only** — 181 `.rs`, 44,410 LOC. 0 Python, 0 C/C++ |
| other tracked content | 120 `.hymeko` fixtures, 81 `.md`, 22 each `.tex`/`.tikz`/`.mmd` |
| packages | 2 crates: `hymeko_core`, `hymeko_wgpu`; virtual workspace root |
| manifests | `Cargo.toml`, `hymeko_core/Cargo.toml`, `hymeko_wgpu/Cargo.toml` |
| test framework | `cargo test`; 11 suites |
| CI | **none** — no `.github/`, `.gitlab-ci.yml` or `.gitea/` before this change |
| git hooks | **none** installed (samples only) |
| cluster / SLURM | **none** — the only `.sh` is `tools/cuda_prototype/build.sh`, a local build |
| tracked files | 511 at Phase 0; 529 at migration time |
| pre-existing contract files | `CLAUDE.md` (724 ln, **uncommitted**), `CORE.YAML` (245 ln), `tools.yaml` (177 ln) |
| absent | `SYMBOLS.md`, `.duplication-baseline.json`, `Makefile` |

**Host:** Windows 11 26200, MINGW64 shell, rustc 1.97.1, cargo 1.97.1,
git 2.55.0.windows.3, Python 3.12.13 (uv-managed).

### The finding that shaped Phase 2

The working-tree `CLAUDE.md` is git blob `ee90ee4593ab48f5ed73b6e985bdfbd4cab03f77`
— **the identical object** `hymeko/hymeko` extracted the contract from at
`ad1df25a`. `MIGRATION.md` says it imported "core/CLAUDE.md (working tree at core
8664791)"; that is literally true, and it means the 724-line document the whole
`docs/agent-contract/` tree derives from has never been committed anywhere.

```
core worktree CLAUDE.md      blob ee90ee45…   724 lines   (uncommitted)
hymeko@ad1df25a:CLAUDE.md    blob ee90ee45…   724 lines   (same object)
core HEAD:CLAUDE.md                           637 lines
core origin/main:CLAUDE.md                    567 lines
```

So the incoming document is a **descendant of the file it sits beside**, not a
competitor. It survives in exactly two places, one of which is an uncommitted
working tree.

---

## Modules activated

| module | activated | reasoning |
|---|:--:|---|
| `global.md` | yes | §0 workflow and the nine principles are language-neutral |
| `10-discovery.md` | yes | §6.1/§6.6/§6.7 are the rules the tooling enforces; the repo has ~200 reports and ~80 plan dirs, exactly the history the search precondition exists for |
| `20-testing.md` | yes | 11 suites, frozen KAT digests, benchmark harness — the layer rules and determinism section apply directly |
| `30-code-health.md` | yes | anti-patterns, complexity, error handling; the §6.5 Rust items (`Vec<T>` over `Vec<Box<dyn Trait>>`, `unwrap` discipline) are on-point |
| `40-observability.md` | yes | benches and corpus validation are multi-minute; the "never run blind" rule holds even without training runs |
| `50-reporting.md` | yes | §2 plan bundle and §9 report are already this repo's practice (22 plan dirs carry `.tex`/`.tikz`/`.mmd`) |
| `60-hpc.md` | **no** | no SLURM, sbatch, array submission or cluster path exists. `tools/cuda_prototype/build.sh` is a local build script, not a cluster artifact |
| `repo/` | **no** | per instruction — §4 cap, §10 toolchain, §11 authoring stay in the existing `CLAUDE.md`; `tools.yaml` already pins §10 |
| `incidents/` | **no** | per instruction — the source repo's dated cases |

**tools.yaml languages:** already Rust-only (`schema_version`, `host`, `rust`,
`docs`, `output_formats`, `policies`). The pruning step was a **no-op** — there
were no Python or C/C++ sections to remove.

**CORE.YAML:** kept verbatim per your Phase 1 decision. Unmodified by this branch
(`git diff HEAD~2 -- CORE.YAML` is empty).

**Peak-RSS cap:** 16 GB, unchanged, per your Phase 1 answer.

---

## Files added

17 files, 3,180 lines, in two commits.

### `b7bd022` — documents (7 files, 730 lines)

| file | lines | note |
|---|---:|---|
| `docs/agent-contract/global.md` | 43 | byte-identical to source |
| `docs/agent-contract/modules/10-discovery.md` | 145 | byte-identical |
| `docs/agent-contract/modules/20-testing.md` | 86 | byte-identical |
| `docs/agent-contract/modules/30-code-health.md` | 186 | byte-identical |
| `docs/agent-contract/modules/40-observability.md` | 21 | byte-identical |
| `docs/agent-contract/modules/50-reporting.md` | 99 | byte-identical |
| `CLAUDE.incoming.md` | 150 | at the ceiling, not over |

Every copied document was verified with `cmp` against the source. None was edited.

### `c00a038` — tooling (10 files, 2,450 lines)

| file | lines | note |
|---|---:|---|
| `tools/gen_symbols.py` | 648 | verbatim |
| `tools/preflight_new_artifact.py` | 358 | verbatim |
| `tools/duplication_gate.py` | 297 | verbatim |
| `tools/hooks/pre-commit` | 86 | verbatim |
| `tools/hooks/commit-msg` | 87 | verbatim |
| `tools/hooks/install.sh` | 66 | verbatim |
| `tools/ci/duplication-gate.yml` | 100 | verbatim |
| `.github/workflows/duplication-gate.yml` | 100 | copy of the above, **inert** |
| `SYMBOLS.md` | 694 | generated here |
| `.duplication-baseline.json` | 14 | measured here |

No script was forked or adapted. All seven copied tools verified byte-identical
with `cmp`.

---

## Symbol index

```
python3.12 tools/gen_symbols.py
wrote SYMBOLS.md: 3 package(s), 674 symbol(s)
wall: 170 ms          output: 694 lines, 37,051 bytes
```

| package | symbols |
|---|---:|
| `hymeko_core` | 446 |
| `hymeko_wgpu` | 200 |
| `core` (virtual workspace root) | 3 |

`--check` passes against the committed index (exit 0).

**Small tool observation:** the virtual workspace root (`Cargo.toml` with a
`[workspace]` table and no `[package]`) is listed as a package named `core` and
contributes charter metadata but no symbols. Cosmetic; reported rather than
patched, per the no-forking rule.

---

## Duplication baseline

First measurement of this repository's current state. **Not a verdict.**

```
duplication (builtin) in core: 2 clone(s), 52 cloned line(s) across 196 file(s)
params: min_lines 20, min_tokens 120
```

All 52 cloned lines sit in a single file:

```
hymeko_core/tests/intermediate_tests/ir_transformation_tests.rs   52 cloned line(s)
```

**Clone delta (§6.7):** `duplication (builtin): 2 clone(s), 52 cloned line(s)
across 196 file(s); baseline 2 / 52` — exit 0, no increase. The migration adds
2,450 lines of Python and introduces **zero** new clones.

No §6.5 anti-patterns introduced: no new Rust artifact, no new public symbol in
either crate, no new function family, no new dependency.

---

## Preflight calibration

The instruction asked for 20 random existing filenames. The source repo's own
report explains why that is a poor noise measure — a name whose family already
exists *should* fire — so both measurements are given.

**(1) Twenty existing names from this repo** (seed 20260904):

```
FIRED 19/20    91 candidates total    mean 4.8 when firing
```

That is the tool working. The one non-firing case was `data/robotics/README.md`.

**(2) Control — twenty names from an unrelated repo**, proposed against core.
`vscode-plugin` (TypeScript, 29 tracked files) was used because the source repo
used the same corpus:

```
core   (529 tracked files, 640 symbols):  FIRED 14/20  ->  70%
```

The source repo reports **25%**. That comparison does not hold as stated: the
source measured a *different* random sample, and measured it against `graph`,
not against itself. So the same 20 names were run against the source repo to
isolate the variable:

| target repo | tracked files | fired | rate |
|---|---:|---:|---:|
| `hymeko` (contract source) | 70 | 8/20 | **40 %** |
| `core` (this repo) | 529 | 14/20 | **70 %** |

Same sample, same tool, same default threshold (0.65). **The difference is corpus
size**, not a per-repo tuning need: ~7.5× the files gives ~1.75× the firing rate.

A visible share of core's firings are exact `1.00` matches on universal filenames
that carry no information — `README.md`, `LICENSE`, `.gitignore`, `main.rs`,
`tests.rs`. The matcher has no stop-list for ubiquitous names.

**The threshold was not changed.** This is reported as a finding about the shared
tool, per instruction.

---

## Verification

| check | result |
|---|---|
| `gen_symbols.py --check` against committed index | **PASS** (exit 0) |
| `duplication_gate.py` against committed baseline | **PASS** (exit 0, 2/52 vs 2/52) |
| hooks installed in `.git/hooks` | **PASS** — `pre-commit installed, current`; `commit-msg installed, current` |
| hook end-to-end: pre-commit blocks | **PASS** — staged `hymeko_core/src/ir/coherence_probe.rs`; pre-commit caught the stale index (detected the new `scratch_probe` symbol), ran the clone gate (2/52, no increase) and preflight (`1.00` vs `coherence.rs` ×2). No commit created. |
| hook end-to-end: commit-msg blocks | **PASS** — with the index refreshed, commit-msg refused the message with no `DISCOVERY:` line. No commit created. |
| hook end-to-end: `DISCOVERY:` bypass | **PASS** — message carrying a `DISCOVERY:` line committed as `1f06d94` |
| scratch unwound | **PASS** — `git reset --soft`, unstaged, file removed, index regenerated to 674 symbols. `grep -c scratch_probe SYMBOLS.md` = 0 |
| CI workflow parses | **PASS** — `yaml.safe_load` OK; `name: contract gates`; 3 jobs (`symbol-index` 3 steps, `duplication` 6 steps, `language-gates` 4 steps) |
| `CLAUDE.incoming.md` readable, non-empty | **PASS** — 13,189 bytes, 150 lines |
| `CLAUDE.md` readable, non-empty | **PASS** — 74,051 bytes, 724 lines, untouched |
| `tools.yaml` readable, non-empty, parses | **PASS** — 6,857 bytes, 177 lines, 6 top-level keys |
| `CORE.YAML` readable, non-empty | **PASS** — 11,581 bytes, 245 lines |
| `CORE.YAML` parses as YAML | **FAIL** — pre-existing, see Conflicts #1 |
| line count vs 150 ceiling | **PASS** — 150, at the ceiling |
| repo's own test suite | **PASS** — 552 passed, 0 failed at `c00a038`; see below |

### Test suite

Measured in clean detached worktrees so the in-progress webxr work in your
working tree could not contaminate the result.

| tree | scope | result |
|---|---|---|
| `c00a038` (after migration, committed) | full `--workspace`, 15 suites | **552 passed, 0 failed, 8 ignored** — exit 0 |
| `7019940` (base, committed) | `-p hymeko_core --test mod` | **243 passed, 0 failed, 7 ignored** |
| `c00a038` (after migration, committed) | same suite, isolated | **243 passed, 0 failed, 7 ignored** — identical |
| your working tree (webxr in progress) | full `--workspace` | 241 passed, **2 failed**, 7 ignored |

The like-for-like row is the third: the `hymeko_core --test mod` suite gives
**243 / 0 / 7 both before and after** the migration. The migration adds files and
a hook and changes no test outcome.

The working-tree run aborted after the failing suite (cargo stops on failure), so
it reports only 3 suites; its 135 lib unit tests vs the committed 134 is the
in-progress `webxr/query.rs` work, not the migration.

The two working-tree failures are **not caused by this migration** and predate
it. Both share one root cause:

```
v1_corpus_digest_is_unchanged
  the number of lowerable fixtures changed (106 vs 105)
every_lowerable_corpus_program_round_trips_through_the_public_facade
  the codec gate must cover the frozen corpus (106 vs 105)
```

The 106th lowerable fixture is the untracked
`data/webxr/maxwell_fiducial_demo.hymeko`; `KAT_CORPUS_COUNT` is still 105. The
discriminating test is the base-worktree run above: at committed `7019940`,
without that file, both tests pass. Re-freezing is the webxr work's business, via
`print_corpus_digest_for_capture`.

---

## New / removed dependencies

**None** in either crate. `Cargo.toml`, `Cargo.lock` and `CORE.YAML` are
untouched by this branch.

The tooling requires **Python 3.11+ with no third-party packages**. It is not
declared in `tools.yaml` — see Conflicts #4.

---

## Conflicts (not resolved)

Listed, not fixed, per instruction.

**1. `CORE.YAML` is not valid YAML.** Pre-existing; unchanged by this branch.

```
yaml.scanner.ScannerError: while scanning a simple key
  in "CORE.YAML", line 219, column 7 — could not find expected ':'
```

Lines 217, 218 and 220 are block-sequence items containing an unquoted
`lockdown: ` (colon-space). Backticks are markdown, not YAML quoting, so the
parser reads a mapping inside the list item:

```yaml
    - any modification to a `lockdown: full` item
    - any modification to a `lockdown: implementation` item beyond
    - any modification to `lockdown: api` items in core crates
```

This has been latent because nothing parses the file — agents read it as prose.
It matters now: the contract's §1 says *"If `CORE.YAML` is missing, malformed, or
unreadable: halt and ask"*, and any future gate that loads it will fail. **Not
fixed:** the file is `lockdown: full` on itself and lists "any change to this
file" as requiring approval. The fix is three pairs of quotes. `tools.yaml`
parses cleanly.

**2. The "portable set" is not self-contained — a template defect.** The five
copied modules carry **nine** links into `incidents/` and one into `60-hpc.md`,
neither of which the migration instruction permits copying:

```
global.md            -> incidents/2026-06-28-novelty-search-miss.md
10-discovery.md      -> incidents/2026-06-03-duplicate-scaffolding.md
20-testing.md        -> incidents/2026-07-01-metric-integrity.md
                     -> incidents/2026-07-05-uncertified-reward-launch.md
30-code-health.md    -> incidents/2026-05-11-cartesian-api-and-scaffolds.md
                     -> incidents/2026-06-03-duplicate-scaffolding.md
                     -> incidents/2026-06-04-kifu-and-proliferation.md
                     -> incidents/2026-07-01-batching-the-non-bottleneck.md
                     -> incidents/2026-07-01-stale-runs.md
                     -> modules/60-hpc.md
40-observability.md  -> incidents/2026-07-01-sixty-five-minutes-dark.md
```

Copying verbatim leaves them dangling; editing them forks the shared documents,
which is the failure the contract exists to prevent. **This is a template defect,
not a local one** — it will recur in every repository that adopts the portable
set, and `30-code-health.md`'s `60-hpc.md` link means the modules are not
independently activatable. Left dangling and recorded.

**3. `.github/workflows/` is inert, and the contract's own reasoning says not to
place it there.** `tools/ci/duplication-gate.yml` carries a comment explaining it
is deliberately *not* under `.github/` because a workflow that has never run
"would be a file that has never run claiming to be a gate". Core has the same
property — its remote is gitea on `amaterasu.local`, with no Actions runner
found. The migration instruction overrides ("place the workflow file anyway and
report that it is inert"), so it is placed and reported. The tension between the
instruction and the file's own comment is the conflict.

**4. `tools.yaml` pins no Python, and the repo now runs Python.** This was a
Rust-only repository; `tools.yaml` correctly had no Python section, and the
pruning step was a no-op. The tooling is Python 3.11+. Nothing in `tools.yaml`
records that, so §10's "one canonical tool per concern per language" no longer
covers everything the repo executes. Not added — that is a `tools.yaml` decision.

**5. `python3` on this host is a Microsoft Store stub.** `command -v python3`
resolves to `WindowsApps/python3`, which prints an install advertisement and
exits without running anything. **The shared hooks already handle this** — they
probe each candidate with a real execution test and honour `CONTRACT_PYTHON`, and
the source repo's comments show it hit the same defect. Recorded as confirmation
that the upstream fix works here, not as an open issue. Note the hook selected
`python3.14` while this report's manual runs used `python3.12.13`.

**6. `__pycache__` was not ignored — RESOLVED.** Running the hooks creates
`tools/__pycache__/`, which showed as untracked. This repository's `.gitignore`
had no `__pycache__` or `*.pyc` entry because it never had Python. Since the
noise is a direct byproduct of the tooling this migration installed, and
`.gitignore` is not in `CORE.YAML`, it was fixed here rather than left: two lines
appended, verified by running `preflight_new_artifact.py` (which imports, so it
does create the directory) and confirming `git status` stays clean.

**7. `CLAUDE.md` is uncommitted and is the contract's only other copy.** Covered
above. Not a template defect; a state to resolve before anything resets this tree.

Of these, **#2 is the one that has appeared in more than one place** — it is a
property of the template, reproducible in any adopting repository, and it should
be fixed in `hymeko/hymeko` rather than in core.

---

## Open issues and follow-ups

1. Merge `CLAUDE.incoming.md` into `CLAUDE.md`. §4, §10 and §11 must survive the
   merge — they are in the existing file and were not copied into
   `docs/agent-contract/`.
2. Commit `CLAUDE.md` (724 lines). It exists in no commit in this repository.
3. Decide the `CORE.YAML` YAML-validity fix (Conflicts #1) — needs approval.
4. ~~Re-freeze `KAT_CORPUS_COUNT` / `KAT_CORPUS_DIGEST`~~ — **owned by the webxr
   work, not by this migration.** Two tests are red in the working tree until
   that lands; the committed tree is green.
5. ~~Raise Conflicts #2 against `hymeko/hymeko`~~ — **that is the other package,
   handled elsewhere.** Recorded here so core has the evidence; no action from
   this side.
6. ~~Decide `.gitignore` for `__pycache__`~~ — done in this branch.

---

## Status

```
MERGE PENDING   documents staged as CLAUDE.incoming.md, awaiting your merge
```

**Enforcement or only text?** This repository now has **both**, and they are
separable:

- **Enforcement is live.** `SYMBOLS.md` and `.duplication-baseline.json` are
  committed and current; `pre-commit` and `commit-msg` are installed in
  `.git/hooks` and were verified end to end — they blocked a real commit three
  times and released it only on a `DISCOVERY:` line. That works today, regardless
  of the merge.
- **CI is not.** `.github/workflows/duplication-gate.yml` is placed but inert; no
  runner was found. The gates run locally and on no server.
- **The documents are staged, not adopted.** `CLAUDE.incoming.md` sits beside the
  724-line `CLAUDE.md`, which is untouched. Until you merge them, the binding root
  document is still the old one — and §4, §10 and §11 exist only there.

`DONE` would be wrong: the documents layer is complete but not merged.

---

## Provenance

- Migration branch `chore/agent-contract`, commits `b7bd022`, `c00a038`.
- Base `7019940`; contract source `hymeko/hymeko@cd926c87` (clean tree).
- Working tree **dirty throughout** with unrelated in-progress webxr work
  (`CLAUDE.md`, `data/webxr/meta_spatial.hymeko`,
  `hymeko_core/src/webxr/mod.rs`, `data/webxr/maxwell_fiducial_demo.hymeko`,
  `hymeko_core/src/webxr/query.rs`, `hymeko_core/src/webxr/query/`). Every commit
  used an explicit pathspec; none of those six paths was staged or modified.
- Measurement worktrees: `wt-head` at `7019940`, `wt-post` at `c00a038`, both
  outside the repository.
- Seed for both preflight samples: `20260904`.
- No plan bundle under `docs/plans/`: this task added no code to either crate and
  changed no research-affecting behaviour. Contract documents and their tooling,
  installed under an explicit instruction, are not a §2 implementation change.
