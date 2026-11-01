# Core's shared-contract adoption

Core's portable tooling is pinned to provider revision
`e6d83977978f198f509d448ac8121deed3af0aee` (2026-11-01 adoption).
Shared instructions and implementations stay in the provider; Core carries
entry points, pinned wrappers and its project-specific rules. The pin adopts
specification-derived checks, explicit measurement opt-in, conditional reports,
checkout convergence and fail-closed tooling checks.

In this workspace Core is `hymeko_framework/core` and the provider is
`03_implementation/hymeko`; the root entry points resolve it as `../../hymeko`.
For another layout, adjust those entry points to the existing provider checkout;
do not create a copy. The active rules are
[`global.md`](../../../../hymeko/docs/agent-contract/global.md) and its subject
modules. Resolve a missing sibling before source edits; do not fall back to the
historical copies here. Fetch relevant remotes and inspect local changes and
ahead/behind status before synchronizing either checkout.

Codex starts with root [`AGENTS.md`](../../AGENTS.md), requiring actual reads of
[`CLAUDE.md`](../../CLAUDE.md), shared global rules and applicable modules.
Claude Code imports `AGENTS.md` and the sibling global rules from `CLAUDE.md`.
A Markdown link alone does not load instructions.

The [provider reader guide](../../../../hymeko/docs/agent-contract/README.md) gives
the activation procedure. In a fresh Claude Code session, accept the external
import/project trust and inspect `/memory`, `/status` and `/hooks`. Expected
imports here: `AGENTS.md` and `../../hymeko/docs/agent-contract/global.md`. Project
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
worktrees share the repository's hook directory. The wrappers resolve their explicit provider revision with `uv tool run`;
a globally installed command from another repository does not override that pin.
Verify installation provenance and compare hook bytes after an upgrade.

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

Instruction wiring refreshed on 2026-11-01 against provider main
`e6d83977978f198f509d448ac8121deed3af0aee`. Shared instructions load from
the fetched provider checkout. File validation and hook installation do not
prove another Claude session has accepted external imports.

The 2026-11-01 detector migration uses the corrected Rust whole-literal erasure
introduced by provider `8823c90`. On the same 201 Core source files, the previous
detector counted 460 clones / 8736 cloned lines; the new detector counts 553 /
12921. The baseline records that tool-semantics change and its reason. This is
not new source duplication or a code-quality improvement. Runtime/test/benchmark
sources remain unchanged from Core main `7664d55` for this adoption.
The installed local Git hooks were compared before replacement and verified
byte-for-byte afterward; no global tool environment was replaced.
