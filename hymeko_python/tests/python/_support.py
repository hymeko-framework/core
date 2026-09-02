"""Shared test-support for the hymeko Python suite.

Fixture paths, the Rust-computed canonical-hash oracle (kept in lock-step with
`tests/hash_oracle.rs`), and a *deterministic* `.hymeko` **source** emitter for the
large-scale parser tests. The emitter is a trivial text generator (a sliding window
over a node chain) — not a graph-generation framework: the core's combinatorial
generators (`hymeko::generators`) produce index-level designs, not source, and are
not reachable from Python, so they cannot drive parser-first sized tests.
"""

from __future__ import annotations

import pathlib

# tests/python/_support.py -> parents[3] is the worktree root that holds data/.
_ROOT = pathlib.Path(__file__).resolve().parents[3]
DATA = _ROOT / "data"


def fixture(rel: str) -> str:
    p = DATA / rel
    if not p.exists():
        raise FileNotFoundError(p)
    return str(p)


# Canonical-hash oracle: fixture -> hex, computed by the Rust core
# (`canonical_program_hash`) in tests/hash_oracle.rs. The Python suite asserts the
# same values for the same fixtures loaded through `hymeko.load` — that IS the
# Python<->Rust parity check.
ORACLE = {
    "minimal_examples/minimal_example.hymeko": "e7e1f1a305fb5e8754605252f6c351cd9af84211a27ee64fc27d09d0ab2ff738",
    "minimal_examples/minimal_example_base_elements.hymeko": "ee3db3cb11ae66946dd21dbd8c7b6c6593c8ec0362f541ee12564003bde4609b",
    "minimal_examples/minimal_example_basic_hierarchy.hymeko": "1f85625feaf93ec507363cc89bd3eb3f13504a90c408cc486db00c87c705195f",
    "minimal_examples/tag_annotations/minimal_tag_annotation.hymeko": "90e3e0f1d071298dcf6ec9e45ed29ec84aa197715fc008e86887127f74a43a51",
    "minimal_examples/minimal_example_fields_with_reference.hymeko": "93ac4fe4c8f7bcfd594656e25add2f1faea414948adf8905482c1e56f108475f",
    "typical_graphs/fano_graph.hymeko": "b88715ed80e62f247ea71f57e0ffb4f450e86a799e630186ea5ec6a063c86dc5",
    "typical_graphs/k4_3uniform.hymeko": "0a8a903e3f89660db98fb603d06d4993acea1b79e2bed04c8a7a5afe91294237",
    "typical_graphs/sunflower_delta_system.hymeko": "194c71e4164a7aaef3897b59622767fe6533929dde231c2a5d707675dfd2914e",
    "typical_graphs/generic_hypergraph.hymeko": "6a0c09d8cf24f1aa52223ba1378f900516b4fdb1d6495ab122bd8dee35242f89",
}

# A small in-memory model with mixed signs, weights, a duplicate incidence, and an
# annotation — used by the semantic tests.
MIXED_SOURCE = (
    "MixedDesc{ }\n"
    "A;\n"
    "B;\n"
    "C;\n"
    "@E1 {\n"
    "  (+A, -B, ~C);\n"   # +/-/~ signs
    "  (+A, +A);\n"       # duplicate incidence (A twice)
    "}\n"
)


def windowed_source(name: str, n_nodes: int, arity: int) -> str:
    """A deterministic sliding-window hypergraph as `.hymeko` source.

    `n_nodes` nodes `N0..N{n-1}`; one edge with `n_nodes - arity + 1` hyperarcs,
    each referencing `arity` consecutive nodes with a deterministic sign pattern.
    Incidence count = `(n_nodes - arity + 1) * arity`.
    """
    if arity < 1 or n_nodes < arity:
        raise ValueError("need n_nodes >= arity >= 1")
    parts = [f"{name}{{}}"]
    parts.extend(f"N{i};" for i in range(n_nodes))
    parts.append("@E {")
    for i in range(n_nodes - arity + 1):
        refs = ", ".join(
            ("+" if (i + j) % 3 else "-") + f"N{i + j}" for j in range(arity)
        )
        parts.append(f"  ({refs});")
    parts.append("}")
    return "\n".join(parts) + "\n"


def windowed_incidence_count(n_nodes: int, arity: int) -> int:
    return (n_nodes - arity + 1) * arity
