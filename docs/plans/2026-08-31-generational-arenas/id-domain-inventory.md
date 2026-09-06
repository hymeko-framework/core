# ID-domain inventory (Stage 1.1)

Created-at: 2026-08-31 19:30 CEDT · branch `feat/core-generational-arenas` · base `67771c9`

Every identifier domain in `hymeko_core`, classified as *canonical entity*
(candidate for generational treatment) or *excluded* (stays a plain index).
Source of truth: `hymeko_core/src/common/ids.rs` + a repo-wide grep.

## Canonical entity IDs — in scope

All four are `Id<Tag>` aliases (`#[repr(transparent)]` over `usize`, sealed
`pub(crate)` field, `Id::new`/`raw` the only seam — the M2c sealing was done
explicitly to prepare this phase).

| Domain | Alias | Indexes | `::new` sites (src) | type uses (src) |
|---|---|---|---|---|
| declaration | `DeclId` | `Ir::decl_nodes` | 21 | 185 |
| node record | `NodeId` | `Ir::nodes` | 5 | 32 |
| edge record | `EdgeId` | `Ir::edges` | 5 | 28 |
| hyper-arc record | `HyperArcId` | `Ir::arcs` | 3 | 17 |

These name records in append-only arenas whose handles could, in a future
mutable IR, outlive their entity — the case generational IDs exist to catch.

## Excluded domains — must NOT be generationalized

| Domain | Where | Why excluded |
|---|---|---|
| `SymId` (`Id<SymTag>`) | interner / string table | interned symbols are value-identity, deduplicated, never deleted; a symbol handle is a content key, not an arena slot. Shares `Id<T>` with the entity IDs, so a generational form must be a **separate type**, not a change to `Id<T>`. |
| interner indexes | `resolution::interner` | internal to the interner's own dedup table |
| `DenseNodeIndex` / `DenseEdgeIndex` | — | not distinct types in core; dense positions used by tensor/graph lowering, meaningful only within one pass |
| CSR row/column indexes | `tensor/representations` | structural offsets into a matrix, not entity handles; raw `usize`/`u32` |
| shader buffer indexes | WGPU boundary (non-core) | GPU-side offsets; out of scope entirely |

The excluded dense/CSR/shader indices are **not** wrapped in `Id<T>` today
(they are raw integers), so there is no accidental-generationalization risk —
the phantom-tag boundary already separates them.

## Key structural facts (bearing on the decision)

- **The Ir is strictly append-only.** No `remove`/`delete`/`free`/`retire` on
  the entity tables; no `swap_remove`/`drain`/`clear`/`truncate` on
  `decl_nodes`/`nodes`/`edges`/`arcs`. Grep-verified. Nothing can currently make
  a handle stale.
- **The canonical hash is already generation-independent** —
  `canonical_hash.rs` reads no `.raw()`/slot value; it hashes content and tree
  structure. So the "hash must not depend on slot/generation" invariant is free.
- **The hot lookup is a single load.** `Ir::node/edge/arc` are
  `self.<table>.get(id.raw())` with a comment (M2a.1) that even a redundant
  `is_none()` guard was measurably too expensive to keep. A generation check
  adds either a second array load or a fatter slot on this exact path.
- **SymId is the heaviest excluded domain** (168 type uses) and shares `Id<T>`;
  this forces a *separate* `GenId<T>` type for the four entity domains rather
  than a change to the shared `Id<T>`.
