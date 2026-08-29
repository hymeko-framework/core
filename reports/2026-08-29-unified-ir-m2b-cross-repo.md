# Unified IR M2b — cross-repository status

**Date:** 2026-08-29
**Authorisation:** `APPROVED-CORE-EDIT: unified-ir-m2b-sibling-consumers`
**Result:** **Q1–Q3 blocked, no sibling repository modified.** The cause is verified, not inferred.

---

## 0. The blocker

Every sibling consumer depends on `hymeko_core` as a **git dependency pinned in its own `Cargo.lock`**, and every pin predates the accessor API.

| Repository | Core pin | Pin date | Commits behind core HEAD | `ir/access.rs` at that pin |
|---|---|---|---:|---|
| `network` | `3bdebf89` | 2026-08-22 | 22 | **absent** |
| `graph` | `3bdebf89` | 2026-08-22 | 22 | **absent** |
| `transformation` | `71c1f997` | 2026-08-21 | 24 | **absent** |

`hymeko_core/src/ir/access.rs` first appeared at **`e660b49`** (M2a, 2026-08-28). No pin includes it, so `Ir::node`, `Ir::edge`, `Ir::arc`, `Ir::decls`, `Ir::*_count`, `Ir::contains_*`, `Ir::*_of` and `Ir::*_for_decl` **do not exist** in the core these repositories actually compile against.

Each `Cargo.toml` declares `branch = "main"`, so the *intent* is to track core — but the lockfile is what determines the build, and it is 22–24 commits stale.

## 1. Why this was not worked around

Migrating any sibling consumer would first require bumping its `Cargo.lock` to a core commit containing the accessors. That is not a read-only consumer migration:

1. **It is a dependency change.** The governing brief lists dependency and `Cargo.lock` modification as not authorised, and the sibling token covers only "read-only IR access migrated to storage-independent accessors".
2. **It is a 22–24 commit jump**, spanning the WGPU work, the IR-stream wire codec and assembler, the canonical-hash traversal rewrite (M0.6) and the accessor layer (M2a/M2a.1). Each carries its own verification surface in a repository whose gates I have not established a baseline for.
3. **It cannot be validated as this phase's work.** An equivalence test proving "the migrated consumer behaves as before" would be meaningless while the entire core underneath it also changed.

The overnight rules are explicit: *"ne találj ki scope-bővítő workaroundot"*. A lockfile bump to unblock a migration is exactly that.

## 2. Preflight results, recorded

All three repositories are otherwise **safe to work in** — the blocker is solely the pin.

| Repository | Branch | HEAD | Worktree | Governance files |
|---|---|---|---|---|
| `network` | main | `8d88ffe` | clean | none (no `CLAUDE.md`, `CORE.YAML`, `tools.yaml`) |
| `graph` | main | `ac341e0` | clean | none |
| `transformation` | main | `21b5826` | clean | none |

**Nothing in any sibling repository was modified, staged, stashed or reverted.**

## 3. Migration table

| Repository | Direct sites before | Migrated | Intentionally retained | Unexplained remaining | Tests | Touched-file coverage | Commit | Remote |
|---|---:|---:|---:|---:|---|---|---|---|
| `core` (`hymeko_core`) | 9 consumer | **9** | 35 (categorised) | **0** | 502 pass / 1 known drift | `pretty_print` 0→89.9 %, `ir_writer` 0→97.5 % | `f67135e` | `4d2ed32` |
| `core` (`hymeko_hnn`) | 7 | **7** | 0 | **0** | included above | `hypergraphview` 90.0→94.1 % | `4d2ed32` | `4d2ed32` |
| `network` | 4 (re-measured) | 0 | — | — | not run | not measured | — | — |
| `graph` | not enumerated | 0 | — | — | not run | not measured | — | — |
| `transformation` | not enumerated | 0 | — | — | not run | not measured | — | — |

Sites in the three blocked repositories were **not classified**, because classification without the ability to migrate or test would be an estimate presented as an inventory. The network count of 4 was re-measured (matching the earlier figure) before the blocker was found.

## 4. The Q4 check that did fire

One cross-repository verification completed and returned a result worth recording:

> **No sibling repository builds against the local `core` checkout.** All three use a git dependency on `ssh://git@amaterasu.local:2222/hymeko/core.git`, resolved through their own lockfiles. Publishing to core `main` therefore has **no effect** on any sibling build until someone deliberately updates that repository's lock.

This is the answer to "check that no consumer accidentally uses a different core checkout" — none of them uses the local one at all, by design. It also means the three commits pushed today cannot have broken any sibling: their builds are unchanged because their inputs are unchanged.

## 5. What unblocking would require

Not done here; recorded so the decision is available.

1. A separate, narrow authorisation for a `Cargo.lock` bump per repository.
2. For each: establish its own gate baseline **at the current pin** first, so the dependency jump and the consumer migration are separable.
3. Bump the lock, re-run its gates, and record any behaviour change attributable to the 22–24 core commits — independently of any accessor migration.
4. Only then migrate its read-only consumers, with the before/after equivalence tests this phase used in `core`.

Steps 2 and 3 are the part that cannot be skipped. Merging them into step 4 is what would make the result unverifiable.

## 6. Next

**`network` remains the next M2b target**, at 4 re-measured sites — the smallest and lowest-risk of the three. Its entry condition is now explicit: a lockfile bump authorised and verified as its own change, before any consumer migration.
