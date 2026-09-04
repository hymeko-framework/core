#!/usr/bin/env python3
"""Generate `SYMBOLS.md` — the public surface of every package, in one file.

    python3 tools/gen_symbols.py            # write SYMBOLS.md
    python3 tools/gen_symbols.py --emit     # print it instead of writing
    python3 tools/gen_symbols.py --check    # exit 1 if the committed file is stale

## Why an index exists at all

The contract's discovery rule (§6.1) requires a search before any new artifact,
and its ownership rule (§0.1) requires knowing which package owns a capability
before deciding how to build it. Both are correct and both are expensive: done
honestly, placing one function means reading a dozen manifests and READMEs.

A cost that high is not paid, it is skipped — and the two on-record duplication
incidents in this project were both skipped searches, not wrong ones. So the
index is not documentation. It is the cheap form of a check that is otherwise
avoided, which is the only reason to generate it mechanically rather than write
it once and let it rot.

## What it will not tell you

That a symbol is the right one to extend. §0.1's rule stands: the documentation
is the index, the code is the authority. This file is the index — the fastest
way to find the candidates, and never the last word on whether one fits.

## Determinism

The output contains no timestamp, no absolute path and no host detail, because
`--check` has to be able to fail for one reason only: the surface moved. Every
list is sorted; two runs on the same tree produce byte-identical output.

No dependencies. Python 3.11+.
"""

from __future__ import annotations

import argparse
import ast
import os
import re
import sys
from dataclasses import dataclass, field

# The one string. The hook and the CI gate import it from here rather than
# spelling it again, so a rename cannot leave three files disagreeing.
INDEX = "SYMBOLS.md"

GENERATED_BY = "tools/gen_symbols.py"

# Directories never descended into. `target/` and `tests/` are excluded from
# Rust extraction specifically (below); these are excluded everywhere.
#
# `fixtures` is here because a fixture tree is an INPUT to a test, not part of the
# repository's surface: its symbols are not callable by anyone, and its
# deliberately near-identical files are not duplication. Pointing `--root` at a
# fixture still indexes it — the exclusion is on the path relative to the root.
SKIP_DIRS = frozenset({
    ".git", ".github", "target", "build", "dist", "node_modules",
    "__pycache__", ".venv", "venv", ".mypy_cache", ".pytest_cache", ".ruff_cache",
    "fixtures",
})


@dataclass(frozen=True, order=True)
class Symbol:
    """One public name, and the file it was found in.

    Ordered so that a list of symbols sorts deterministically without the
    caller having to remember a key.
    """

    kind: str
    name: str
    path: str


class Extractor:
    """Strategy: one language's notion of a public symbol.

    Preconditions: `extract` receives the decoded text of a file whose suffix
    is in `suffixes`, and a repository-relative path used only for provenance.
    Postconditions: returns symbols whose `kind` is drawn from `kinds`, in any
    order; the caller sorts. Never raises on malformed input — a file that
    cannot be parsed yields no symbols and one entry in `problems`.
    """

    language = ""
    suffixes: tuple[str, ...] = ()
    kinds: tuple[str, ...] = ()

    def __init__(self) -> None:
        self.problems: list[str] = []

    def wants(self, relpath: str) -> bool:
        """Suffix matches, and the file is not a test.

        Tests are excluded in every language, not only in Rust: a `test_*`
        function is not something a caller can reach, and an index that lists
        them is an index nobody finishes reading.
        """

        if not relpath.endswith(self.suffixes):
            return False
        parts = relpath.split("/")
        if "tests" in parts or "test" in parts:
            return False
        base = parts[-1]
        return not (base.startswith("test_") or base == "conftest.py" or base.endswith("_test.py"))

    def extract(self, text: str, relpath: str) -> list[Symbol]:
        raise NotImplementedError


