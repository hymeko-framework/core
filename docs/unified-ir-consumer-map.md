# Unified IR — consumer map (Phase 0, Part D)

**Date:** 2026-08-28 · design/measurement only · companion to `reports/2026-08-28-unified-ir-phase0-audit.md`

Repository-by-repository dependency and consumption inventory for `hymeko_core::ir::Ir` and `hymeko_core::common::ids`. Every row was read, not inferred from naming.

---

## 1. Dependency edges

`hymeko_core` is a leaf: it depends on nothing in the workspace, and everything else depends on it. The graph is a star, one-way, resolved at compile time by `path` dependency.

```
                        ┌──────────────────┐
                        │   hymeko_core    │   (leaf; depends on no sibling)
                        │  ir::Ir, ids::Id │
                        └────────┬─────────┘
       ┌──────────┬──────────┬───┴────┬───────────┬──────────┬──────────┐
   hymeko_hnn  hymeko_wgpu  hymeko_  hymeko_    hymeko_    hotaru   hymeko-
   (core/)     (core/)      emitter  formats    pgraph               network
                            query    (transformation/)  (graph/)
                                 │
                            hymeko_python  ──► pyo3 (23 FFI sites)
```

`phase3-backup-20260825/` mirrors `core/` and is a backup tree, not a live consumer. It is excluded from every count.

**The edge direction is the whole problem.** No consumer can be migrated ahead of core, and core cannot be migrated without breaking every consumer in the same instant — unless an adapter exists (`docs/unified-ir-migration.md` §J.3).

---

## 2. What each repository consumes

| Repository | Crate(s) | Consumes | Coupling strength | Breaks on ID change? |
|---|---|---|---|---|
| **core** | `hymeko_core` | owner of `Ir` and `Id<T>` | — | it *is* the change |
| **core** | `hymeko_hnn` | `Ir` → `HyperGraphView::from_ir`; `DeclId` for node/edge decl vectors | **structural** — builds the primary dense derived view | yes, compile |
| **core** | `hymeko_wgpu` | GPU buffers built from HNN views, not from `Ir` directly | indirect | no |
| **transformation** | `hymeko_emitter` | `Ir` read (4 emitters) **and written** (`bridge::to_compiler_ir`) | **strongest — a second writer of core's invariants** | yes, compile *and* silently |
| **transformation** | `hymeko_formats` | `Ir` → `snapshot::NodeDto` (JSON DTO, `target_id: usize`) | **published wire format** (WASM + Python) | yes, wire |
| **transformation** | `hymeko_query` | `Ir` traversal, predicate evaluation, transforms | read-heavy | yes, compile |
| **graph** | `hymeko_pgraph` | P-graph structures, PGIP I/O | mostly own ID space | partial |
| **hymeko_python** | `hymeko_python` | pyo3 bindings; exposes IDs to Python | **published FFI (23 sites)** | yes, ABI |
| **network** | `hymeko-network` | network/topology structures | light | partial |
| **hotaru** | `hotaru` | downstream application | light | partial |
| **tensor** | `hymeko_clifford` | Clifford algebra; touches `Ir` barely (1 file) | minimal | unlikely |
| **vscode-plugin** | (Rust support crate) | `raw()`-heavy, no direct indexing | surface-only | low |
| **parser** | — | **zero ID references** | **none** | **no** |
| **proofs** | — | **zero ID references** | **none** | **no** |

### 2.1 The two good results

**`parser` consumes no ID type at all.** It produces an AST; lowering is the only boundary. The parser is therefore *immune* to the entire migration, and the lowering function is the single contract to hold stable. This is worth protecting deliberately — it is the reason the migration has a clean upstream edge.

**`proofs` likewise.** No coupling to change.

### 2.2 The three hard consumers

1. **`hymeko_emitter` — the only *writer* outside core.** `bridge::push_decl` re-implements the five-array lockstep (`decl_nodes`, `decl_to_node`, `decl_to_edge`, `decl_to_arc`, `decl_hash`) by hand. Core's `Ir::…resize` does the same thing independently. Two implementations of one unenforced invariant, in two repositories, that must change together.
2. **`hymeko_formats::snapshot` — a published wire format.** `ArcDto { sign: i8, target_id: usize, … }` is serialized to JSON and consumed by the browser demo and by Python. `target_id` is a raw `DeclId` index. Changing the ID representation changes this JSON. It is versionless.
3. **`hymeko_python` — a published ABI.** 23 pyo3/extern sites. Python holds IDs across calls, so ID stability is an API guarantee already relied on, undocumented.

---

## 3. Why `transformation` forces a coordinated migration

**Counts used here** (from `docs/unified-ir-id-inventory.md` §2, which is authoritative): **196** dense-index sites in total — `transformation` **127**, `hymeko_core` **52**, `graph` 7, `hymeko_hnn` 6, `network` 4, all others 0. Plus **23** pyo3/extern FFI sites in `hymeko_python` and **3** ID-shaped JSON DTO fields in `hymeko_formats`.

Four independent reasons; any one alone would suffice.

