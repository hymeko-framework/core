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

> **STATUS — specified, not built (checked 2026-09-04).** No `SYMBOLS.md` and no
> `make symbols` target exists in this repository or in `core/`. The specification
> below is kept verbatim so that it stays buildable; until it is built, §6.6's manual
> ledger is the whole of the discipline. Do not cite this section as a gate that ran.

`SYMBOLS.md` at repo root is a generated, always-read index:

    make symbols     # regenerates; CI fails if the file is stale

It lists, per crate/package: the charter one-liner, the public traits, the
public types, the public functions, and the module's owner concept. It is
generated (`rg` over `pub fn|pub struct|pub trait|pub enum` for Rust,
`__all__` + top-level defs for Python), never hand-edited.

§0 step 1 becomes: read the gate file, the toolchain pin, **and `SYMBOLS.md` where it
exists**. Where it does not, the ownership pass of §0.1 is run the expensive way.

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

> **STATUS — specified, not built (checked 2026-09-04).** None of the three checks
> below exists: no `.duplication-baseline.json`, no `scripts/preflight_new_artifact.py`,
> and no `jscpd` / `vulture` / `cargo machete` / `cargo udeps` wiring in any manifest,
> workflow or Makefile, here or in `core/`. The specification is kept verbatim.

Per the §3 precedent (a reward oracle that refuses an uncertified run), a rule that
matters is wired into tooling, not left to operator discipline. Three checks, to run
in CI and locally:

1. **Clone detection.** `jscpd --min-lines 20 --min-tokens 120 --reporters json`
   over `hymeko_*/`, plus `pylint --disable=all --enable=duplicate-code` for
   Python and `rust-code-analysis` for Rust. The gate is a **delta** gate:
   a change may not increase the clone count or cloned-line total versus the
   baseline in `.duplication-baseline.json`. Raising the baseline requires
   the same written justification as a §1 core edit.

2. **New-artifact hook.** `scripts/preflight_new_artifact.py <path>`:
   given a proposed new file or symbol name, it greps the symbol index and
   the tree for trigram-similar names and prints the candidates. Wire it as
   a pre-commit hook on added files. Exit code 1 when candidates exist and
   no `DISCOVERY:` reference is present in the commit message.

3. **Dead-code sweep.** `cargo machete`, `cargo udeps`, `vulture` for Python,
   run per change. A duplicate usually leaves the original unreferenced;
   an orphaned module is evidence of a duplication that already happened.

Report (§9) must quote the clone-delta line once the gate exists. Until then, "no §6.5
anti-patterns introduced" is a claim made by a person, and it is worth exactly what a
person's unaided grep is worth — which is the reason this section was specified.