class RustExtractor(Extractor):
    """`pub fn` / `pub struct` / `pub trait` / `pub enum`, excluding tests.

    `pub(crate)` and `pub(super)` are deliberately not counted: they are not
    the crate's surface, they are its internals spelled loudly.
    """

    language = "rust"
    suffixes = (".rs",)
    kinds = ("trait", "struct", "enum", "fn")

    DECL = re.compile(
        r"^\s*pub\s+"
        r"(?:async\s+|unsafe\s+|const\s+|extern\s+\"[^\"]*\"\s+)*"
        r"(fn|struct|trait|enum)\s+"
        r"([A-Za-z_][A-Za-z0-9_]*)"
    )

    def wants(self, relpath: str) -> bool:
        # `target/` is already skipped by the walk; repeated here so that the rule
        # survives someone pointing an extractor at a path list of their own.
        return super().wants(relpath) and "target" not in relpath.split("/")

    def extract(self, text: str, relpath: str) -> list[Symbol]:
        found = []
        for line in text.splitlines():
            m = self.DECL.match(line)
            if m:
                found.append(Symbol(m.group(1), m.group(2), relpath))
        return found


class PythonExtractor(Extractor):
    """Top-level classes and defs, plus `__all__` where it is present.

    Parsed with `ast`, not with a regular expression: a regex counts a `def`
    inside a docstring or a conditional import and misses a decorated class,
    and an index that lies is worse than none.

    Underscore-prefixed top-level names are omitted unless `__all__` names
    them, since the question the index answers is what a caller can reach.
    """

    language = "python"
    suffixes = (".py",)
    kinds = ("class", "def", "export")

    def extract(self, text: str, relpath: str) -> list[Symbol]:
        try:
            tree = ast.parse(text)
        except SyntaxError as err:
            self.problems.append(f"{relpath}: unparseable ({err.msg} at line {err.lineno})")
            return []

        found = []
        for node in tree.body:
            if isinstance(node, ast.ClassDef):
                if not node.name.startswith("_"):
                    found.append(Symbol("class", node.name, relpath))
            elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                if not node.name.startswith("_"):
                    found.append(Symbol("def", node.name, relpath))
            elif isinstance(node, (ast.Assign, ast.AnnAssign)):
                found.extend(self._exports(node, relpath))
        return found

    def _exports(self, node: ast.Assign | ast.AnnAssign, relpath: str) -> list[Symbol]:
        targets = node.targets if isinstance(node, ast.Assign) else [node.target]
        if not any(isinstance(t, ast.Name) and t.id == "__all__" for t in targets):
            return []
        if not isinstance(node.value, (ast.List, ast.Tuple)):
            return []
        names = [e.value for e in node.value.elts
                 if isinstance(e, ast.Constant) and isinstance(e.value, str)]
        return [Symbol("export", n, relpath) for n in names]


class CppExtractor(Extractor):
    """Declarations in headers only.

    A translation unit's definitions are not its interface; the header is. This
    is a heuristic line matcher and is documented as one — C++ cannot be parsed
    by regular expression, and a `libclang` dependency would put the index
    behind an install step, which is how a gate stops being run.
    """

    language = "cpp"
    suffixes = (".h", ".hpp", ".hh", ".hxx")
    kinds = ("type", "decl")

    TYPE = re.compile(
        r"^\s*(?:template\s*<[^>]*>\s*)?"
        r"(?:class|struct|union|enum(?:\s+class)?)\s+"
        r"([A-Za-z_][A-Za-z0-9_]*)\s*[:{;]"
    )
    FUNC = re.compile(
        r"^\s*(?!return\b|else\b|case\b)"
        r"(?:[A-Za-z_][\w:]*\s*(?:<[^>]*>)?[\s\*&]+)+"
        r"([A-Za-z_][A-Za-z0-9_]*)\s*\([^;{]*\)\s*(?:const\s*)?(?:noexcept\s*)?;"
    )

    def extract(self, text: str, relpath: str) -> list[Symbol]:
        found = []
        for line in text.splitlines():
            m = self.TYPE.match(line)
            if m:
                found.append(Symbol("type", m.group(1), relpath))
                continue
            m = self.FUNC.match(line)
            if m:
                found.append(Symbol("decl", m.group(1), relpath))
        return found


EXTRACTORS: dict[str, type[Extractor]] = {
    "rust": RustExtractor,
    "python": PythonExtractor,
    "cpp": CppExtractor,
}

