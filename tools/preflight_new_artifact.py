#!/usr/bin/env python3
"""Print what already exists, before a new file or symbol is created.

    python3 tools/preflight_new_artifact.py hymeko_neuro/pgraph/
    python3 tools/preflight_new_artifact.py --symbol ReservoirSampler
    python3 tools/preflight_new_artifact.py --json submit_run_v2.sh

Exit 0 when nothing similar exists. Exit 1 when something does — which is not a
failure, it is the answer: read the candidates, then either extend one or say in
one sentence why it does not fit (§6.6's `rejected … because`).

## Why this one runs first

Every other duplication check in the contract is a detector. The clone gate finds
a copy after it is written, the dead-code sweep finds the original after it is
orphaned, and the ledger records a search that has already happened. Only this one
runs while the duplicate is still a proposal, and the on-record cases are all cases
where a search would have been cheap and simply was not run: ~1700 lines rebuilt
beside an existing crate, a package almost created next to the crate that already
did the job, five submit scripts for one experiment family.

So the design constraint is not accuracy, it is being run. It takes no arguments
beyond the name, needs no install, and answers in well under a second.

## What it matches on

Character trigrams, two ways, over both the symbol index and the tracked
filenames:

  * Dice — how much two names share overall. Catches near-spellings.
  * Overlap — how much of the *shorter* name is inside the longer. Catches the
    case Dice misses, where a short proposal sits inside a longer existing name:
    `pgraph` against `hymeko_pgraph` scores 0.48 by Dice and 0.71 by overlap, and
    the second number is the one that matters, because that is the 2026-06-03
    incident exactly.

Names shorter than `MIN_TRIGRAMS` trigrams are compared by equality only. Overlap
on a three-character name matches half the repository.

It never writes anything, and it is not authoritative: it produces candidates for
a person to reject in writing, which is what §6.6 asks for.

No dependencies. Python 3.11+.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_symbols  # noqa: E402  — same directory, and the source of INDEX

# Chosen from the sweep recorded in reports/2026-09-04-contract-tooling.md: the
# HIGHEST value at which all three on-record incidents are still caught, which is
# the quietest setting that does not lose the cases this exists for. The binding
# constraint is the 2026-06-03 reservoir sampler, which resembles the existing
# `new_reservoir` by 0.67 and disappears above it.
DEFAULT_THRESHOLD = 0.65

# Below this, a name is too short for similarity to mean anything and only an
# exact match counts. "abb" overlaps half the tree; "reservoir" does not.
#
# Six, not five: a three-character name yields five padded trigrams, so a floor of
# five let `abb` match `abbreviation walker` at 0.60. Six excludes names under four
# characters, which is what the sentence above always meant.
MIN_TRIGRAMS = 6

CODE_SUFFIXES = (".rs", ".py", ".sh", ".c", ".cc", ".cpp", ".h", ".hpp", ".toml", ".md", ".yml", ".yaml")

# Repository furniture: every repository has these, and adding one duplicates
# nothing. Being told that four READMEs exist is not the failure this check is
# for, and a hook that objects to a README gets uninstalled by lunchtime.
# Compared after `normalise`, which strips the extension: `Cargo.toml` and
# `Cargo.lock` both arrive here as "cargo".
FURNITURE = frozenset({
    "readme", "license", "licence", "gitignore", "gitattributes",
    "cargo", "changelog", "contributing",
})


@dataclass(frozen=True, order=True)
class Candidate:
    """One existing thing that resembles the proposal."""

    score: float
    kind: str      # "symbol" | "file"
    name: str
    where: str

    def as_dict(self) -> dict:
        return {"score": round(self.score, 3), "kind": self.kind, "name": self.name, "where": self.where}


def normalise(name: str) -> str:
    """`ReservoirSampler`, `reservoir-sampler.py`, `RESERVOIR_SAMPLER` -> `reservoir sampler`.

    Preconditions: any string. Postconditions: lowercase, alphanumeric words
    separated by single spaces, possibly empty.
    """

    name = re.sub(r"\.[A-Za-z0-9]{1,6}$", "", name.strip().strip("/"))
    name = re.sub(r"(?<=[a-z0-9])(?=[A-Z])", " ", name)
    name = re.sub(r"[^A-Za-z0-9]+", " ", name)
    return " ".join(name.lower().split())


def trigrams(text: str) -> set[str]:
    padded = f"  {text}  "
    return {padded[i:i + 3] for i in range(len(padded) - 2)}


def similarity(a: str, b: str) -> float:
    """max(Dice, overlap) over character trigrams; equality-only for short names.

    Postconditions: 0.0 <= result <= 1.0, and result == 1.0 iff the normalised
    names are equal.
    """

    if a == b:
        return 1.0
    ta, tb = trigrams(a), trigrams(b)
    if min(len(ta), len(tb)) < MIN_TRIGRAMS:
        return 0.0
    shared = len(ta & tb)
    if not shared:
        return 0.0
    dice = 2 * shared / (len(ta) + len(tb))
    overlap = shared / min(len(ta), len(tb))
    return min(max(dice, overlap), 0.999)


SYMBOL_LINE = re.compile(r"^- `([^`]+)` — `([^`]+)`$")


def index_symbols(repo: str) -> tuple[list[tuple[str, str]], str]:
    """(name, path) for every symbol, and where the corpus came from.

    Reads the committed index when it is there. When it is not — another
    repository, a fresh checkout — it builds one in memory with `gen_symbols`
    rather than growing a second idea of what a public symbol is.
    """

    path = os.path.join(repo, gen_symbols.INDEX)
    if os.path.exists(path):
        out = []
        with open(path, encoding="utf-8") as fh:
            for line in fh:
                m = SYMBOL_LINE.match(line.rstrip("\n"))
                if m:
                    out.append((m.group(1), m.group(2)))
        return out, gen_symbols.INDEX

    _, packages, _ = gen_symbols.build(repo)
    return [(s.name, s.path) for p in packages for s in p.symbols], "built in memory (no committed index)"


def tracked_files(repo: str) -> list[str]:
    """Tracked paths, from git; a plain walk when this is not a checkout."""

    try:
        out = subprocess.run(
            ["git", "-C", repo, "ls-files"],
            capture_output=True, text=True, check=True,
        ).stdout
        files = [line.strip() for line in out.splitlines() if line.strip()]
        if files:
            # Same exclusion the index uses: a fixture tree is a test input, and
            # matching a proposal against it would report candidates that no
            # caller can reach.
            return [f for f in files if not (set(f.split("/")) & gen_symbols.SKIP_DIRS)]
    except (OSError, subprocess.CalledProcessError):
        pass
    return gen_symbols.walk_sources(repo)


@dataclass
class Corpus:
    symbols: list[tuple[str, str]]
    files: list[str]
    source: str

    @classmethod
    def load(cls, repo: str) -> "Corpus":
        symbols, source = index_symbols(repo)
        return cls(symbols=symbols, files=tracked_files(repo), source=source)


# Basenames that identify a role rather than an artifact. Proposing one of these
# says nothing on its own, so the directory it sits in is matched with it.
GENERIC_BASENAMES = frozenset({
    "lib", "main", "mod", "init", "index", "setup", "cargo", "pyproject",
    "makefile", "cmakelists", "readme", "build", "conftest", "test", "tests", "plan",
})


def proposal_terms(proposal: str, as_path: bool) -> list[str]:
    """The strings a proposal should be matched under.

    The **last** component only — the basename, or the final directory for a
    proposal ending in `/`. That is what names the artifact:
    `hymeko_neuro/pgraph/` duplicates `hymeko_pgraph` by its last component and
    by nothing else.

    Matching the intermediate directories as well was measured and rejected: it
    lifted the noise rate on twenty unrelated names from 3/20 to 19/20, because
    every file under `hymeko_pgraph/` then matched every other one through a
    directory name none of them chose.

    The exception is a basename that names a role rather than a thing — `lib.rs`,
    `mod.rs`, `__init__.py`, `Cargo.toml`. There the parent directory is what
    carries the identity, so it is matched too.
    """

    if not as_path:
        return [normalise(proposal)]

    parts = [p for p in proposal.replace(os.sep, "/").split("/") if p and p != "."]
    if not parts:
        return []

    last = normalise(parts[-1])
    if last in FURNITURE:
        return []
    terms = [last] if last else []
    if last in GENERIC_BASENAMES and len(parts) > 1:
        parent = normalise(parts[-2])
        if parent:
            terms.append(f"{parent} {last}")
    return [t for t in dict.fromkeys(terms) if t]


def search(corpus: Corpus, terms: list[str], threshold: float,
           exclude: str | None = None) -> list[Candidate]:
    """Every corpus entry resembling any term, best score first.

    `exclude` drops the proposal's own path. This is not a nicety: the hook runs
    on files that are already staged, so `git ls-files` lists them and every added
    file matched itself at 1.00 — a gate that blocked every new file, which is a
    gate nobody keeps. Found by running the hook, not by reading it.
    """

    best: dict[tuple[str, str, str], float] = {}

    for name, where in corpus.symbols:
        if where == exclude:
            continue
        key = ("symbol", name, where)
        norm = normalise(name)
        for term in terms:
            score = similarity(term, norm)
            if score >= threshold:
                best[key] = max(best.get(key, 0.0), score)

    for path in corpus.files:
        if path == exclude:
            continue
        base = path.rpartition("/")[2]
        key = ("file", base, path)
        norm = normalise(base)
        for term in terms:
            score = similarity(term, norm)
            if score >= threshold:
                best[key] = max(best.get(key, 0.0), score)

    return sorted(
        (Candidate(score, kind, name, where) for (kind, name, where), score in best.items()),
        key=lambda c: (-c.score, c.kind, c.name, c.where),
    )


def looks_like_path(proposal: str) -> bool:
    return "/" in proposal or os.sep in proposal or proposal.endswith(CODE_SUFFIXES)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("proposals", nargs="*", metavar="PATH_OR_NAME",
                        help="paths or symbol names about to be created")
    parser.add_argument("--path", action="append", default=[], metavar="PATH",
                        help="force a proposal to be read as a path (repeatable)")
    parser.add_argument("--symbol", action="append", default=[], metavar="NAME",
                        help="force a proposal to be read as a symbol name (repeatable)")
    parser.add_argument("--root", default=None, metavar="DIR",
                        help="repository to search (default: the one holding this script)")
    parser.add_argument("--threshold", type=float, default=DEFAULT_THRESHOLD, metavar="F",
                        help=f"similarity to report at (default {DEFAULT_THRESHOLD})")
    parser.add_argument("--top", type=int, default=8, metavar="N",
                        help="candidates shown per proposal (default 8)")
    parser.add_argument("--json", action="store_true", help="machine-readable output, for the hook")
    args = parser.parse_args()

    proposals: list[tuple[str, bool]] = []
    proposals += [(p, True) for p in args.path]
    proposals += [(s, False) for s in args.symbol]
    proposals += [(p, looks_like_path(p)) for p in args.proposals]
    if not proposals:
        parser.error("nothing proposed — pass a path or a name")

    repo = os.path.abspath(args.root) if args.root else os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if not os.path.isdir(repo):
        print(f"not a directory: {repo}", file=sys.stderr)
        return 2

    corpus = Corpus.load(repo)

    results = []
    for proposal, as_path in proposals:
        terms = proposal_terms(proposal, as_path)
        own = proposal.replace(os.sep, "/").strip("/") if as_path else None
        found = search(corpus, terms, args.threshold, exclude=own)
        results.append((proposal, as_path, terms, found))

    any_found = any(found for _, _, _, found in results)

    if args.json:
        print(json.dumps({
            "repo": os.path.basename(repo),
            "corpus": {"source": corpus.source, "symbols": len(corpus.symbols), "files": len(corpus.files)},
            "threshold": args.threshold,
            "candidates_found": any_found,
            "proposals": [
                {"proposal": p, "as_path": ap, "terms": t,
                 "candidates": [c.as_dict() for c in f[:args.top]]}
                for p, ap, t, f in results
            ],
        }, indent=2))
        return 1 if any_found else 0

    print(f"preflight — {len(corpus.symbols)} symbol(s) from {corpus.source}, "
          f"{len(corpus.files)} tracked file(s), threshold {args.threshold}")
    for proposal, as_path, terms, found in results:
        print()
        if as_path and not terms:
            print(f"  {proposal}   (path; repository furniture — not an artifact, not checked)")
            continue
        print(f"  {proposal}   ({'path' if as_path else 'symbol'}; matched as: {', '.join(terms)})")
        if not found:
            print("    no candidates — creation is authorised by this check")
            continue
        for candidate in found[:args.top]:
            print(f"    {candidate.score:.2f}  {candidate.kind:<6} {candidate.name}   {candidate.where}")
        if len(found) > args.top:
            print(f"    … and {len(found) - args.top} more")

    if any_found:
        print()
        print("Candidates exist. Extend one, or record in the ledger which you rejected and why.")
    return 1 if any_found else 0


if __name__ == "__main__":
    sys.exit(main())
