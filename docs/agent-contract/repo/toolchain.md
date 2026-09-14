---
source: CLAUDE.md §10 Pinned Toolchain, lines 600-664, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: verbatim extraction — not yet de-projectified
---

**Contains:** §10 test runners · benchmarking · profiling · memory · coverage · property testing · versions

## 10. Pinned Toolchain

Tool variability between runs invalidates comparison. This section fixes **one canonical tool per concern per language**. Use the listed tool. Substitution requires a Section 1 (CORE.YAML-level) approval.

### Test runners

- **Rust:** `cargo test` (release-mode for performance tests).
- **Python:** `pytest -p no:randomly` (deterministic order; opt into shuffle explicitly per test).
- **C/C++:** `gtest`.

### Benchmarking

- **Rust:** `criterion` — statistically rigorous, regression-aware.
- **Python:** `pytest-benchmark`.
- **C/C++:** `google/benchmark`.

Single-shot timers (`time`, `time.time()`, `Instant::now()`) are diagnostic only. They are **not** reportable as benchmarks.

### CPU / wall-time profiling

- **Rust:** `cargo flamegraph` (perf backend on Linux, dtrace on macOS).
- **Python:** `py-spy record -o profile.svg -- <cmd>` (sampling, no code changes, low overhead).
- **C/C++:** `perf record -F 999 -g -- <cmd>` followed by `inferno-flamegraph`.

Profile output is always **flamegraph SVG**, attached to the report. No screenshots, no terminal captures, no proprietary GUI exports.

### Memory profiling

- **Rust:** `dhat` for allocation tracking; `heaptrack` for native traces.
- **Python:** `memray run` followed by `memray flamegraph`.
- **C/C++:** `heaptrack`.

### Coverage

- **Rust:** `cargo llvm-cov --lcov` (replaces `tarpaulin`; faster, branch coverage).
- **Python:** `pytest --cov` with lcov export.
- **C/C++:** `llvm-cov` with `-fprofile-instr-generate -fcoverage-mapping`.

Common output format across languages: **lcov**. Reports include line and branch coverage delta vs. the previous baseline.

### Property-based testing

- **Rust:** `proptest`.
- **Python:** `hypothesis`.
- **C/C++:** `rapidcheck`.

### Static analysis & complexity

Already pinned in Sections 6.2 and 6.3 (clippy, rustfmt, ruff, mypy, cppcheck, clang-tidy, radon, lizard, rust-code-analysis).

### Versions

Tool versions follow a **semver major-locked** policy, declared in `tools.yaml` at the repo root. Claude Code reads `tools.yaml` at session start, alongside `CORE.YAML`.

- Each entry pins a `major_version`. Minor and patch upgrades are **free** and need not be reflected in `tools.yaml`.
- Bumping the **major version** of a pinned tool requires Section 1 approval.
- Versions must be valid semver (`MAJOR.MINOR.PATCH`). Non-semver markers (commit hashes, dates, `latest`, `nightly`) are not accepted; halt and ask if a tool publishes only such markers.
- For 0.x tools, `major_version: "0"` allows any 0.y.z. This is deliberately looser than Cargo's caret semantics.
- Substitution to a **different tool** (e.g., `samply` instead of `cargo flamegraph`) is a separate axis and also requires Section 1 approval.
- If a minor/patch bump introduces a measurement-relevant behavior change (sampling backend swap, output format change, breaking deprecation), note it in the report.

Tools that track an external toolchain (rustup, Linux kernel, LLVM) are declared without `major_version` and follow the toolchain.

---

### The contract gates are not installed here

Added 2026-09-04. This repository does **not** carry a copy of the gates. They are one
implementation in `hymeko/hymeko`, run from git and pinned to a revision in the hook
files:

    SPEC="git+ssh://git@192.168.1.228:2222/hymeko/hymeko.git@8871abe7ecc830965b97af1e0a0838fc688b27fc"
    sh tools/hooks/install.sh          # installs the pinned tool, installs the hooks
    sh tools/hooks/install.sh --status # .git/hooks is not tracked; check it

Why not vendored: a copy existed here until 2026-09-04 and had already drifted into
measuring two things wrong — the index overstated the public surface by 7 symbols, and
the duplication baseline read 2 clones / 52 lines against an actual 417 / 8045. Three
copies of a duplication detector is the joke that writes itself. See
`reports/2026-09-04-contract-gates-depinned.md`.

Pin a **revision**, not a branch. A moving pin means the gate's answer changes without
anyone changing anything, and the delta baseline then fails for no reason.

| concern | tool | why this one |
|---|---|---|
| running the pinned gates | **uv** | resolves interpreter, tool and its locked deps; no per-repo Python project |
| parsing another language | **tree-sitter** | the real grammar; a regex counted `pub(crate)` and a `pub fn` in a private `mod` as public surface |
| parsing Python | **`ast`**, stdlib | the canonical parser |
| design by contract | **icontract** | §8 says pre/postconditions; this evaluates them |
| property-based testing | **hypothesis** | pinned above; see §3 for when it is required |
| clone detection | built-in **token** detector (type-2), plus jscpd | §6.7 |
| containers | **Docker**, on amaterasu | below |

**The hooks prefer an installed entry point.** Measured: `uv tool run --from <git spec>`
costs ~785 ms per call against ~320 ms installed, and the hook makes three calls — 2.7 s
versus 1.7 s against a 2 s budget. `install.sh` runs `uv tool install` so the fast path
is the default rather than something a person has to know.

### Docker runs on amaterasu

The default Docker context is `amaterasu` → `ssh://kyberszittya@192.168.1.228`. Set
deliberately, and worth stating because it is not obvious from a workstation:

- The Windows daemon is not running; a local build dies at
  `npipe:////./pipe/dockerDesktopLinuxEngine`.
- This workstation cannot reach `docker.io` or `ghcr.io` at all. **buildx resolves
  registry metadata on the client**, so a BuildKit build fails here even though the
  daemon could pull. `DOCKER_BUILDKIT=0` makes the daemon resolve, and works.
- amaterasu also hosts Gitea (3000; 2222 is git-only SSH; the shell is port 22).
  A gate verified in a container there is verified on the machine CI will use.

Restore with `docker context use amaterasu`. Build with:

    DOCKER_BUILDKIT=0 docker build -f tools/ci/Dockerfile -t hymeko-contract-gates:0.1.0 .
    docker run --rm -v "$PWD:/repo" hymeko-contract-gates:0.1.0 all
