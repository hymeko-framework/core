> Historical PoW-0 design from 2026-09-02. Python bindings now have a separate canonical repository; this document does not declare an active Core workspace member. See reports/2026-09-06-worktree-consolidation.md.

# HyMeKo Python bindings — architecture (PoW-0)

**STRATEGY-ID:** HYMEKO-PLATFORM-2026-09-01-v1 · **ROADMAP-TASK:** PY0. 2026-09-02.
Crate `hymeko_python` (Rust) → Python module `hymeko`. Read-only.

## Dependency & ownership

```
hymeko_python  ──▶  parser        (real .hymeko parser, git dep)
hymeko_python  ──▶  hymeko_core    (canonical Ir, resolution, lowering, canonical hash)
```

`hymeko_core` is **full-locked** (CORE.YAML): the binding consumes its public API only
and adds **no** edit to it. Forbidden and absent here: `hymeko_core → PyO3`, `parser →
Python`, a second IR, an editor IR, a Python-side canonical DTO, a JSON intermediate, a
dense/star graph, a new CLI, and any mutation/transaction API.

### What the Python object owns

A `Model` holds an `Arc<ModelInner>`; `ModelInner` owns exactly what is needed to read
and name the canonical program and nothing more:

- the **`Interner`** (owned; moved out of the transient `ModuleStore` with
  `std::mem::take` — it is `Default` and not `Clone`) — resolves `SymId → &str`;
- the **`Arc<CompiledProgram>`** — owns the canonical **`Ir`**, the **`Index`**, and the
  precomputed **`canon_hash`**;
- a precomputed **`DeclId → canonical path`** (from `Index.by_path`).

The `ModuleStore` is dropped after the build — only `Ir` + `Interner` + `Index` are
retained (all three are required: names need the interner, canonical paths and the
canonical hash need the index). Entities and id objects hold `Arc<ModelInner>`, so a view
keeps its model alive (no dangling), and the raw `Ir` never leaves Rust.

## Pipeline

```
source ─▶ RealParser ─▶ ModuleStore(resolve+const+imports) ─▶ lower ─▶ Ir ─▶ coherence ─▶ Model
```

Both entry points drive the **production** `ModuleStore::compile` path (so `const` and
`@"import.hymeko"` includes are handled and it never panics):

- `hymeko.parse(source, filename="<memory>")` — a one-file `MemProvider`.
- `hymeko.load(path)` — a real-filesystem `StdFsProvider`, imports resolved recursively.

The heavy build runs with the **GIL released** (`Python::allow_threads`): `ModuleStore`,
`Interner`, `Ir` and `CompiledProgram` are all `Send`/Ungil (the interner is
`{FxHashMap<&'static str, SymId>, Vec<Box<str>>}` — heap-stable, no `Rc`/`RefCell`), so
the whole compile + coherence pass happens off-GIL and only the typed error (a `Send`
enum) or the finished bundle crosses back. `canonical_hash()` returns the precomputed
`canon_hash` (O(1)).

## Errors (never a panic across FFI)

`compile` returns `ModuleLoadError { Io | Parse(String) | Cycle }`. `Parse(msg)` is
stage-tagged by `compile` itself, so the binding classifies by the exact prefix
(`module_store.rs`): `const:` / `index ` / `using alias` → `HyMeKoResolutionError`,
`lower failed` → `HyMeKoLoweringError`, `dep missing` / `root missing` → `HyMeKoIoError`,
anything else (the raw parser message) → `HyMeKoSyntaxError`. `Io`/`Cycle` →
`HyMeKoIoError`; coherence findings → `HyMeKoValidationError`; id misuse →
`HyMeKoIdError`. All derive from `HyMeKoError`. (This classification is a documented
heuristic over `compile`'s fused error channel; a future core change exposing staged
error types would sharpen it — the *types* are already the right set.)

## ID model & migration

`DeclId` / `NodeId` / `EdgeId` / `HyperArcId` are four distinct frozen Python types.
Each carries a **model-instance token** (a process-unique `AtomicU64`). Safety:

- **wrong domain** (e.g. a `NodeId` where a `DeclId` is expected) → `TypeError` at the
  PyO3 boundary (distinct types);
- **cross-model** (an id from another `Model`) → `HyMeKoIdError` (token mismatch);
- **out-of-range** → `HyMeKoIdError` (checked against the model's counts; not reachable
  from Python today since ids are only minted by the model and are never mutated).

The raw `usize` is **not** a public contract (no value getter; it appears only in
`repr`). Ids are **not** stable across pickling, snapshots, or transactions. **Migration
to generational canonical ids** replaces the internal `(raw, token)` with a
`(generation, index, token)` triple behind the *same opaque type* — no public-surface
change; equality/hash and the cross-model guard are unaffected. A stable public id and a
snapshot-local compact coordinate stay separate concepts.

## Incidences & the sparse/star direction

Incidences are read **directly from the canonical `Ir`** (`ArcRec.refs`), forward and
arc-major — no reverse index. `incidences_of_arc(arc)` is O(arity);
`incidences_of_node(node)` is a query over the Ir (O(arcs·arity), deterministic);
`incidence_arrays()` is the arc-major sparse SoA batch export.

Core `main` has **no** reverse-incidence index (`incidence.rs` / `PreparedIr` live only on
the unpromoted `feat/core-canonical-incidence-backbone` branch). Per PY0 §7 the binding
does **not** reimplement it. When that work promotes, the future integration point is:

```python
model.prepare_incidence_index()   # O(nnz) build, once
model.incident_arcs(node_id)      # O(deg) reverse lookup
model.star_operator()             # O(1) borrowed A_star = BᵀB handle (Bx / Bᵀy / BᵀBx)
```

No materialized star or clique graph is ever built.

## Batch representation study (§6)

Three options for `incidence_arrays()`:

- **A** — Python `list`/`tuple`: one Python int per element; ~16× slower and allocates
  per incidence. Rejected (baseline only).
- **B (chosen)** — one bulk `memcpy` per array into an immutable `PyBytes`, exposed as a
  typed `memoryview`. No per-incidence Python object; safe; no parent-retention needed
  (the `PyBytes` owns the copy). Measured overhead ≈ **1.40×** the Rust-only SoA build at
  5·10⁵ incidences — that factor *is* the single bulk copy (~10.5 MB), decomposed. A
  single pass over the (pointer-chasing) `SignedRefR` structs dominates; five
  `PyBytes::new_with` passes (no intermediate `Vec`) measured *slower* (5× traversal), so
  the single pass is kept (a §6.5#18 result).
- **C** — true zero-copy `memoryview` over the Rust `Vec` via the buffer protocol with
  parent-retention: deferred (would need more machinery; no undocumented `unsafe`).

## Toolchain

pyo3 0.23.5 (abi3-py312, offline-cached), maturin 1.15, CPython 3.12.13 — all
**project-local** via `uv` (no global toolchain install). One `abi3` wheel covers
CPython ≥ 3.12.

## Future steps

Mutable transaction API · the canonical incidence/star view (when the core work promotes)
· optional NumPy / WGPU integration (zero-copy arrays, GPU operators).
