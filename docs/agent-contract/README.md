# Core's shared-contract adoption

Core consumes `hymeko/hymeko` at provider revision
`d02aa614bc06a348dbcbe3936dcd3b9285b8bdeb` (2026-09-05). Shared instructions and
implementations stay in the provider; Core carries entry points, pins and its
project-specific rules.

Use sibling checkouts named `core` and `hymeko`. The active rules are
[`global.md`](../../../hymeko/docs/agent-contract/global.md) and its subject
modules. Resolve a missing sibling before source edits; do not fall back to the
historical copies here. Fetch relevant remotes and inspect local changes and
ahead/behind status before synchronizing either checkout.

Codex starts with root [`AGENTS.md`](../../AGENTS.md), requiring actual reads of
[`CLAUDE.md`](../../CLAUDE.md), shared global rules and applicable modules.
Claude Code imports `AGENTS.md` and the sibling global rules from `CLAUDE.md`.
A Markdown link alone does not load instructions.

The [provider reader guide](../../../hymeko/docs/agent-contract/README.md) gives
the activation procedure. In a fresh Claude Code session, accept the external
import/project trust and inspect `/memory`, `/status` and `/hooks`. Expected
imports here: `AGENTS.md` and `../hymeko/docs/agent-contract/global.md`. Project
settings supply the deterministic persona command and semantic Stop check.
Codex file validation cannot establish a separate Claude session's trust decision.
In a fresh Codex session, verify its loaded root `AGENTS.md` and subsequent
mandatory shared-file reads.

Portable gates resolve the provider revision in `tools/hooks/pre-commit` and
`tools/hooks/commit-msg`:

```sh
sh tools/hooks/install.sh
sh tools/hooks/install.sh --status
contract-symbols --root . --check
contract-duplication --root .
```

Compare installed hooks before replacement and reconcile foreign logic. Git
worktrees share the repository's hook directory. The installed-entry-point fast
path is shared across repositories too: another installation can change its tool
revision. Verify installation provenance on upgrades, or use the explicit pinned
`uv tool run --from` commands in the report. Hook file equality alone does not
verify the installed tool version.

Core does not run the provider-only `contract-self-check`; the hook retains its
provider-source guard. The workflow template uses the same pin, but a tracked
workflow does not prove an active Gitea runner.

`repo/` stays Core-owned: resource budgets, toolchain and HyMeKo authoring. Local
`global.md`, `modules/` and `incidents/` are historical snapshots of provider
`ff0c013`, retained for dated report citations. Do not edit or re-sync them as a
competing current contract. Section numbers keep their original meaning upstream.

See [the adoption report](../../reports/2026-09-05-contract-provider-refresh.md)
for verification and limits. `reports/2026-09-04-contract-split.md` records the
historical extraction.
