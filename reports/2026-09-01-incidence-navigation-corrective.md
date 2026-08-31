# Incidence navigation — corrective (exact coordinates, honest star benchmark)

**APPROVED-CORE-EDIT: canonical-ir-incidence-navigation** (corrective pass)
2026-09-01 · branch `feat/core-incidence-navigation` · base `3190097`

Supersedes the Stage-3 benchmark of `reports/2026-09-01-incidence-navigation.md`
(marked with a corrective erratum, not deleted).

## 1. Corrected terminology and units

- **Time unit.** The prototype printed `elapsed * 1e6` — **microseconds** — but
  labelled it "ns". All benchmark numbers here are **µs**, produced by a single
  typed helper `median_us` that always returns microseconds.
- **Control naming.** The old "DENSE" control was a **clique / 2-section
  expansion** (`O(Σ k²)` legs, connecting a hyperedge's nodes pairwise), **not** a
  true O(nnz) sparse star. The corrective distinguishes them (§6).
- **Sparse star is O(nnz).** The signed incidence operator `B` and its transpose
  are O(nnz); only the clique 2-section is O(Σ k²).

## 2. Incidence coordinate (`(arc, slot)`) API

The reverse index now stores the **exact incidence coordinate**, SoA to avoid the
`(HyperArcId,u32)` tuple pad:

```
row:   Vec<u32>          // DeclId -> range
arcs:  Vec<HyperArcId>   // coordinate.arc
slots: Vec<u32>          // coordinate.ref_slot   (checked: slot >= 2^32 -> SlotOverflow)
```

From a node-major query, one incidence resolves in **O(1)** to its `ArcRec`,
`SignedRefR`, target, sign, weights, annotation, `HyperArcId` and ref slot —
`IncidenceIndex::incidences(node) -> impl Iterator<Item = IncidenceRef<'ir>>` and
`resolve(arc, slot) -> IncidenceRef<'ir>`, no arc rescan. `incident_arcs` /
`incident_slots` give the raw parallel slices.

## 3. Lifetime / provenance guarantee

`IncidenceIndex<'ir>` **borrows the `Ir` it was built from**. Consequences: it
cannot be paired with a different `Ir`; the `Ir` cannot be mutated while the index
lives (borrow-checked); the methods hand back `IncidenceRef<'ir>` directly; and the
error-prone separate `&Ir + &IncidenceIndex` argument pair is gone. **No `unsafe`,
no pointer identity.**

## 4. Exact coordinate-bijection validator

`validate() -> Result<(), CoordError>` proves a **bijection**: it marks each
`(arc, slot)` by its canonical position `arc_base[arc] + slot` in a `0..nnz`
seen-array, so a **duplicated coordinate** is caught on the second mark and a
**missing** one by the final sweep; each coordinate must be in range and sit in
its true target's reverse row. The old `degree + any(target)` check could **not**
catch a coordinate swap that duplicates one incidence and drops another with the
same target — that hole is now closed. Typed variants: `RowLength`,
`CoordOutOfRange`, `TargetMismatch`, `DuplicateCoordinate`, `NotABijection`.

**Fault mutants** (lib unit tests, corrupting the private coordinate arrays):
duplicated coordinate → `DuplicateCoordinate`; a swap keeping node degree →
`TargetMismatch`; out-of-range slot → `CoordOutOfRange`; truncated row →
`RowLength`.

## 5. The sparse star operator (no materialization)

`B[arc, target]` — the ragged signed sparse operator; `ArcRec.refs` is its row
orientation, the reverse index the `Bᵀ` access permutation. Allocation-free,
caller-owned output/scratch:

- `node_to_arc(x, y)` = `Bx` — arc-major, output-partitioned by arc.
- `arc_to_node(y, z)` = `Bᵀy` — **node-major via the reverse index**,
  output-partitioned by node (so serial ≡ parallel exactly; no scatter contention).
- `star_operator(x, z, y_scratch)` = `BᵀBx`.

**Semantic policy (explicit):** sign `+/-/~` → `+1/-1/0`; `scalar_weight` = no
weights → `1.0`, else the **product** of the `ValueR::Num` components (a `List`
flattened one level; non-numeric ignored; all-non-numeric → `1.0`); duplicate
incidences each contribute; FP accumulation is refs-left-to-right per arc and
coordinate order per node. Correctness is pinned against an **independent dense-B
matrix-vector oracle** for `Bx`, `Bᵀy`, `BᵀBx` (incl. a duplicate incidence).

## 6. Corrected benchmark (real production struct, µs, honest controls)

Real `IncidenceIndex` on a synthetic `Ir`; **`size_of::<HyperArcId>() = 8`**;
one typed µs helper; clique build is **complete** (count + fill). nnz = 1e6.

| workload | index build | bytes/inc | Bx | Bᵀy | BᵀBx | clique build | clique bytes/inc |
|---|--:|--:|--:|--:|--:|--:|--:|
| arity 5 (200k arcs, 500k nodes) | 139 ms | **14.8** | 24.8 ms | 243 ms | 314 ms | 193 ms (1.4×) | 18.0 (**1.2×**) |
| arity 20 (50k arcs, 50k nodes) | 70 ms | **12.4** | 15.5 ms | 168 ms | 207 ms | 480 ms (6.8×) | 76.2 (**6.1×**) |

**Honest reading:**
- **Memory (retracted 2–13× → corrected):** real coordinate index is **12.4–14.8
  B/incidence** (u32 row + 8-byte `HyperArcId` + u32 slot). The clique 2-section is
  **1.2×** the index memory at arity 5 but **6.1×** at arity 20 — the O(Σ k²)
  blow-up is **arity-dependent**, not the flat 13× the u32 prototype implied.
- **Build:** the clique 2-section costs **1.4×** (arity 5) to **6.8×** (arity 20)
  the index build.
- **Operator:** `Bᵀy` (168–243 ms) is ~**10× `Bx`** (15–25 ms). This is honest:
  `Bᵀy` resolves each incidence's sign/weight by chasing into the scattered
  `ArcRec.refs`, so it is memory-scatter-bound. A signed-value precompute (an
  operator-side `Vec<f64>` alongside the coordinates) would remove the chase — a
  documented follow-up, not done here (it is a derived cache, not part of the
  lossless coordinate index).
- **Persistent bytes/incidence** = the index (12.4–14.8). **Peak temporary** during
  build = index arrays + a `cur` cursor (`row.len()` u32) ≈ +2–3 B/incidence.

## 7. Production memory (measured on the real struct)

`retained_bytes()` sums `row.capacity()*4 + arcs.capacity()*8 + slots.capacity()*4`
— i.e. real `HyperArcId` (8 B), not a u32 prototype. At nnz 1e6: 14.8 MB (arity 5,
700k decls) / 12.4 MB (arity 20, 100k decls). Bytes/declaration = `row` term
(~4 B/decl). Allocation count: 3 arrays + 1 build cursor.

## 8. Tests / coverage

New/updated: coordinate golden shapes (`+/-/~` resolves sign per slot; duplicate
→ distinct slots; ragged/shared/singleton; empty), the independent
exact-coordinate scan oracle, the weight-policy cases (None / single / multi-valued
product / non-numeric), the independent dense-B operator oracle (`Bx`/`Bᵀy`/`BᵀBx`),
the dangling-target fault, and the four validator fault-mutants. **117 lib + 210
integration + 7 doctests green; fmt + clippy `-D warnings` clean.**

## 9. Own mistakes / retracted claims

1. Benchmark printed µs, labelled "ns" — retracted, single typed µs helper added.
2. The "DENSE" control was a clique/2-section (O(Σ k²)), not a sparse star —
   renamed and the blow-up shown to be arity-dependent (1.2× → 6.1×), not a flat
   13×.
3. Prototype used u32 arc ids — real `HyperArcId` is 8 B; memory re-measured on the
   production struct (12.4–14.8 B/incidence).
4. The first validator (`degree + any(target)`) was not a bijection check — it
   missed a degree-preserving coordinate swap; replaced by the seen-array bijection
   validator with a fault-mutant for exactly that case.

## 10. Decision — ready for the graph direct-Ir phase?

**Yes.** All corrective gates are green: `(arc, slot)` coordinate API with O(1)
resolution; borrowed-`Ir` provenance; exact bijection validator + fault-mutants;
the allocation-free `Bx`/`Bᵀy`/`BᵀBx` operator with an explicit semantic policy and
an independent oracle; honest, corrected, unit-consistent benchmark on the real
struct. The `hymeko_graph` **`graph-direct-ir-algorithms`** phase can start on this
API. One non-blocking follow-up recorded: a signed-value operator precompute to
remove the `Bᵀ` arc-scatter, and the transaction-updated stored index for the
future mutable IR.

## Provenance

Branch `feat/core-incidence-navigation` @ base `3190097`; rustc 1.97.1,
x86_64-pc-windows-msvc. Benchmark is single-host; a quiet-host / Amaterasu second
run is advisable before publishing absolute numbers.
