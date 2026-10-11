# hymeko_core

The HyMeKo semantic core — a standalone cargo workspace.

Checked value updates retain only the current graph's referenced symbols.
Canonical CBOR export and admitted decode use the same symbol compaction;
superseded timestamp/text values do not accumulate in the current snapshot.
Declaration IDs, graph semantics and canonical digests are preserved. Symbol IDs
are local to a snapshot and may be renumbered: pair them with that snapshot's
interner/index. Explicitly retained old snapshots remain unchanged and readable.
See [the symbol-lifetime plan](docs/plans/2026-09-25-symbol-lifetime/plan.pdf).

- **`hymeko_core/`** — the **IR** (`ir/`), **encoding**/serialization (`writers/` — CBOR + IR writers), and canonical IR-to-tensor adapters (`tensor/`: reference-weight extraction and `TensorInc`).
- **`hymeko_wgpu/`**: WGPU compute backend, consuming `hymeko_tensor` directly.

The `hymeko_hnn` implementation lives in `hymeko_nagare::hnn` (the Nagare repository). It owns the **hypergraph representation** (`traversal/`: `HyperGraphView`, incidence views) and the **tensor-transformation** operators (`tensor/`: HGNN / signed-HGNN / message-passing).

`parser` is an external repository (`hymeko/parser`), pulled as a `git` dependency.

## Build & test

```sh
cargo build --workspace
cargo test  --workspace
```

Generic numeric contracts, COO/CSR kernels and initialization now belong to
`hymeko_tensor`. Its optional `sparse`, `interop` and `arrow-schema` features enable
parallel sparse storage, Nalgebra/JAX conversion and Arrow schemas. Core uses its
dependency-free default. The retired Iceoryx scaffold is removed; `ir_stream` owns
Core transport. See [the migration and local integration status](docs/hymeko-kernel-ownership.md#tensor-extraction-authorized-2026-09-23).

## Fixtures

`data/{minimal_examples,robotics,typical_graphs}/` hold the `.hymeko` fixtures the integration tests load.

## Provenance / notes

- Migrated from the `hymeko_framework_rust` monorepo: IR + hypergraph representation + tensor transformation + encoding.
- The `hymeko_hre` runtime engine is **not** part of this repo; the ~6 integration tests that reached into it were gated out.
- `test_load_meta_geometry` asserts a node count (57) that drifted to 58 in the monorepo's own parser; it fails identically upstream (pre-existing, unrelated to this migration).