# Manifest filename -> the language it declares. Order matters only for a
# directory carrying more than one, where the first match wins.
MANIFESTS: tuple[tuple[str, str], ...] = (
    ("Cargo.toml", "rust"),
    ("pyproject.toml", "python"),
    ("setup.py", "python"),
    ("CMakeLists.txt", "cpp"),
)

# Suffix -> language, for the fallback packages that have no manifest at all.
SUFFIX_LANGUAGE: dict[str, str] = {}
for _lang, _cls in EXTRACTORS.items():
    for _suffix in _cls.suffixes:
        SUFFIX_LANGUAGE[_suffix] = _lang

NO_CHARTER = "<no charter>"

KIND_TITLES = {
    "trait": "traits",
    "struct": "structs",
    "enum": "enums",
    "fn": "functions",
    "class": "classes",
    "def": "functions",
    "export": "`__all__`",
    "type": "types",
    "decl": "declarations",
}


@dataclass
class Package:
    """A unit of ownership: what a capability would belong *to*."""

    name: str
    language: str
    root: str                      # repo-relative, "" for the repository root
    manifest: str | None           # repo-relative, None for a fallback package
    charter: str = NO_CHARTER
    symbols: list[Symbol] = field(default_factory=list)

    @property
    def sort_key(self) -> tuple[str, str]:
        return (self.root, self.name)


def walk_sources(repo: str) -> list[str]:
    """Every source file under `repo`, repo-relative, with `/` separators.

    Postcondition: no path passes through a directory in `SKIP_DIRS`.
    """

    out = []
    for dirpath, dirnames, filenames in os.walk(repo):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
        for name in sorted(filenames):
            full = os.path.join(dirpath, name)
            rel = os.path.relpath(full, repo).replace(os.sep, "/")
            out.append(rel)
    return out


