Created-at: 2026-09-15 23:02 +0200
ETA: 15-25 minutes for the Core adapter/tests after approval; consumer integration
60-90 minutes including HIVE catalog, Quest UI, Docker build and APK.

Scope: transfer a Core-validated DecodedCborPayload into CompiledModel without
source generation, parsing, lowering, new validation logic or arbitrary IR/interner pairing.
API: CompiledModel::from_decoded_cbor(snapshot: DecodedCborPayload) -> Self.
It consumes snapshot.into_parts() and moves ir/index/imports/hash/root_path and
the paired interner into the existing CompiledProgram/CompiledModel. Root stays
workspace-relative ModuleKey; it does not grant filesystem access. No allocation
of new declarations or ID remapping. No changes to CBOR or ir_stream schemas.
Only the existing decoder can construct the accepted validated input type.
ExecutionState::new/prepare_model_at/commit retain their current authority,
base checking and publication semantics. Legacy derived caches remain governed
by the existing decoder and value-update invalidation; no consumer trusts them.

Files: hymeko_core/src/module_store/execution.rs (adapter and focused tests),
execution_tests.rs (reuse its existing CallbackParser in the focused test),
SYMBOLS.md, this plan bundle and task report. No dependency or fixture changes.
CORE.YAML protected source touched: execution.rs. Approval requested:
APPROVED-CORE-EDIT: ir-snapshot-install
Status: APPROVED-CORE-EDIT: ir-snapshot-install received in user chat, 2026-09-15T23:37:18+02:00.
Earlier ir-value-updates approval covers existing leaf changes, not this constructor.

Discovery: read execution.rs/value_updates.rs/cbor_writer.rs and public payload/
CompiledProgram definitions; searched sources and SYMBOLS for from_decoded_cbor,
from_payload/from_snapshot/install. Decoder and publication exist, constructor does not.
Extend the existing CompiledModel; no standalone loader or parallel state model.

Tests: compile once with counted parser, encode/decode/install, check complete IR,
matching interner/IDs/imports/hash and semantic digest, then mutate a numeric leaf
and publish through existing ExecutionState. Retained old snapshot stays unchanged;
stale prepared candidate is rejected. No parser calls occur during installation,
publication or value edits. Reject malformed bytes through the existing decoder.
Run focused Core tests, then HIVE roundtrip upload/mutation/reconnect tests.
Run formatting, symbols and duplication gates; no benchmark campaign.

Budget: move payload fields; no additional full-IR copy during install. HIVE owns
network size limits, model count, aggregate resident checkpoint limits and admission.
Consumer tests enforce those independently. Bound builds below 16 GiB and 4 jobs.
Worst case: maximum admitted fragmented model, rejected/truncated upload and stale
replacement must not publish. Source must never be used to repair a bad payload.
Rollback: revert only this task's adapter/tests after reviewing preserved dirty work.
No commit/push/branch switch/new worktree. Existing Core checkout is dirty at
368c7fd and contains c6c59d9 snapshot sources plus approved value updates.

Progress 2026-09-16 00:11 +0200: approved adapter implemented; 14 Core execution
tests and canonical stream integration test pass, Core Clippy passes. Symbols
regenerated and checked. Duplication gate passes: 482 clones / 10393 lines,
unchanged relative to the pre-task source; stored baseline 484 / 10411 unchanged.
Existing CallbackParser test helper is reused via test-only parent visibility.
