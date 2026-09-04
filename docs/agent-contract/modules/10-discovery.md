<!-- SYNCED from hymeko/hymeko @ ff0c013 — docs/agent-contract/modules/10-discovery.md
     Do not edit here. The universal modules have ONE source; a fork of §6.5
     in each repository makes every citation in every report ambiguous.
     Edit upstream, then re-sync and record the revision. -->

---
source: CLAUDE.md §0.1, §0.2, §6.1, §6.6, §6.7, lines 93-135, 319-335, 454-513, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: de-projectified (Phase 4)
---

**Contains:** §0.1 ownership pass · §0.2 symbol index · §6.1 the search precondition · §6.6 the ledger · §6.7 the machine gate

# Discovery

### 0.1 Ownership pass — docs index it, code decides it

Before adding a capability, build a short **ownership map**: which package owns each piece of what
was asked for. Start from the documentation — repo `README`s, `docs/`, the charter comments at the
top of each `Cargo.toml` — because that is the fastest index of what lives where.

Then **confirm every entry against the code**, because in this framework the two have disagreed
repeatedly:

- a crate's stated charter described what it was *meant* to contain, while it in fact owned a whole
  domain vocabulary that belonged elsewhere;
- two repositories' `README`s each claimed to be the canonical home of the same crate;
- a module's public API read as exactly the right seam for a new capability, and it was scheduled to
  be retired — which no document recorded.

So: **documentation is the index, the code is the authority.** The load-bearing checks are the
dependency edges in each `Cargo.toml` (which way may a dependency point?), the public exports, and a
grep for the capability under every name it might carry. Where a capability already exists partially,
extend it in the owning package rather than starting a competing one in the consumer.

**A doc that disagrees with the code is a finding.** Report it; do not silently follow either one.
And where placement is still unclear after this pass, stop and ask — a capability put in the wrong
package is expensive to move once anything depends on it.

### 0.2 The symbol index

> **STATUS — built and enforced (2026-09-04).** `tools/contract_tools/index.py`,
> committed `SYMBOLS.md`, `--check` in the pre-commit hook and in CI. This section
> is no longer a specification.

`SYMBOLS.md` at repo root is a generated, always-read index:

    uv run contract-symbols            # regenerate
    uv run contract-symbols --check    # exit 1 if it is stale

It lists, per crate/package: the manifest, the charter one-liner, and the public
symbols by kind. It is generated, never hand-edited.

**Extraction is by grammar, not by pattern.** tree-sitter for Rust and C++; `ast`
for Python, which is the canonical parser and not a compromise. This is a rule and
not an implementation note, because the earlier `rg`-over-`pub fn` version got four
things wrong that no reader of its output could see: it counted `pub(crate)` as
public surface, counted a `pub fn` inside a private `mod` as reachable, missed a
signature spread over two lines, and missed one behind an attribute macro.
Reachability is a property of the tree. A line matcher has no tree.

**The index is a function of the parse, not of the byte stream.** Reformatting,
reflowing a comment or renaming a local must not change it. That is testable and is
tested.

**Ownership comes from the manifest where the manifest states it.** A
`pyproject.toml` at the repository root does not make the whole repository one
package: the index reads `[tool.hatch.build.targets.wheel] packages`. Getting this
wrong attributes files to a distribution that does not contain them, which is the
§0.1 question answered backwards.

§0 step 1 becomes: read the gate file, the toolchain pin, **and `SYMBOLS.md`**.

Rationale: §0.1's ownership pass is correct but expensive — reading ten
crates' READMEs and manifests to place one function is a cost that gets paid
by skipping it. The index makes the cheap version of the pass possible, and
§0.1's code-is-the-authority rule still applies when the index and the code
disagree (that disagreement is a finding; report it).


### 6.1 No Redundant Code

Before adding ANY new artifact — function, class, module, package, Rust crate, Python script, configuration file, plan doc, fixture — **search the codebase first**. This is a precondition on every creation, not a stylistic preference. The search pass is:

1. `find <repo_root> -iname "*<concept>*"` — directories + filenames matching the concept.
2. `grep -rln "<key term>" <relevant_subdirs>` — find existing implementations, partial scaffolds, or stale plans.
3. `ls <candidate_dir>/` and read one or two representative files.
4. Only after this pass returns nothing does new-creation become authorised.

If similar logic exists, extend or refactor — do not paste a near-copy.

- The same algorithm appearing in **three or more** places is a refactor trigger, not a feature.
- Use traits / ABCs / interfaces / generics to unify variants.
- If you find yourself writing a block you have written before in this repository, stop and consolidate.

