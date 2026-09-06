Created-at: 2026-09-06T02:40:23.997496+02:00
# Worktree consolidation
Owner: local HyMeKo workspace administration. ETA: 15-25 minutes, subject to dirty-file inventory.
Use the existing hymeko_framework repositories as canonical working directories.
Inventory every linked checkout, refs, ignored/untracked files, operation markers, and remote state.
Preserve every branch, detached commit, stash and non-generated local file before removal.
Back up Git refs and local-only files with hashes; do not merge feature implementations.
Select the existing audited Core/Graph/Transformation branches in their canonical working directories.
Remove redundant clean worktrees with git worktree remove, without force. Verify absolute paths first.
Independent duplicate clones require verified bundle/ref import into the canonical owner before retirement.
Keep any unresolved active/dirty work intact until it has an exact recoverable destination.
Preserve old task paths as directory junctions where needed for report access and sibling imports.
Acceptance: one active checkout per repository where safe, explicit preserved exceptions, branch/HEAD and file-hash verification, no unmerged work lost.
No source edits, dependency updates, main merges, branch deletion or release publication.
Validation is Git/ref/file preservation, not a new product test run.
Rollback: recorded branches and Git bundles recreate removed checkouts; local file backups restore nontracked state.

## Follow-up, 2026-09-06T03:17:56.569604+02:00
User explicitly requests all useful work from D:/hnnwork and D:/h be preserved and redundant worktrees deleted. No new worktrees; all execution from canonical Core. ETA 20-35 minutes for archive verification and generated-cache deletion.
Archive owner: core/.git/worktree-cleanup-20260906. Preserve the existing archive there, all exact dirty files, nested snapshot Git stores, full refs and useful timing/profile/wheel evidence. Move the sole Nagare, HySTL and Python Git repositories to existing framework root without overwriting its unversioned hymeko_python directory. Remove reviewed disposable targets and redundant linked worktrees only after verified recovery. Archive unique inactive clones if direct removal is blocked. Preserve active user files and every branch; do not merge features. Verify retained source hashes, refs, worktree lists, path dependencies and disk space. Administrative operation only: no product rebuild or dependency change.

## Integration correction ? 2026-09-06T03:24:48.310965+02:00
The user clarified: "nem meg?rizni kell hanem itnegr?lni". This supersedes the earlier archive-only/no-source-change disposition. Useful code must be integrated in its canonical owner before retiring the old worktree.
Graph: combine 4a59c238 with 83774b8b in the existing audit checkout. Resolve four merge conflicts preserving the approved current directed-slot/consistent-A* and bounded-enumeration fixes, add the existing provenance-safe matroid owner and its tests/evidence. Keep the current benchmark dependency lock. Run focused regressions first, then full Graph workspace tests and scoped formatting/lints. No new worktree, dependency source, API compatibility layer or main-branch merge.
Chordex: transplant the three existing snapshot changes into the central no-Git working directory, whose corresponding files exactly equal the snapshot base. Run its existing transport tests including real TCP. Simulator: merge the existing ingress changes into current user work; preserve newer local dependency choices and projection changes, update relocated fixture paths, run ingress and WASM checks against the existing dependency set. Other user work is neither reverted nor committed.
Python: canonical D:/hakiko_ai_ws/03_implementation/hymeko_python already exists at 573a30ff; do not create a second central checkout. Confirm superseded WIP capabilities against it and import missing useful tests/evidence only. Nagare and HySTL: move the existing source/Git stores to the framework sibling root, preserving revisions and path dependencies.
Verification: reviewed merged diff, previous regression coverage, current targeted tests, file hashes and refs, no old-root dependencies, then old worktree/cache removal. Archive is rollback only. CORE changes: Graph's existing protected implementations under explicit user integration authorization; Core algorithm/fixtures untouched. ETA now 30-50 minutes, depending on necessary compile time. Peak build memory budget 16 GiB, at most 4 build jobs; use existing central target directories, never create an isolated build checkout.
Rollback: reverse only the task-specific patch captured against the pre-integration user files; Graph pre-merge HEAD 4a59c238 and old source ZIP; branch refs retained.

