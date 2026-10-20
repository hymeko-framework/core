# hymeko_core

The HyMeKo semantic core — a standalone cargo workspace.

Checked value updates retain only the current graph's referenced symbols.
Canonical CBOR export and admitted decode use the same symbol compaction;
superseded timestamp/text values do not accumulate in the current snapshot.
Declaration IDs, graph semantics and canonical digests are preserved. Symbol IDs
are local to a snapshot and may be renumbered: pair them with that snapshot's
interner/index. Explicitly retained old snapshots remain unchanged and readable.
See [the symbol-lifetime plan](docs/plans/2026-09-25-symbol-lifetime/plan.pdf).

Repeated checked value updates can reuse caller-owned scratch. Create it once
with `ValueUpdateWorkspace::with_capacity(256, 16_384)?`, then pass it to updates:

```rust
use hymeko::module_store::execution::{
    CompiledModel, ValueUpdate, ValueUpdateError, ValueUpdateWorkspace,
};

fn update(
    model: &CompiledModel,
    changes: &[ValueUpdate<'_>],
    scratch: &mut ValueUpdateWorkspace,
) -> Result<CompiledModel, ValueUpdateError> {
    model.with_value_updates_in(changes, scratch)
}
```

The capacities bound edits and the old-plus-new string occurrences in edited
values, including repetitions. Overflow returns `LimitExceeded`; scratch contents
are cleared on success or error while its buffers retain their capacity. Existing
4096-edit, value-node, depth and text limits still apply. The original
`with_value_updates` method uses the same implementation with temporary scratch.
Snapshot construction, nested-value traversal, validation and symbol compaction
may still allocate. These APIs do not promise a hard real-time deadline.

Repeated value edits reuse structure-validation evidence bound to their immutable
model and an inverse symbol-to-record index. A compaction rewrites affected record
groups; unchanged name paths, imports and indexes stay shared. The inverse index
has an initial construction cost and may still be copied linearly on text edits.

`model.with_declaration_pages(256)?` opts into page-level declaration copying.
Page sizes must be nonzero powers of two. Conversion costs O(N); subsequent
indexed writes copy the directory and touched pages. Normal indexed reads and
iteration stream pages. Explicit contiguous Vec/slice access may cache a full
copy; mutable Vec operations return to flat storage. Compile/decode keep the
contiguous default. Canonical encoding, hashes and snapshot isolation are unchanged.

For controlled publication, create `SnapshotRetirement::with_capacity(limit)?`
and call `ExecutionState::commit_retaining(candidate, &mut retirement)`. A full
queue returns the candidate for retry. `retirement.reclaim(max_checks)` checks
only that many retired roots, rotating roots still held by readers. Reclamation
runs outside the publication lock and can run on a caller-owned worker. Capacity
bounds roots, not bytes; a root's destructor can still do substantial work.

- **`hymeko_core/`** — the **IR** (`ir/`), **encoding**/serialization (`writers/` — CBOR + IR writers), and canonical IR-to-tensor adapters (`tensor/`: reference-weight extraction and `TensorInc`).
- **`hymeko_wgpu/`**: WGPU compute backend, consuming `hymeko_tensor` directly.

The `hymeko_hnn` implementation lives in `hymeko_nagare::hnn` (the Nagare repository). It owns the **hypergraph representation** (`traversal/`: `HyperGraphView`, incidence views) and the **tensor-transformation** operators (`tensor/`: HGNN / signed-HGNN / message-passing).

`parser` is an external repository (`hymeko/parser`), pulled as a `git` dependency.

## Build & test

`generators::GraphGenerator` is the representation-independent generation contract;
the existing `HypergraphGenerator` implements it. Seeded implementations also use
`SyntheticGenerator` to expose explicit seed/reseed control. Graph-specific output
types and generators remain in their owning packages.

Programmatic root construction uses `ir::ir::IrConstruction::new(&mut ir)` and
`append_node` / `append_edge`. These extend the same construction owner used by
lowering, keep declaration and record IDs distinct, and reject inconsistent
dispatch-table lengths without partial mutation. The caller supplies resolved
names, annotations and bases; this does not parse or resolve a source model.
`DeclNode::root`, `RefAtomR::plain(...).with_weights(...)` and `ArcRec::unowned`
centralize raw-record defaults. An unowned arc does not create an owner,
declaration or dispatch binding.

Canonical declaration and arc records now carry model-local `AnnotationId`s;
node and edge records carry `BaseListId`s. Read logical metadata through
`ir.decl_annotation(decl)`, `ir.annotation(record.anno)` and
`ir.bases(record.bases)`. NONE denotes empty data without a payload entry.
Nonempty base lists are exact boxed slices; their ID plus slice header costs
no more than the former Vec header. Dense nonempty annotations still pay an
additional ID; this layout primarily benefits sparse metadata.

`DeclNode<AnnoR>` is owned construction input accepted by `IrConstruction` and
raw `push_decl`; `DeclNode` in canonical tables stores the annotation ID.
`ir.decl_draft(decl)` explicitly clones logical metadata for a construction input.
Serialize the owning `Ir`; compact declaration handles cannot be serialized as
logical annotations on their own. Raw declaration edits use
`set_decl_annotation` / `decl_annotation_mut` and must preserve exclusive
nonempty annotation slots. Coherence admission rejects slot aliasing with
other declarations, arcs or reference payloads. Checked compiled-model updates
reuse existing slots without retaining superseded values; declaration records
remain shared when only an existing annotation changes. Declaration-page
snapshots also page annotation storage. Logical wire bytes and identities are
unchanged, and old snapshots retain their original values.

`ir::incidence::IncidenceProjection` prepares numeric arc rows in canonical
declaration or node coordinates. An existing incidence view exposes
`prepare_projection(IncidenceDomain::Nodes)` (or `Declarations`); the reverse
index lazily caches each domain, and projection clones share its buffers.
`IncidenceProjection::build(&ir, domain)` uses the same preparation without
requiring a reverse index. Invalid node bindings and out-of-domain references
are rejected once; the immutable IR borrow protects subsequent use.

`IncidenceRow::gather` / `scatter` are common matrix-free operations with an
`IncidenceAccumulator` arithmetic policy. The existing `IncidenceOperatorPlan`
also uses these rows for its lazily prepared forward apply. Its transpose stays
node-major and preserves serial/parallel arithmetic order. Transpose-only plans
keep one coefficient per incidence; first forward use adds forward coefficients
and shared coordinates. The structural cache never contains model-specific
weights, and no matrix product or pairwise graph is constructed.

Canonical conversion from `ReferenceDraft` compacts the resulting reference
vector before retaining it. A 32-byte record therefore does not retain the
96-byte draft buffer's spare capacity. This may resize or move the allocation
once during construction; warm operator calls do not perform compaction.

`util::measurement` owns `MeasurementRunner`, `Runner`, `Clock`, `Distribution`,
`CountingAllocator` and `AllocationSnapshot`. A runner reserves reusable sample
storage at construction, retains chronological samples, and summarizes a scratch
copy through in-place order-statistic selection. Clocks are statically dispatched;
deterministic test clocks verify scheduling without running benchmarks. Timings are
microseconds; sample count and warmup policy are explicit. Constructing or importing
the types never starts a measurement or installs an allocator.

The optional counting allocator records allocation requests/bytes on the calling
thread, including failed requests; it does not report retained memory, worker-thread
totals or RSS. The library never sets a global allocator for its consumers.

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
