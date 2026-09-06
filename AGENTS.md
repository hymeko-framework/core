# HyMeKo Core agent entry point

This repository consumes the shared contract from the sibling `hymeko/hymeko`
checkout at `../hymeko`. Core owns its semantic IR and compute vocabulary; consult
`docs/hymeko-platform-strategy.md` and `docs/hymeko-kernel-ownership.md` when choosing
a capability owner.

Before acting, read `CLAUDE.md`, `CORE.YAML`, `tools.yaml`, `SYMBOLS.md`, and
`../hymeko/docs/agent-contract/global.md` in full. Read applicable subject modules
under `../hymeko/docs/agent-contract/modules/` before acting on their subject.
These are mandatory instructions, not optional reference links. Codex must use
its file-reading tool; it does not expand Claude's `@path` imports.

Before editing files, creating/switching a branch, or creating a worktree,
successfully run `git fetch` for relevant remotes in every affected repository
and compare the freshly fetched base under the shared `global.md`. Reuse existing
suitable checkouts across agent handoffs and preserve user changes. If fetch
fails, continue read-only work unless the user authorizes offline work.

For cross-repository work, also read
`../hymeko/docs/agent-contract/repo/repository-map.md` and query the provider's
`ecosystem.hymeko` map. Core-specific resource, toolchain and authoring rules stay
under `docs/agent-contract/repo/`. The local copies of shared modules are historical
snapshots, not a second current authority.

The adopted provider revision and setup procedure are recorded in
`docs/agent-contract/README.md`. If the sibling contract path is absent, repair the
loading chain before source edits. Core's portable verification is symbols,
duplication and preflight through `tools/hooks/`; `contract-self-check` is the
provider's aggregate suite and is not a Core requirement. Claude-specific runtime
commands and Stop hooks apply only in Claude Code.