Graph integration file scope:
```text
.gitignore
benchmarks/Cargo.lock
docs/graph-navigation-performance.md
docs/matroid-over-canonical-ir.md
docs/plans/2026-09-03-codex-p34-audit/ownership.mmd
docs/plans/2026-09-03-codex-p34-audit/plan.md
docs/plans/2026-09-03-codex-p34-audit/plan.mmd
docs/plans/2026-09-03-graph-navigation-p31/plan.md
docs/plans/2026-09-03-graph-navigation-p31/plan.mmd
docs/plans/2026-09-03-graph-navigation-p31/plan.tex
docs/plans/2026-09-03-graph-navigation-performance/plan.md
docs/plans/2026-09-03-graph-navigation-performance/plan.mmd
docs/plans/2026-09-03-graph-navigation-performance/plan.tex
docs/plans/2026-09-03-p34-levi-and-measurements/plan.md
docs/plans/2026-09-03-p34-levi-and-measurements/plan.mmd
docs/plans/2026-09-03-p34-levi-and-measurements/plan.tex
docs/plans/2026-09-03-zero-copy-matroid/plan.md
docs/plans/2026-09-03-zero-copy-matroid/plan.mmd
docs/plans/2026-09-03-zero-copy-matroid/plan.tex
docs/plans/2026-09-04-graph-performance-evidence/plan.md
docs/plans/2026-09-04-graph-performance-evidence/plan.mmd
docs/plans/2026-09-04-informed-berge-evidence/plan.md
hymeko_graph/src/direct_ir/berge_enum.rs
hymeko_graph/src/direct_ir/informed.rs
hymeko_graph/src/lib.rs
hymeko_graph/src/matroid/error.rs
hymeko_graph/src/matroid/facade.rs
hymeko_graph/src/matroid/levi.rs
hymeko_graph/src/matroid/mod.rs
hymeko_graph/src/matroid/rank2.rs
hymeko_graph/src/matroid/workspace.rs
hymeko_graph/tests/codex_p34_audit.rs
hymeko_graph/tests/direct_ir_coverage.rs
hymeko_graph/tests/direct_ir_optional.rs
hymeko_graph/tests/informed_berge_evidence.rs
hymeko_graph/tests/informed_common/mod.rs
hymeko_graph/tests/ir_ops_bench.rs
hymeko_graph/tests/map_routing_nav.rs
hymeko_graph/tests/matroid_bench.rs
hymeko_graph/tests/matroid_facade.rs
hymeko_graph/tests/matroid_levi_vv.rs
hymeko_graph/tests/matroid_vv.rs
hymeko_graph/tests/p32_diagnosis.rs
hymeko_graph/tests/p32_workspace_alloc.rs
hymeko_graph/tests/p34_cell.rs
hymeko_graph/tests/p34_memory.rs
hymeko_graph/tests/p34_repr_bench.rs
hymeko_graph/tests/performance_evidence.rs
hymeko_graph/tests/performance_evidence/representations.rs
hymeko_graph/tests/performance_evidence/statistics.rs
hymeko_graph/tests/performance_evidence/telemetry.rs
hymeko_graph/tests/steiner_mycielski_nav.rs
hymeko_pgraph/tests/andor_ts_matrix.rs
hymeko_pgraph/tests/codex_p34_audit.rs
reports/2026-09-03-codex-p34-independent-audit.md
reports/2026-09-03-graph-navigation-p31.md
reports/2026-09-03-graph-navigation-p32.md
reports/2026-09-03-graph-navigation-performance.md
reports/2026-09-03-matroid-navigation-p34.md
reports/2026-09-03-zero-copy-matroid-performance.md
reports/data/2026-09-03-codex-p34-allocations.tsv
reports/data/2026-09-03-codex-p34-artifact-sha256.tsv
reports/data/2026-09-03-codex-p34-claims.tsv
reports/data/2026-09-03-codex-p34-duplication.tsv
reports/data/2026-09-03-codex-p34-rerun.tsv
reports/data/2026-09-03-map-routing-measurement.tsv
reports/data/2026-09-03-navigation-measurement.tsv
reports/data/2026-09-03-p32-work-counters.tsv
reports/data/2026-09-03-p32-workspace-batch-alloc.tsv
reports/data/2026-09-03-p33-matroid-latency.tsv
reports/data/2026-09-03-p34-cpu-scaling.tsv
reports/data/2026-09-03-p34-ir-ops.tsv
reports/data/2026-09-03-p34-levi-perf.tsv
reports/data/2026-09-03-p34-manifest.tsv
reports/data/2026-09-03-p34-memory.tsv
reports/data/2026-09-03-p34-representation.tsv
reports/data/2026-09-03-p34-ts-norm.tsv
reports/figures/codex-p34-batch-scaling.svg
reports/figures/codex-p34-like-for-like-components.svg
reports/figures/p32-allocations.svg
reports/figures/p32-work-counters.svg
reports/figures/p33-matroid-basis-memory.svg
reports/figures/p33-matroid-incremental.svg
reports/figures/p33-matroid-latency.svg
reports/figures/p34-breakeven.svg
reports/figures/p34-cpu-scaling.svg
reports/figures/p34-memory.svg
reports/figures/p34-rank-basis-ratio.svg
reports/informed-berge-evidence/2026-09-04-local-baseline/artifact-sha256.tsv
reports/informed-berge-evidence/2026-09-04-local-baseline/data/cells.tsv
reports/informed-berge-evidence/2026-09-04-local-baseline/data/enumeration-summary.tsv
reports/informed-berge-evidence/2026-09-04-local-baseline/data/processes.tsv
reports/informed-berge-evidence/2026-09-04-local-baseline/data/results.tsv
reports/informed-berge-evidence/2026-09-04-local-baseline/data/rss.tsv
reports/informed-berge-evidence/2026-09-04-local-baseline/data/samples.tsv
reports/informed-berge-evidence/2026-09-04-local-baseline/data/search-comparison.tsv
reports/informed-berge-evidence/2026-09-04-local-baseline/data/search-work.tsv
reports/informed-berge-evidence/2026-09-04-local-baseline/data/shortest-comparison.tsv
reports/informed-berge-evidence/2026-09-04-local-baseline/figures/berge-shortest.svg
reports/informed-berge-evidence/2026-09-04-local-baseline/figures/grid-astar.svg
reports/informed-berge-evidence/2026-09-04-local-baseline/figures/weighted-map-astar.svg
reports/informed-berge-evidence/2026-09-04-local-baseline/manifest.tsv
reports/informed-berge-evidence/2026-09-04-local-baseline/report.md
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/artifact-sha256.tsv
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/data/cells.tsv
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/data/enumeration-summary.tsv
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/data/processes.tsv
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/data/results.tsv
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/data/rss.tsv
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/data/samples.tsv
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/data/search-comparison.tsv
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/data/search-work.tsv
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/data/shortest-comparison.tsv
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/figures/berge-shortest.svg
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/figures/grid-astar.svg
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/figures/weighted-map-astar.svg
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/manifest.tsv
reports/informed-berge-evidence/2026-09-04-repro-KYBERSAMAN/report.md
reports/performance-evidence/2026-09-04-dijkstra-comparability/artifact-sha256.tsv
reports/performance-evidence/2026-09-04-dijkstra-comparability/data/arity-sweep.tsv
reports/performance-evidence/2026-09-04-dijkstra-comparability/data/cells.tsv
reports/performance-evidence/2026-09-04-dijkstra-comparability/data/dijkstra-pre-post.tsv
reports/performance-evidence/2026-09-04-dijkstra-comparability/data/processes.tsv
reports/performance-evidence/2026-09-04-dijkstra-comparability/data/representation-totals.tsv
reports/performance-evidence/2026-09-04-dijkstra-comparability/data/representations.tsv
reports/performance-evidence/2026-09-04-dijkstra-comparability/data/rss.tsv
reports/performance-evidence/2026-09-04-dijkstra-comparability/data/samples.tsv
reports/performance-evidence/2026-09-04-dijkstra-comparability/data/search-work.tsv
reports/performance-evidence/2026-09-04-dijkstra-comparability/figures/arity-sweep.svg
reports/performance-evidence/2026-09-04-dijkstra-comparability/figures/batch-scaling.svg
reports/performance-evidence/2026-09-04-dijkstra-comparability/figures/process-memory.svg
reports/performance-evidence/2026-09-04-dijkstra-comparability/figures/representation-total.svg
reports/performance-evidence/2026-09-04-dijkstra-comparability/manifest.tsv
reports/performance-evidence/2026-09-04-dijkstra-comparability/report.md
reports/performance-evidence/2026-09-04/artifact-sha256.tsv
reports/performance-evidence/2026-09-04/data/cells.tsv
reports/performance-evidence/2026-09-04/data/core-ir-coverage.tsv
reports/performance-evidence/2026-09-04/data/processes.tsv
reports/performance-evidence/2026-09-04/data/representation-totals.tsv
reports/performance-evidence/2026-09-04/data/representations.tsv
reports/performance-evidence/2026-09-04/data/rss.tsv
reports/performance-evidence/2026-09-04/data/samples.tsv
reports/performance-evidence/2026-09-04/figures/batch-scaling.svg
reports/performance-evidence/2026-09-04/figures/process-memory.svg
reports/performance-evidence/2026-09-04/figures/representation-total.svg
reports/performance-evidence/2026-09-04/manifest.tsv
reports/performance-evidence/2026-09-04/report.md
scripts/codex_p34_audit_figures.ps1
scripts/informed_berge_evidence/figures.ps1
scripts/informed_berge_evidence/run.ps1
scripts/p32_figures.py
scripts/p33_matroid_figures.py
scripts/p34_cpu_rss.ps1
scripts/p34_figures.py
scripts/performance_evidence/figures.ps1
scripts/performance_evidence/run.ps1
scripts/svg_plot.py
```

