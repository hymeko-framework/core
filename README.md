# hymeko_core

The HyMeKo semantic core — a standalone cargo workspace.

- **`hymeko_core/`** — the **IR** (`ir/`), **encoding**/serialization (`writers/` — CBOR + IR writers), and tensor primitives (`tensor/`: COO/CSR representations, aggregation, conv traits).
- **`hymeko_hnn/`** — the **hypergraph representation** (`traversal/`: `HyperGraphView`, incidence views) and the **tensor-transformation** operators (`tensor/`: HGNN / signed-HGNN / message-passing).

`parser` is an external repository (`hymeko/parser`), pulled as a `git` dependency.

## Build & test

```sh
cargo build --workspace
cargo test  --workspace
```

Optional features (off by default): `hymeko_core/arrow-schema` (Arrow tensor schema), `hymeko_core/ipc` (iceoryx2 IPC).

## Fixtures

`data/{minimal_examples,robotics,typical_graphs}/` hold the `.hymeko` fixtures the integration tests load.

## Provenance / notes

- Migrated from the `hymeko_framework_rust` monorepo: IR + hypergraph representation + tensor transformation + encoding.
- The `hymeko_hre` runtime engine is **not** part of this repo; the ~6 integration tests that reached into it were gated out.
- `test_load_meta_geometry` asserts a node count (57) that drifted to 58 in the monorepo's own parser; it fails identically upstream (pre-existing, unrelated to this migration).
