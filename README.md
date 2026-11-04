# core

The HyMeKo semantic core: module loading and imports, name resolution,
validation, lowering to the canonical intermediate representation (IR),
canonical encoding and hashes, and the adapters from the IR to tensors.
Source parsing lives in [parser](https://github.com/hymeko-framework/parser),
numerical kernels in [tensor](https://github.com/hymeko-framework/tensor) and
hypergraph algorithms in [graph](https://github.com/hymeko-framework/graph).

## Crates

| Crate | Contents |
|---|---|
| `hymeko_core` (library `hymeko`) | The IR (`ir`), module store and compiled models (`module_store`), resolution, CBOR and IR writers (`writers`), IR streaming (`ir_stream`), incidence projections and tensor adapters (`tensor`), graph generators (`generators`) and measurement utilities (`util::measurement`) |
| `hymeko_wgpu` | WGPU compute backend (DX12 and Vulkan) for Tensor-owned kernels: CSR SpMM, activations, Chebyshev-Catmull-Rom inference, a pipeline cache and buffer arenas; it does not depend on the IR |

## Use

```toml
[dependencies]
hymeko_core = { git = "https://github.com/hymeko-framework/core.git" }
```

```rust
use std::path::Path;

use hymeko::module_store::execution::CompiledModel;
use hymeko::module_store::source_provider::MemProvider;
use hymeko::util::real_parser::RealParser;

let sources = MemProvider::default()
    .with_file("demo.hymeko", "demo_description {} demo { body {} }");
let model = CompiledModel::compile(sources, RealParser, Path::new("demo.hymeko"))?;

let ir = &model.program().ir;
assert_eq!(ir.decl_nodes.len(), 2);
let digest = model.semantic_digest();
```

A `CompiledModel` is an immutable compiled program with its symbol table.
Sources come from a `SourceProvider`: `StdFsProvider` reads the file system,
`MemProvider` serves in-memory text, `OverlayProvider` layers one over the
other and `ProjectProvider` confines reads to a project root within byte and
file limits. Checked value updates derive new models that share unchanged
structure; `ExecutionState` publishes snapshots to concurrent readers.

## Documentation

- [docs/hymeko-language.md](docs/hymeko-language.md): what a `.hymeko` model
  can express, construct by construct.
- [docs/api-notes.md](docs/api-notes.md): contracts of value updates,
  snapshot retirement, programmatic construction, incidence projections and
  the measurement utilities.
- [docs/unified-ir-architecture.md](docs/unified-ir-architecture.md): the
  design of the canonical IR: transactions, distributed updates, derived
  views and serialization roles.
- [docs/wgpu-backend.md](docs/wgpu-backend.md): the WGPU backend.

## Build and test

```sh
cargo test --locked
cargo test --locked -p hymeko_core --lib
cargo bench -p hymeko_core --bench ir_bench
```

`data/` holds the `.hymeko` fixtures the integration tests load. Measurement
tests stay ignored unless selected explicitly.

`cargo dev-test` and `cargo dev-run -- MODEL.hymeko` build the tests and the
compiler CLI against a dynamic-library build of `hymeko_core`, which shortens
incremental rebuilds; see [dev/README.txt](dev/README.txt).

## License

MIT; see [LICENSE](LICENSE).