**(a) One-way compile-time dependency.** `transformation` does `use hymeko::common::ids::…`. Core cannot depend back. An ID change in core is an immediate compile break downstream — there is no intermediate state where both compile, absent an adapter.

**(b) It writes core's invariants.** See §2.2(1). A repository that only *reads* can lag a release. A repository that *writes an invariant* cannot: if core tightens the invariant and the emitter still writes the old shape, the emitter corrupts what core repairs.

**(c) The break is silent, not loud.** The 127 dense-index sites look like:

```rust
ir.decl_nodes[node.decl.0]     // emit_hymeko, bridge
ir.arcs[arc_id.0]              // bridge reverse walk
ir.decl_to_node[did.0] = Some(nid)   // bridge forward walk — a write
```

Under a stable-key scheme these **still compile** as long as `Id` keeps a `usize`-like surface — and defect **D4** (`pub .0`, `From<usize>`, `From<Id<T>> for usize`) guarantees exactly that surface. They would then index the wrong element.

> **A migration that leaves D4 in place converts a compile error into a wrong answer.** Removing `pub .0` and the `From` impls is a *safety precondition* of the migration, not hygiene. It is what forces the 127 sites to fail loudly.

**(d) Consumers beyond Rust.** A JSON DTO (browser) and a pyo3 ABI (Python) both surface raw ID integers. Those are contracts with external code that no Rust refactor can update.

---

## 4. Migration-order consequence

| Property | `core` only (M0, M2) | Cross-repository (M1, M3) |
|---|---|---|
| Compile blast radius | core | core + 6 crates in 4 repositories |
| Silent-breakage risk | none (invariants only tightened) | **high** until D4 is removed |
| External contracts touched | none | JSON DTO, pyo3 ABI |
| Revertable by one commit | yes | no — needs coordinated revert |

This is why `docs/unified-ir-migration.md` sequences **M0 → M2 → M1**, and why M1 opens with removing `From<usize>` — the step whose only purpose is to make the remaining work fail loudly.

---

## 5. Provenance

Counts derive from `.rs` files that actually reference `DeclId|NodeId|EdgeId|HyperArcId|SymId`, excluding `target/` and the `phase3-backup-20260825/` mirror. Per-category numbers and their exact commands are in `docs/unified-ir-id-inventory.md`. Dependency edges were read from each `Cargo.toml`, not inferred from crate names.

---

## M2a addendum — the migration mapping table (2026-08-28)

M2a added the storage-independent read surface M2b migrates onto. **Two of the requested capabilities already existed** and were not duplicated: `Ir::decl_node` already satisfies the entity-lookup contract, and `Ir::as_node`/`as_edge`/`as_arc` already satisfy declaration→record-id. The repository's term is `decl`, and the new surface keeps it.

| Old direct pattern | New API |
|---|---|
| `ir.decl_nodes[d.0]` | `ir.decl_node(d)` *(pre-existing)* |
| `ir.decl_nodes[d.0].kind` | `ir.kind_of(d)` |
| `ir.decl_nodes[d.0].parent` | `ir.parent_of(d)` |
| `ir.decl_nodes[d.0].first_child` / `.last_child` / `.next_sibling` | `ir.first_child_of(d)` / `last_child_of(d)` / `next_sibling_of(d)` |
| `ir.nodes[n.0]` / `ir.edges[e.0]` / `ir.arcs[a.0]` | `ir.node(n)` / `ir.edge(e)` / `ir.arc(a)` |
| `ir.decl_to_node[d.0]` (and `_edge` / `_arc`) | `ir.as_node(d)` / `as_edge(d)` / `as_arc(d)` *(pre-existing)* |
| map lookup **then** record lookup | `ir.node_for_decl(d)` / `edge_for_decl(d)` / `arc_for_decl(d)` |
| `ir.decl_nodes.iter()` / `.enumerate()` | `ir.decls()` — yields `(DeclId, &DeclNode)` |
| `ir.nodes.iter()` / `edges` / `arcs` | `ir.node_records()` / `edge_records()` / `arc_records()` |
| `ir.decl_nodes.len()` and siblings | `ir.decl_count()` / `node_count()` / `edge_count()` / `arc_count()` |
| `d.0 < ir.decl_nodes.len()` | `ir.contains_decl(d)` |
| `ir.decl_children(d)` | `ir.child_decls(d)` — same order, but total |

### Sites that cannot migrate in M2b

| Class | Reason |
|---|---|
| **Mutation** | M2a adds no mutable accessors, deliberately. Lowering writes, `push_anonymous_arc`, `preallocate_from_index`. |
| **Construction / lowering** | Builds the arrays before ids are meaningful — includes `bridge::push_decl` in `transformation`, the second lockstep writer. |
| **Bulk serialization** | Serde reads fields directly by derive. |
| **Performance-critical internal algorithm** | `canonical_hash.rs`, `hash_pass.rs` — already inside the boundary; M2a measured the accessor cost at +20…49 % on scalar lookup, so these may keep indexing. |
| **Malformed-state tests** | Construct incoherence on purpose. |

**Nothing in the read paths of the 196-site inventory needs an API M2a did not provide.** M2b can proceed repository by repository without inventing further read APIs.