You are **forbidden** from emitting many copies of essentially the same code. If a unification crosses into `CORE.YAML`-protected files, see Section 1 — do not unify, halt and ask.

The two 2026-06-03 cases that motivated this precondition, and the "why", are in
[incidents/2026-06-03-duplicate-scaffolding.md](../incidents/2026-06-03-duplicate-scaffolding.md).

### 6.6 Discovery Ledger — the search must be VISIBLE, not merely performed

§6.1 requires a search. This section requires its **result to be emitted**.
An unemitted search is indistinguishable from a skipped one, and on the record
it has been skipped more often than performed.

Before any `Write` of a new file, any `mkdir`, any `cargo new`, and before
adding any new `pub fn` / `pub struct` / `pub trait` / top-level Python
class or function, the reply MUST contain a ledger block in exactly this
form, immediately preceding the tool call:

    DISCOVERY LEDGER — <concept>
      find:    <command run>            -> <n> hits: <paths | none>
      grep:    <command run>            -> <n> hits: <paths | none>
      symbols: <SYMBOLS.md lookup>      -> <n> hits: <paths | none>
      read:    <files actually opened, or "none">
      verdict: EXTEND <path>
             | NEW <path>; rejected <candidate> because <one sentence>
             | NEW <path>; no candidate found

Rules:

- `verdict: NEW` with an empty `rejected` line is only valid when all three
  searches returned zero hits. Otherwise the rejection must name the
  candidate and the reason. "Not quite the same" is not a reason; state the
  differing invariant, owner, or contract.
- A creation performed without a preceding ledger is **unauthorised**. It is
  reverted, not reviewed. Do not argue the merits of the code afterwards —
  the merits are not the question; the missing precondition is.
- The ledger goes in the chat reply AND is copied verbatim into the §9
  report under a `## Discovery` heading.
- Zero-hit ledgers are still written. A cheap, boring, empty ledger is the
  normal case and proves the search ran.

### 6.7 Duplication gate (machine-enforced)

> **STATUS — built and enforced (2026-09-04).** `tools/contract_tools/duplication.py`,
> committed `.duplication-baseline.json`, run by the pre-commit hook and by
> `tools/ci/duplication-gate.yml`. jscpd runs in the container image and has been
> executed; its baseline entry is committed alongside the built-in one.

Per the §3 precedent (a reward oracle that refuses an uncertified run), a rule that
matters is wired into tooling, not left to operator discipline.

**A delta gate, never an absolute count.** A change may not increase the clone count
or the cloned-line total against `.duplication-baseline.json`. Lowering it is welcome
and needs no ceremony; raising it needs the written justification a §1 core edit needs.
An absolute ceiling fails on day one in any repository old enough to need one, and a
gate that fails on day one is switched off on day two.

**Duplication is detected on tokens, not on lines.** The detector erases identifiers
and literals to their node type before hashing, so a copy whose variables were renamed
afterwards is still a clone — which is what every duplication in this project's
history actually looked like. A line-hash detector scores those zero. Comments produce
no tokens at all, so reformatting is invisible to it.

**Detectors are never compared against each other.** Each keeps its own baseline
entry, because two detectors with different minimum-clone sizes legitimately disagree:
on this repository the built-in one reports 9 clones / 134 lines and jscpd reports 0,
since jscpd's 20-line floor does not see an 8-line copied function. Comparing across
them would be a gate that fires whenever a tool changes. This is a precondition in
code, not a warning in prose.

1. **Clone detection.** The built-in token detector always, since it needs no install;
   `jscpd --min-lines 20 --min-tokens 120 --reporters json` in the container, because
   it catches cross-language copies the loaded grammars do not cover. `pylint
   --enable=duplicate-code` and `cargo clippy` where those manifests exist.

2. **New-artifact preflight.** `uv run contract-preflight <path-or-name>`: given a
   proposed file or symbol, it reports existing candidates by trigram similarity
   against the symbol index and the tracked filenames. It is the only check in this
   section that runs while the duplicate is still a proposal; the rest are detectors.
   Wired into `commit-msg` — **not** `pre-commit`, which cannot see the message and
   would read the previous commit's, failing open. Exit 1 when candidates exist and no
   `DISCOVERY:` line is present.

3. **Dead-code sweep.** `cargo machete`, `cargo udeps`, `vulture` for Python, per
   change. A duplicate usually leaves the original unreferenced; an orphaned module is
   evidence of a duplication that already happened. **Still not built** — the one part
   of this section that remains a specification.

Report (§9) must quote the clone-delta line.
