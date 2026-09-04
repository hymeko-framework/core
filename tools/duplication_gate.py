#!/usr/bin/env python3
"""The duplication gate: fail on an INCREASE, never on an absolute count.

    python3 tools/duplication_gate.py                    # measure and compare
    python3 tools/duplication_gate.py --update           # (re)establish the baseline
    python3 tools/duplication_gate.py --report jscpd.json  # compare a jscpd run instead
    python3 tools/duplication_gate.py --json             # machine-readable

## Why a delta and not a threshold

An absolute clone ceiling fails on day one in any repository old enough to need
one, and a gate that fails on day one is switched off on day two. The number that
can honestly be enforced is the second derivative: whatever duplication exists is
the baseline, and a change may not add to it. Lowering the baseline is a separate,
deliberate act; raising it needs the written justification a core edit needs.

## Two detectors, and why there are two

`jscpd` is the cross-language tool the contract names, and the CI workflow runs it.
It is also an npm install, and on the host this was built for `node`, `npx` and
`jscpd` are all absent — so a jscpd-only gate would have been unrunnable in the one
place it had to work first. Hence a built-in detector as well: no install, always
runnable, and honest about being cruder.

The built-in one is a line-window scanner. Source lines are normalised (blank and
comment-only lines dropped, whitespace collapsed), every window of `MIN_LINES`
consecutive normalised lines carrying at least `MIN_TOKENS` tokens is hashed, and a
hash seen more than once marks its lines as cloned. `cloned_lines` is the number of
marked lines; `clones` is the number of maximal contiguous marked runs, so an
overlapping family of windows counts once.

It does not tokenise, so it will miss a copy that renamed its variables — which
`jscpd` catches. The two are not interchangeable and the baseline records each
under its own name; comparing one detector's count against another's would be a
gate that fires on a tool change.

## What is not scanned

Markdown. This repository deliberately holds near-identical prose — a contract
split into modules, incidents quoting the rules they produced — and counting that
as duplication would make the gate meaningless in exactly the repository that
defines it. Duplication in prose is a matter for review, not for a hash.

No dependencies. Python 3.11+.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
from dataclasses import dataclass, field

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_symbols  # noqa: E402  — reuses one notion of "a source tree"

BASELINE = ".duplication-baseline.json"

# Matches the contract's stated jscpd parameters (§6.7), so the two detectors are
# at least looking for the same size of thing.
MIN_LINES = 20
MIN_TOKENS = 120

SOURCE_SUFFIXES = (
    ".rs", ".py", ".sh", ".c", ".cc", ".cpp", ".cxx", ".h", ".hpp", ".hh",
    ".js", ".ts", ".tsx", ".java", ".go", ".toml", ".yml", ".yaml",
)

LINE_COMMENT = re.compile(r"^\s*(#|//|--|;)")
TOKEN = re.compile(r"\w+")


@dataclass(frozen=True)
class Measurement:
    """What a detector found. The unit the baseline stores and compares."""

    detector: str
    clones: int
    cloned_lines: int
    files: int

    def as_dict(self) -> dict:
        return {"clones": self.clones, "cloned_lines": self.cloned_lines, "files": self.files}

    def worse_than(self, other: "Measurement") -> list[str]:
        """Which counts rose. Empty list means the change may pass."""

        rises = []
        if self.clones > other.clones:
            rises.append(f"clones {other.clones} -> {self.clones}")
        if self.cloned_lines > other.cloned_lines:
            rises.append(f"cloned lines {other.cloned_lines} -> {self.cloned_lines}")
        return rises


@dataclass
class Window:
    path: str
    start: int
    end: int


@dataclass
class BuiltinScan:
    """The no-install detector. See the module docstring for what it cannot see."""

    min_lines: int = MIN_LINES
    min_tokens: int = MIN_TOKENS
    marked: dict[str, set[int]] = field(default_factory=dict)

    def sources(self, repo: str) -> list[str]:
        return [f for f in gen_symbols.walk_sources(repo) if f.endswith(SOURCE_SUFFIXES)]

    @staticmethod
    def normalise(text: str) -> list[tuple[int, str]]:
        """(line number, collapsed text) for lines that carry code.

        Postcondition: blank and comment-only lines are absent, so a block of
        code stays one block when its comments are reflowed.
        """

        out = []
        for number, raw in enumerate(text.splitlines(), start=1):
            if not raw.strip() or LINE_COMMENT.match(raw):
                continue
            out.append((number, " ".join(raw.split())))
        return out

    def run(self, repo: str) -> Measurement:
        files = self.sources(repo)
        seen: dict[str, list[Window]] = {}

        for rel in files:
            try:
                with open(os.path.join(repo, rel), encoding="utf-8", errors="replace") as fh:
                    lines = self.normalise(fh.read())
            except OSError:
                continue
            for i in range(len(lines) - self.min_lines + 1):
                window = lines[i:i + self.min_lines]
                body = "\n".join(text for _, text in window)
                if len(TOKEN.findall(body)) < self.min_tokens:
                    continue
                digest = hashlib.sha1(body.encode("utf-8")).hexdigest()
                seen.setdefault(digest, []).append(Window(rel, window[0][0], window[-1][0]))

        self.marked = {}
        for windows in seen.values():
            if len(windows) < 2:
                continue
            for window in windows:
                self.marked.setdefault(window.path, set()).update(range(window.start, window.end + 1))

        cloned_lines = sum(len(v) for v in self.marked.values())
        return Measurement("builtin", self._runs(), cloned_lines, len(files))

    def _runs(self) -> int:
        """Maximal contiguous marked runs, so overlapping windows count once."""

        total = 0
        for numbers in self.marked.values():
            previous = None
            for number in sorted(numbers):
                if previous is None or number != previous + 1:
                    total += 1
                previous = number
        return total

    def report(self) -> list[str]:
        return [f"{path}: {len(numbers)} cloned line(s)" for path, numbers in sorted(self.marked.items())]


def from_jscpd(path: str) -> Measurement:
    """Read a `jscpd --reporters json` report into the same unit.

    Preconditions: `path` is a jscpd JSON report with a `statistics.total` block.
    Postconditions: a Measurement tagged `jscpd`, never mixed with `builtin`.
    """

    with open(path, encoding="utf-8") as fh:
        data = json.load(fh)
    total = data.get("statistics", {}).get("total", {})
    return Measurement(
        detector="jscpd",
        clones=int(total.get("clones", 0)),
        cloned_lines=int(total.get("duplicatedLines", 0)),
        files=int(total.get("sources", 0) or total.get("files", 0)),
    )


def load_baseline(repo: str) -> dict:
    path = os.path.join(repo, BASELINE)
    if not os.path.exists(path):
        return {}
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


def save_baseline(repo: str, baseline: dict) -> None:
    path = os.path.join(repo, BASELINE)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(baseline, fh, indent=2, sort_keys=True)
        fh.write("\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=None, metavar="DIR", help="repository to measure")
    parser.add_argument("--report", default=None, metavar="FILE", help="a jscpd JSON report to read instead of scanning")
    parser.add_argument("--update", action="store_true", help=f"write the measurement into {BASELINE}")
    parser.add_argument("--measure", action="store_true",
                        help="measure and print only — never reads or writes the baseline")
    parser.add_argument("--list", action="store_true", help="print the files carrying cloned lines")
    parser.add_argument("--json", action="store_true", help="machine-readable output")
    args = parser.parse_args()

    repo = os.path.abspath(args.root) if args.root else os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if not os.path.isdir(repo):
        print(f"not a directory: {repo}", file=sys.stderr)
        return 2

    scan = BuiltinScan()
    measurement = from_jscpd(args.report) if args.report else scan.run(repo)

    if args.measure:
        # Read-only by construction: measuring another repository must not leave a
        # file in it. The first version did, in four checkouts, before this existed.
        if args.json:
            print(json.dumps({"detector": measurement.detector, "measured": measurement.as_dict()}, indent=2))
        else:
            print(f"duplication ({measurement.detector}) in {os.path.basename(repo)}: "
                  f"{measurement.clones} clone(s), {measurement.cloned_lines} cloned line(s) "
                  f"across {measurement.files} file(s)")
        if args.list:
            for line in scan.report():
                print(f"  {line}")
        return 0

    baseline = load_baseline(repo)
    detectors = baseline.setdefault("detectors", {})
    recorded = detectors.get(measurement.detector)

    if args.update or recorded is None:
        detectors[measurement.detector] = measurement.as_dict()
        baseline.setdefault("_comment", (
            "Duplication baseline. The gate fails on an INCREASE only. Lowering these "
            "numbers is welcome and needs no ceremony; raising them needs the written "
            "justification a core edit needs. Detectors are not interchangeable — each "
            "is compared only against its own entry."
        ))
        baseline.setdefault("params", {"min_lines": MIN_LINES, "min_tokens": MIN_TOKENS})
        save_baseline(repo, baseline)
        verb = "updated" if args.update else "established"
        print(f"{BASELINE} {verb} for detector '{measurement.detector}': "
              f"{measurement.clones} clone(s), {measurement.cloned_lines} cloned line(s), "
              f"{measurement.files} file(s) scanned")
        if args.list:
            for line in scan.report():
                print(f"  {line}")
        return 0 if args.update else 1

    previous = Measurement(measurement.detector, recorded["clones"], recorded["cloned_lines"], recorded.get("files", 0))
    rises = measurement.worse_than(previous)

    if args.json:
        print(json.dumps({
            "detector": measurement.detector,
            "measured": measurement.as_dict(),
            "baseline": previous.as_dict(),
            "rises": rises,
            "pass": not rises,
        }, indent=2))
        return 1 if rises else 0

    print(f"duplication ({measurement.detector}): "
          f"{measurement.clones} clone(s), {measurement.cloned_lines} cloned line(s) "
          f"across {measurement.files} file(s); "
          f"baseline {previous.clones} / {previous.cloned_lines}")
    if args.list:
        for line in scan.report():
            print(f"  {line}")

    if rises:
        print("duplication increased: " + "; ".join(rises), file=sys.stderr)
        print(f"Consolidate, or justify the rise in writing and run --update.", file=sys.stderr)
        return 1

    if measurement.clones < previous.clones or measurement.cloned_lines < previous.cloned_lines:
        print("duplication fell — run --update to lock the improvement in.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
