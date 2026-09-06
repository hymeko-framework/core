# hymeko_core — architecture map

Read-only audit (Stage 2), branch `feat/core-generational-arenas` @ `67771c9`,
2026-08-31. Metrics are reproducible from the scripts noted below; no production
source was edited to produce this map.

## Scale

`hymeko_core/src`: **63 files, 13,475 LOC**. A large fraction is **inline
`#[cfg(test)]`** — the crate carries its tests next to the code, so raw file LOC
overstates production size. Production-vs-test split for the biggest files:

| file | total | prod | test |
|---|--:|--:|--:|
| ir_stream/assembler.rs | 2512 | ~713 | ~1799 |
| ir_stream/section.rs | 912 | ~539 | ~373 |
| ir/ir.rs | 838 | ~164 | ~674 |
| ir_stream/envelope.rs | 626 | ~431 | ~195 |
| generators.rs | 610 | ~445 | ~165 |
| ir/canonical_hash.rs | 541 | **~541** | 0 |
| ir_stream/codec.rs | 494 | ~180 | ~314 |
| resolution/const_resolve.rs | 493 | ~354 | ~139 |

The only genuinely-large **pure-production** file is `canonical_hash.rs` (541,
hash-order-sensitive). `assembler.rs` at 713 production LOC is the next; the rest
are within the §6.2 800-LOC warning once tests are excluded.

## Modules (bottom-up layering)

| module | prod LOC | pub symbols | role | depends on (internal) |
|---|--:|--:|---|---|
| `common` | 228 | 18 | ids, pathkey, foundation | — |
| `ir_stream` | ~1900/5174 | 125 | binary IR wire codec (envelope/section/codec/assembler) | self only |
| `resolution` | 1393 | 38 | interner, resolve, const-eval, intern pass | common, ir |
| `ir` | ~1200/2649 | 107 | the canonical IR, access, lower, coherence, hashing | common, resolution, **ir_stream** |
| `module_store` | 505 | 29 | compile/orchestration over resolution+ir | resolution, ir, common |
| `tensor` | 2409 | 120 | COO/CSR, SpMV/SpMM, decomposition, compute backends | ir, common (near-isolated: 19 internal edges) |
| `writers` | 158 | 6 | IR pretty/text writers | ir, resolution, common, module_store |
| `util` | 226 | 4 | pretty_print | ir, resolution, module_store, common |

Layering is clean and acyclic at the module level: `common` → {`ir_stream`,
`resolution`} → `ir` → `module_store` → {`writers`, `util`}; `tensor` hangs off
`ir`+`common` and is otherwise self-contained. One edge worth noting: **`ir`
depends on `ir_stream`** (8 use-edges) — the IR knows about its own wire codec.

## External dependencies

`parser` (grammar/AST), `serde`, `rustc-hash`, `blake3` (canonical hash),
`nalgebra` + `nalgebra-sparse` (tensor), `num-traits`, `rayon` (data-parallel),
`iceoryx2` (optional IPC — see `tensor/shared_state.rs`), `arrow` (optional
columnar). The **WGPU backend is a separate crate** (`hymeko_wgpu`) depending on
core; the core-side boundary is the tensor representations it exposes for
readback.

## Risk signals (production only)

| signal | count | note |
|---|--:|---|
| `unsafe` | **1** | `interner.rs:24` — `Box<str>` → `&'static str` intern aliasing; sound (the str lives behind the box's own stable heap allocation, unaffected by `Vec` growth), documented inline |
| `unwrap()` | 24 | mostly test/`const`-context |
| `expect()` | 31 | mostly invariant-guarded seams |
| `panic!()` | 10 | 8 are test assertions in assembler/const_resolve; ~2 production (`compute/mod.rs:111` shape-overflow guard, `decomposition.rs:23` mode precondition) |
| functions > 150 LOC | **3** | coherence `validate_hierarchy_index_coherence` (216), section `validate_sections` (178), assembler `probe_all_ten` (152, a test probe) |
| functions > 7 params | **2** | `lower_arc` (8), `compute_degree_by_index` (8) |
| allocation-hot files | — | intern_pass (34 sites), generators (23), canonical_hash (22), resolve (18) |

By complexity metrics the crate is **healthy**: 614 detected functions, only 3
over 150 LOC and 2 over 7 params. The audit's leverage is in **duplication and
module organization**, not oversized functions.

## Hierarchy traversal (centralized, with two seams)

The decl tree is walked through a proper iterator API (`Ir::decl_children` /
`child_decls`, `first_child_of` / `next_sibling_of`). Raw `first_child` /
`next_sibling` access remains only where it is legitimate: `coherence.rs`
(validating the links themselves) and `lower.rs` (building the links). One
redundancy: **two child-iterator types** — `DeclChildren` (ir.rs, panics on a
malformed link, 2 callers) and `ChildDecls` (access.rs, total/bounded, **0
callers**). See the simplification candidates.

## Reproduce

- LOC/prod-test split: `wc -l` + first `#[cfg(test)]` offset per file.
- Function metrics: `docs/plans/.../` `fnmetrics` heuristic (brace-matched body
  span + top-level param count).
- Dependency edges: `grep -oE "use crate::[a-z_]+"` per module.
- Component diagram: `docs/core-component-diagram.mmd`.