def _read(repo: str, relpath: str) -> str | None:
    try:
        with open(os.path.join(repo, relpath), encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except OSError:
        return None


def _cargo_name(text: str) -> str | None:
    """The `[package] name` of a Cargo manifest, or None for a workspace root."""

    section = None
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            section = stripped[1:-1]
            continue
        if section == "package":
            m = re.match(r'name\s*=\s*"([^"]+)"', stripped)
            if m:
                return m.group(1)
    return None


def _pyproject_name(text: str) -> str | None:
    section = None
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            section = stripped[1:-1]
            continue
        if section in ("project", "tool.poetry"):
            m = re.match(r'name\s*=\s*"([^"]+)"', stripped)
            if m:
                return m.group(1)
    return None


def _cmake_name(text: str) -> str | None:
    m = re.search(r"project\s*\(\s*([A-Za-z_][A-Za-z0-9_\-]*)", text)
    return m.group(1) if m else None


def manifest_name(repo: str, relpath: str, language: str) -> str | None:
    text = _read(repo, relpath)
    if text is None:
        return None
    if language == "rust":
        return _cargo_name(text)
    if language == "python" and relpath.endswith("pyproject.toml"):
        return _pyproject_name(text)
    if language == "cpp":
        return _cmake_name(text)
    return None


def first_prose_line(text: str) -> str | None:
    """The first line of a README that reads as a sentence, not as furniture.

    A heading is skipped rather than stripped of its `#`. A README whose first
    line is `# fixture_native` has a charter of `<no charter>` until someone
    writes one — repeating the package's own name back is not a description, and
    an index full of them would teach the reader to stop reading the column.
    """

    for line in text.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith(("#", "---", "===", "<!--", "![", "[!", "|", ">")):
            continue
        return stripped
    return None


def python_docstring_candidates(repo: str, root: str) -> list[str]:
    """Where a package's `__init__.py` might be, nearest first.

    A `pyproject.toml` sits at the distribution root; the module it declares is
    usually one directory down, in `<name>/` or `src/<name>/`. Looking only at
    the root finds nothing, which is what the first version did.
    """

    def under(*names: str) -> str:
        return "/".join(p for p in (root, *names) if p)

    candidates = [under("__init__.py")]
    for parent in ("", "src"):
        directory = os.path.join(repo, *(p for p in (root, parent) if p))
        if not os.path.isdir(directory):
            continue
        for child in sorted(os.listdir(directory)):
            if child in SKIP_DIRS or not os.path.isdir(os.path.join(directory, child)):
                continue
            candidates.append(under(*(p for p in (parent, child, "__init__.py") if p)))
    return candidates


def charter_of(repo: str, package: Package) -> str:
    """First doc comment, else the README's first prose line, else NO_CHARTER."""

    def under(*names: str) -> str:
        return "/".join(p for p in (package.root, *names) if p)

    if package.language == "rust":
        for entry in ("src/lib.rs", "src/main.rs"):
            text = _read(repo, under(entry))
            if text is None:
                continue
            for line in text.splitlines():
                stripped = line.strip()
                if stripped.startswith("//!"):
                    prose = stripped[3:].strip()
                    if prose:
                        return prose
                elif stripped and not stripped.startswith("//"):
                    break

    if package.language == "python":
        for candidate in python_docstring_candidates(repo, package.root):
            text = _read(repo, candidate)
            if text is None:
                continue
            doc = _module_doc(text)
            if doc:
                return doc

    for entry in ("README.md", "README.rst", "README.txt", "README"):
        text = _read(repo, under(entry))
        if text is not None:
            line = first_prose_line(text)
            if line:
                return line

    return NO_CHARTER


def _module_doc(text: str) -> str | None:
    try:
        doc = ast.get_docstring(ast.parse(text))
    except SyntaxError:
        return None
    if not doc:
        return None
    return doc.strip().splitlines()[0].strip() or None


def discover_packages(repo: str, files: list[str]) -> list[Package]:
    """Manifests first; then top-level source directories nothing else claims.

    Postcondition: package roots are unique, and every returned package has at
    least one source file assigned by `assign_symbols`, except manifest
    packages, which are reported even when empty — an empty declared package is
    itself worth seeing.
    """

    packages: dict[str, Package] = {}

    for rel in files:
        directory, _, base = rel.rpartition("/")
        for manifest, language in MANIFESTS:
            if base != manifest:
                continue
            if directory in packages:
                break
            name = manifest_name(repo, rel, language) or (directory.rpartition("/")[2] or _repo_name(repo))
            packages[directory] = Package(name=name, language=language, root=directory, manifest=rel)
            break

    claimed = set(packages)
    for rel in files:
        suffix = "." + rel.rpartition(".")[2] if "." in rel else ""
        language = SUFFIX_LANGUAGE.get(suffix)
        if language is None:
            continue
        if _owner(claimed, rel) is not None:
            continue
        top = rel.partition("/")[0] if "/" in rel else ""
        if top in packages:
            continue
        name = top or _repo_name(repo)
        packages[top] = Package(name=name, language=language, root=top, manifest=None)
        claimed.add(top)

    for package in packages.values():
        package.charter = charter_of(repo, package)

    return sorted(packages.values(), key=lambda p: p.sort_key)


def _repo_name(repo: str) -> str:
    return os.path.basename(os.path.abspath(repo)) or "repository"


def _owner(roots: set[str], relpath: str) -> str | None:
    """The deepest package root containing `relpath`, or None.

    Deepest wins so that a crate nested inside a script directory keeps its own
    symbols instead of donating them to the directory above it.
    """

    best = None
    for root in roots:
        if root == "":
            candidate = ""
        elif relpath.startswith(root + "/"):
            candidate = root
        else:
            continue
        if best is None or len(candidate) > len(best):
            best = candidate
    return best


def assign_symbols(repo: str, packages: list[Package], files: list[str]) -> list[str]:
    """Fill each package's symbol list. Returns the problems encountered."""

    extractors = {language: cls() for language, cls in EXTRACTORS.items()}
    roots = {p.root for p in packages}
    by_root = {p.root: p for p in packages}

    for rel in files:
        suffix = "." + rel.rpartition(".")[2] if "." in rel else ""
        language = SUFFIX_LANGUAGE.get(suffix)
        if language is None:
            continue
        extractor = extractors[language]
        if not extractor.wants(rel):
            continue
        owner = _owner(roots, rel)
        if owner is None:
            continue
        text = _read(repo, rel)
        if text is None:
            continue
        by_root[owner].symbols.extend(extractor.extract(text, rel))

    for package in packages:
        package.symbols = sorted(set(package.symbols))

    problems: list[str] = []
    for extractor in extractors.values():
        problems.extend(extractor.problems)
    return sorted(problems)


def render(packages: list[Package]) -> str:
    """The index, as markdown. Deterministic: no clock, no host, no absolute path."""

    total = sum(len(p.symbols) for p in packages)
    lines: list[str] = [
        f"<!-- generated by {GENERATED_BY} — do not edit by hand.",
        f"     regenerate:  python3 {GENERATED_BY}",
        f"     check:       python3 {GENERATED_BY} --check -->",
        "",
        f"# {INDEX}",
        "",
        "The public surface of every package in this repository, generated so that the",
        "discovery search §6.1 requires is cheap enough to actually run. It is the index;",
        "the code is the authority (§0.1). A name here is a candidate to extend, never a",
        "verdict that it fits.",
        "",
        f"**{len(packages)} package(s), {total} public symbol(s).**",
        "",
    ]

    for package in packages:
        lines.append(f"## {package.name} ({package.language})")
        lines.append("")
        lines.append(f"- **Root:** `{package.root or '.'}`")
        lines.append(f"- **Manifest:** {'`' + package.manifest + '`' if package.manifest else 'none — inferred from sources'}")
        lines.append(f"- **Charter:** {package.charter}")
        lines.append("")

        if not package.symbols:
            lines.append("_No public symbols._")
            lines.append("")
            continue

        order = EXTRACTORS[package.language].kinds
        for kind in order:
            group = [s for s in package.symbols if s.kind == kind]
            if not group:
                continue
            lines.append(f"### {KIND_TITLES.get(kind, kind)}")
            lines.append("")
            for symbol in group:
                lines.append(f"- `{symbol.name}` — `{symbol.path}`")
            lines.append("")

    return "\n".join(lines).rstrip("\n") + "\n"


def build(repo: str) -> tuple[str, list[Package], list[str]]:
    files = walk_sources(repo)
    packages = discover_packages(repo, files)
    problems = assign_symbols(repo, packages, files)
    return render(packages), packages, problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--emit", action="store_true", help="print the index instead of writing it")
    parser.add_argument("--check", action="store_true", help=f"exit 1 if {INDEX} is stale")
    parser.add_argument("--root", default=None, metavar="DIR", help="repository to index (default: the one holding this script)")
    parser.add_argument("--stats", action="store_true", help="print package and symbol counts to stderr")
    args = parser.parse_args()

    repo = os.path.abspath(args.root) if args.root else os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if not os.path.isdir(repo):
        print(f"not a directory: {repo}", file=sys.stderr)
        return 2

    text, packages, problems = build(repo)
    for problem in problems:
        print(f"warning: {problem}", file=sys.stderr)

    if args.stats:
        for package in packages:
            print(f"  {package.name:<24} {package.language:<8} {len(package.symbols):>5} symbol(s)", file=sys.stderr)

    target = os.path.join(repo, INDEX)

    if args.emit:
        sys.stdout.write(text)
        return 0

    if args.check:
        try:
            with open(target, encoding="utf-8") as fh:
                on_disk = fh.read()
        except FileNotFoundError:
            print(f"{INDEX} is missing — run: python3 {GENERATED_BY}", file=sys.stderr)
            return 1
        if on_disk != text:
            print(f"{INDEX} is stale — run: python3 {GENERATED_BY}", file=sys.stderr)
            fresh_lines, disk_lines = text.splitlines(), on_disk.splitlines()
            print(f"  on disk: {len(disk_lines)} lines, fresh: {len(fresh_lines)} lines", file=sys.stderr)
            for line in sorted(set(fresh_lines) - set(disk_lines))[:10]:
                print(f"  + {line}", file=sys.stderr)
            for line in sorted(set(disk_lines) - set(fresh_lines))[:10]:
                print(f"  - {line}", file=sys.stderr)
            return 1
        return 0

    with open(target, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)
    print(f"wrote {INDEX}: {len(packages)} package(s), "
          f"{sum(len(p.symbols) for p in packages)} symbol(s)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
