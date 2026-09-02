# hymeko — Python bindings for the HyMeKo parser & canonical IR

`import hymeko` gives Python read-only access to the **real** HyMeKo parser and the
`hymeko_core` canonical intermediate representation (`Ir`). Parsing, name resolution,
lowering, coherence validation and canonical hashing all run through the genuine Rust
pipeline — there is no second IR, no JSON intermediate, and no Python-side model of the
graph. Every object you get back is a read-only *value view* over the canonical `Ir`.

## Install (from source, project-local)

This project builds with [maturin](https://www.maturin.rs/) and PyO3. Using a
project-local Python (no system install needed) via [uv](https://docs.astral.sh/uv/):

```bash
uv venv --python 3.12 .venv
uv pip install --python .venv "maturin>=1.7,<2.0" pytest
.venv/Scripts/maturin develop --release      # or: maturin build --release  → wheel in target/wheels/
```

The wheel is `abi3` (CPython ≥ 3.12), so one wheel covers 3.12/3.13/3.14.

## Quickstart

```python
import hymeko

# parse in-memory source, or load a file (with @"import.hymeko" includes resolved):
model = hymeko.parse('node a { }\nnode b { }\n@e { a -> b; }', filename="<memory>")
model = hymeko.load("network.hymeko")

print(model.node_count, model.edge_count, model.hyperarc_count)
print(model.canonical_hash())          # 64-hex, bit-identical to the Rust oracle

for inc in model.incidences_of_arc(model.hyperarcs()[0].id):
    print(inc.arc, inc.slot, inc.sign, inc.target)   # canonical Ir incidence, no internal index
```

The example never uses a raw Rust index — you address entities by their typed IDs
(`DeclId`, `NodeId`, `EdgeId`, `HyperArcId`), obtained from the model.

## What is exposed

- **Counts** — `declaration_count`, `node_count`, `edge_count`, `hyperarc_count`.
- **Entities** — `declaration(id)`, `node(id)`, `edge(id)`, `hyperarc(id)` and the
  iterators `declarations()`, `nodes()`, `edges()`, `hyperarcs()`. Each view exposes the
  canonical state: name & canonical path, declaration hierarchy, `base`/`isa` links,
  target, slot, `+`/`−`/`~` sign, weights, annotations, values.
- **Hierarchy** — `parent_of(decl_id)`, `children_of(decl_id)`.
- **Incidences** — `incidences_of_arc(arc_id)` (forward, from the arc's refs),
  `incidences_of_node(node_id)` (a query over the canonical `Ir`), and the batch
  `incidence_arrays()` (arc-major sparse SoA, one bulk copy, no per-incidence Python
  object).
- **Hash** — `canonical_hash()`.

## ID safety

`DeclId` / `NodeId` / `EdgeId` / `HyperArcId` are distinct Python types that each carry a
**model-instance token**. They are not interchangeable: passing a `NodeId` where an
`EdgeId` is expected, or an ID from another model, raises a typed error
(`HyMeKoIdError` / cross-model). The raw integer is **not** a public contract, and IDs are
**not** stable across pickling, snapshots, or transactions yet — see
[docs/python-architecture.md](../docs/python-architecture.md) for the planned migration to
generational canonical IDs.

## Errors

Every parser/lowering/validation failure is a typed Python exception — never a Rust panic
across the FFI boundary: `HyMeKoSyntaxError`, `HyMeKoResolutionError`,
`HyMeKoLoweringError`, `HyMeKoValidationError`, `HyMeKoIoError`, `HyMeKoIdError`.

## Status

Proof-of-work 0: read-only. Mutation/transaction APIs, the O(1) borrowed
incidence/star view (from the canonical-incidence-backbone core work), and optional
NumPy/WGPU integration are future steps.