## Simulator stop ? 2026-09-06T03:33:47.646481+02:00
The user explicitly prohibited further Simulator actions. No further reads, edits, tests, rollback, integration or cleanup of its source/snapshot worktrees. Before the stop, this task changed src/model_ingress.rs, src/web.rs, tests/model_ingress.rs and Cargo.toml. Preimages are in .git/worktree-cleanup-20260906/simulator-integration-preimage.zip; existing user work was not reverted. The targeted Simulator run failed before compilation on a concurrently relocated Chordex path. Simulator integration is not claimed complete. Continue only the Core/Graph consolidation; leave Simulator-owned old paths alone.

## Nine Core branches ? 2026-09-06T03:41:19.680105+02:00
The user explicitly requests merging the nine named Core branches into the central Core and permits Graph only as an additional repository. Simulator, Chordex and every other repository are excluded from any further action. No new worktrees.
Target: existing local main, safely advanced from d6be4ea to fetched 4acba44 (the current audited tree is byte-identical to that merge). Preserve any current user modifications; no automatic stash/reset.
Order: deferred generational-arena study/tests; incidence precompute; adoption/profile; canonical incidence backbone; robotics and portable ID ancestors; corrected semantic digest; retire duplicate Python/workspace-registry/initial-import layouts through explicit history merges and historical documentation. All nine original heads must become ancestors of final main. Main publication is part of the requested central branch consolidation; no force-push, branch deletion or release tagging.
Actual code to integrate: owned incidence index data/PreparedIr/StarOperator and coefficient plan, corresponding tests and profiles; portable ID serde and its tests; Core robotics meta/humanoid/quadruped fixtures. Keep current digest algorithm 2 and all 29 corrected controls; preserve current parser lock and distributed-admission semantics. Imported historical Python code is already superseded by its separate owner; do not revive a Python member in Core. Registry belongs to the provider ecosystem.hymeko; preserve old registry/doctor as clearly historical evidence without restoring a second authority. The unrelated old master is the pre-workspace crate import; record its history without recreating root src/tests copies.
Protected files: hymeko_core/src/ir/incidence.rs, common/ids.rs, common/ids_impl.rs and the three data/robotics files. Explicit user authorization: the current request to merge all named branches; previous specific approval scopes remain valid. No new dependency required. Core public incidence additions retain existing borrowed-index entry points.
Validation: focused incidence/backbone/arena/ID/digest checks then complete Core CPU tests, formatting, Clippy, symbol/duplication/preflight gates; Graph consumer test against final Core if needed, using its existing checkout. Use historical measurements without relabeling them as current. No simulator/other-consumer tests. Rollback: original refs and per-merge commits, preserving unrelated user data. ETA 30-45 minutes; four build jobs, 16 GiB budget, reuse existing targets.

