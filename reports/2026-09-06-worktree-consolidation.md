# Core branch consolidation, 2026-09-06

Updated: 2026-09-06T03:58:27+02:00

The requested nine Core branch histories are integrated in the existing canonical Core checkout, based on freshly fetched `4acba44009a0543c20ef910b2b35225a51aa280d`, on `main`. This report records the reviewed merge tree before its normal hooked commit. Final publication and ancestry are checked after that commit. Graph integration uses its existing checkout; no new branch, worktree or clone was created for this consolidation.

## Branch disposition

| Branch | Incoming tip | Result in Core |
|---|---|---|
| `feat/core-generational-arenas` | `093eab3268` | Arena study, prototype, contracts and decision imported. Production arena migration remains deferred. |
| `feat/core-incidence-operator-precompute` | `52c2649237` | Coefficient plan and checks imported through its backbone descendant. |
| `feat/incidence-operator-plan-adoption` | `19c57df4eb` | Existing Bx profile and adoption decision imported; fixture/timing helpers reuse incidence_support. |
| `feat/core-canonical-incidence-backbone` | `64c79b2ad7` | IncidenceIndexData, borrowed IncidenceView, PreparedIr, StarOperator, compatible IncidenceIndex entry points and tests imported. |
| `feat/humanoid-actuation` | `9c2f87eaa8` | Core-owned kinematics, humanoid and quadruped model data, plus portable ID serde and tests imported. |
| `fix/semantic-model-digest-v2` | `112a254326` | Main already contains the corrected algorithm 2. Its implementation and 29 controls stay byte-for-byte unchanged; useful other ancestors are integrated. |
| `feat/hymeko-python-pow0` | `37f5f04cfe` | Historical architecture, plan and report imported. Its implementation is superseded by the existing separate Python owner; no duplicate Python crate/member is added to Core. |
| `chore/workspace-registry` | `7d235c4efe` | Old registry/doctor retained as explicitly historical evidence under reports/worktree-consolidation. Provider ecosystem.hymeko remains the active authority. |
| `master` | `2f9361636c` | Unrelated initial flat import connected as historical ancestry; current workspace layout retained. 105 of 121 original paths match the 71c1f99 workspace migration byte-for-byte. |

The merge was deliberately prepared without automatic source replacement, then populated with reviewed branch deltas before committing. It is not an empty ancestry-only merge: incidence, portable IDs, fixtures, tests and evidence are present in the final tree. The already superseded Python, registry and flat-root layouts are not revived. All nine tips must be ancestors of the final main; existing refs are retained.

## Verification

- `cargo test -p hymeko_core --release --locked --offline --no-fail-fast -j 4 -- --test-threads=4`: **417 passed, 0 failed, 7 ignored**, including doctests. The 29 semantic digest controls pass.
- `cargo clippy -p hymeko_core --all-features --all-targets --release --locked --offline -j 4 -- -D warnings`: **PASS** (13.29 s). Optional features are compiled by this gate; their complete runtime matrix was not run.
- `cargo fmt --all --check`: **PASS**.
- Symbols regenerated with the installed provider tool: **5 packages, 635 public symbols**. Final normal commit runs symbols, duplication and preflight hooks.
- Core Cargo.toml, Cargo.lock and the corrected semantic digest implementation/tests have no diff against the fetched base.
- Native portable ID tests pass. Historical 32-bit/WASM evidence is retained in the original report; no new WASM execution is claimed here.
- Initial Graph integration against its prior Core pin: **556 passed, 0 failed, 21 ignored** across 64 result groups; all-feature/all-target Clippy and formatting pass. The final Core consumer run is recorded in Graph's existing independent-audit report after updating its pin.

### Corpus fixture migration

The imported robotics files change the whole-fixture corpus hash, not the v1 hash algorithm. The first test run therefore correctly failed the old aggregate golden. `reports/2026-09-05-dependency-refresh.md` already contains the controlled same-binary, same-Parser, fixture-only comparison proving the exact two values. The count remains **102**:

- previous fixture set: `f9ab7da8b5d823fa9d55c28e78fd63bc37660a97f67852ad587a145f951cb3b5`;
- imported `9c2f87e` fixture set: `8938b37b221ba015f1a52f318f8b9d9ac9aab65777ee07026916bf2b1d13dd12`.

The corpus test now names this fixture revision; individual canonical hash vectors are unchanged. File provenance and digests are in `worktree-consolidation/robot-fixture-migration.json`. The complete rerun passes.

### Duplication baseline adoption

The existing 60-token detector budget was **403 clones / 7800 cloned lines / 182 files**. Importing the branches initially measured **426 / 8209 / 197**. Reusing the existing incidence_support fixture/timing helpers removed **4 clone groups and 46 cloned lines**, leaving **422 / 8163 / 197**. The baseline is deliberately advanced to those measured values for this user-authorized integration: **+19 groups / +363 lines** over the prior budget. This is a recorded integration cost, not a claimed code-health improvement. The imported independent tests and historical arena prototype remain inspectable; no exclusions, detector changes or disabled hooks hide them.

## Graph integration

Graph combines `4a59c238` with `83774b8b` in its existing `audit/platform-2026-09-05` checkout. Conflict resolution retains current directed-slot/consistent-A* dominance guards, imports the matroid owner and its independent checks, preserves goal pruning, and identifies cycles by alternating node/arc identity so parallel arcs remain distinct. Correct goal pruning lowers the example path expansion requirement from six to four; the existing budget matrix now distinguishes path and cycle exhaustion. Its complete tests and Clippy pass. Historical timing/TSV/SVG evidence stays explicitly dated; this task makes no fresh performance claim.

## Scope and recovery

Canonical source location: `D:/hakiko_ai_ws/03_implementation/hymeko_framework/core`. Graph is its sibling. The user has explicitly restricted further action to these two owners. No subsequent Simulator, Chordex or other-repository changes or tests belong to this phase.

Earlier in the same task, before the user's stop at 03:33, out-of-scope Simulator and Chordex edits occurred. Those edits are not claimed validated or complete. No subsequent read, edit, rollback or cleanup of those owners was performed in this phase. Simulator preimages remain in the existing local recovery archive; retaining this record does not authorize further action there.

The previous cleanup archive was moved unchanged into `.git/worktree-cleanup-20260906`. Historical paths inside it describe original locations. It contains original refs, exact preimages, branch disposition, migration comparisons and full UTF-16 command logs. The follow-up archive preserves 3710 non-generated files with verified hashes. No additional cleanup deletion or disk-space saving is claimed for this branch-integration phase.

Rollback of the Core merge, if subsequently requested: normal `git revert -m 1 <merge>` after checking current user work; original main base and all incoming refs remain available. Graph has its own pre-merge HEAD and normal merge rollback. Do not reset other workers' changes.

## Discovery

The existing consolidation plan/report, historical branch reports, SYMBOLS.md and actual source ownership were inspected before integration. Existing incidence_support owns the profile fixtures and timing helper. The corrected digest already owns semantic hashing. Python bindings and the provider registry already have separate canonical owners, so their obsolete Core layouts were rejected. Exact nine-branch dispositions are also recorded in `worktree-consolidation/core-branch-disposition.json`.

Plan: [existing consolidation plan](../docs/plans/2026-09-06-worktree-consolidation/plan.md), with its TeX/PDF/TikZ/Mermaid bundle.