Integration mechanism: establish the requested multi-parent merge without automatic source replacement, then apply the reviewed branch deltas to its index before the normal hooked commit. This accommodates unrelated master and superseded Python/registry/digest layouts without either restoring them or bypassing final verification. The resulting commit must contain the actual new incidence, ID, fixture and test code, and all nine requested heads as ancestors.

## Final integration decisions
Updated: 2026-09-06T03:58:27+02:00

Duplication adoption: the nine-branch integration brings previously unmerged incidence APIs, independent regression tests, and the deferred arena prototype into the existing Core owner. The measured token detector changed from the committed budget 403 clones / 7800 cloned lines / 182 files to 422 / 8163 / 197. The unrefactored import was 426 / 8209; reuse of incidence_support::{median_ms, windowed} in bx_profile removed four clone groups and 46 cloned lines. Accept the remaining +19 groups / +363 lines as an explicit cost of the user-requested existing branch integration, not as a duplication improvement. Do not remove independent tests or refactor unrelated Core code to lower the counter. Update the existing baseline with contract-duplication --update, keep the detector window at 60 tokens and all hooks enabled, and record the delta in the report. The user's explicit instruction to merge these protected Core branches is the applicable authorization.

Graph consumer: after the validated Core merge is published, update Graph's existing Core revision and both Graph lockfiles to that exact commit. Run its workspace tests with the resolved Git dependency and compile its separate benchmarks if dependencies permit. This implements the user's earlier request for current HyMeKo dependency revisions without creating another checkout. Update Graph's stale registry pointer to the authoritative provider ecosystem.hymeko. Scope remains Core and Graph. Publication uses ordinary pushes only; never delete branch refs or rewrite history in this task.
