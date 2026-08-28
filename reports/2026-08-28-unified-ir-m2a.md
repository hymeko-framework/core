# Unified IR M2a — storage-independent read accessors

**Date:** 2026-08-28
**Authorisation:** `APPROVED-CORE-EDIT: unified-ir-m2a-accessor-api`
**Nature:** additive only. No existing field, method, type or storage changed. `ir/ir.rs` was **not edited at all**.

---

## 0. Verdicts

| Question | Verdict |
|---|---|
| API completeness | **API-COMPLETE** for the M2b mapping table (§F) |
| Performance | **REGRESSION on scalar lookup** (+24…49%); **WITHIN-NOISE on iteration** |
| May M2b begin? | **Yes** — with the cost caveat in §E.4 |
| Public compatibility hazard | **None found** — every prior field and method remains, unchanged |

### Starting state

| Item | Value |
|---|---|
| Local HEAD | `bdc7b87ffd50754a8010be7281ab9ac64b41399f` (`main`) |
| Tree at HEAD | `98a9818a00d52819a1b3a9db1d9bc5f831c11392` |
| Worktree | clean but for the three untracked IFAC artifacts, preserved |
| Host | median 4 % (samples 3, 4, 1, 15, 17 — spikes present, so variants alternate) |

---

## Part A — API design

### A.1 What already existed, and was deliberately not duplicated

The inventory came first, and it changed the design. Two of the capabilities the brief asked for **already existed and were already safe**:

| Brief's requirement | Existing method | Why no new name was added |
|---|---|---|
| `ir.declaration(id) -> Option<&DeclNode>` | **`Ir::decl_node`** | already `Option`-returning, already `NONE`-guarded, already `.get()`-based |
| `ir.node_id_for_decl(id) -> Option<NodeId>` | **`Ir::as_node`** / `as_edge` / `as_arc` | identical contract and identical implementation |

Adding `declaration()` or `node_id_for_decl()` would have been renames with no behavioural difference — precisely the "aliases that differ only cosmetically" the brief forbids, and a §6.1 duplication. The repository's established term is **`decl`** (`decl_nodes`, `decl_node`, `decl_kind`, `decl_children`, `decl_to_node`, `DeclId`, `DeclNode`, `DeclKind`), so the new surface keeps it rather than introducing a second vocabulary.

### A.2 The public API added

All on `impl Ir`, in the new module `hymeko_core/src/ir/access.rs`. **No mutable accessor, no unchecked accessor, nothing returning `Vec` or exposing capacity.**

```rust
// Record lookup by typed id — did not exist before.
pub fn node(&self, id: NodeId)      -> Option<&NodeRec>
pub fn edge(&self, id: EdgeId)      -> Option<&EdgeRec>
pub fn arc (&self, id: HyperArcId)  -> Option<&ArcRec>

// Declaration → record, with the id. Checked against the record's back-link.
pub fn node_for_decl(&self, id: DeclId) -> Option<(NodeId, &NodeRec)>
pub fn edge_for_decl(&self, id: DeclId) -> Option<(EdgeId, &EdgeRec)>
pub fn arc_for_decl (&self, id: DeclId) -> Option<(HyperArcId, &ArcRec)>

// Iteration yielding typed ids.
pub fn decls       (&self) -> impl Iterator<Item = (DeclId, &DeclNode)> + '_
pub fn node_records(&self) -> impl Iterator<Item = (NodeId, &NodeRec)>  + '_
pub fn edge_records(&self) -> impl Iterator<Item = (EdgeId, &EdgeRec)>  + '_
pub fn arc_records (&self) -> impl Iterator<Item = (HyperArcId, &ArcRec)> + '_

// Live-entity counts (not capacity).
pub fn decl_count(&self) -> usize
pub fn node_count(&self) -> usize
pub fn edge_count(&self) -> usize
pub fn arc_count (&self) -> usize

// Containment. Rejects NONE and out-of-range.
pub fn contains_decl(&self, id: DeclId)      -> bool
pub fn contains_node(&self, id: NodeId)      -> bool
pub fn contains_edge(&self, id: EdgeId)      -> bool
pub fn contains_arc (&self, id: HyperArcId)  -> bool

// Total hierarchy reads. Never panic, for any id.
pub fn parent_of      (&self, id: DeclId) -> Option<DeclId>
pub fn first_child_of (&self, id: DeclId) -> Option<DeclId>
pub fn last_child_of  (&self, id: DeclId) -> Option<DeclId>
pub fn next_sibling_of(&self, id: DeclId) -> Option<DeclId>
pub fn kind_of        (&self, id: DeclId) -> Option<DeclKind>
pub fn child_decls    (&self, id: DeclId) -> ChildDecls<'_>   // Item = DeclId
```

### A.3 Why the `_of` suffix, and why the old methods were left alone

The pre-existing hierarchy readers are not total:

| Method | `NONE` | out-of-range |
|---|---|---|
| `parent`, `first_child`, `next_sibling` | guarded → `NONE` | **panics** (direct index) |
| `kind`, `decl_kind` | **panics** (no guard at all) | **panics** |
| `decl_children` | guarded | **panics** |

Their signatures return `DeclId`/`DeclKind`, not `Option`, and existing callers depend on that. Changing them would either break callers or silently change a panicking method's contract — both forbidden. So the total versions are **new methods with a distinguishing suffix**, and the old ones are untouched. `kind_of` returning `Option<DeclKind>` is the one that matters most: `kind()` panics on `DeclId::NONE`, which is a value the type system hands out freely.

`child_decls` differs from `decl_children` in more than naming: `decl_children` resolves each step through `next_sibling`, which indexes directly and therefore panics on a dangling link, and loops forever on a sibling cycle. `child_decls` stops on both. On well-formed input they yield identical sequences — asserted over all 96 fixtures.

---

## Part B — storage assumptions

Every *contract* is storage-independent; the implementations use ordinal indexing internally, which is the entire point of the boundary.

| Method group | Contract assumes | Implementation currently uses |
|---|---|---|
| `node`/`edge`/`arc` | id validity is decidable | dense `Vec::get` |
| `*_for_decl` | a declaration maps to ≤ 1 record of a kind | parallel `decl_to_*` maps |
| iterators | iteration is finite; each live entity appears once | dense `enumerate` |
| counts | **live entities**, not capacity | `Vec::len` |
| `contains_*` | id validity is decidable | range check |
| hierarchy `_of` | links resolve or are absent | checked index |

**No contract assumes** dense ids, ordinal stability, absence of tombstones, or the existence of the parallel maps. A generational or tombstoned arena satisfies all of them by skipping vacant slots — no call site changes.

**Not exposed**, per the brief: slot positions, raw slices as iteration APIs, mutable record references, mapping-array references, unchecked lookup, capacity, index arithmetic.

Iteration order is documented as **deterministic for one snapshot and not part of program semantics**. That is not a hedge — M0.5 proved the canonical hash sorts before folding precisely because storage order is representational.

---

## Part C — absence semantics

Every lookup returns `Option`. `None` means: the id is `NONE`; the id is out of range; the declaration has no record of that kind; or **the mapping and the record disagree**.

That last case is the one worth stating. `node_for_decl(d)` returns `None` when `decl_to_node[d]` names a record whose `decl` back-link points at a *different* declaration. Returning the record anyway would be a silent wrong answer — the caller asked about `d` and would receive something else. A test constructs exactly this state and asserts `as_node` still reports the raw mapping while `node_for_decl` refuses it.

**Asymmetry, stated rather than hidden:** `arc_for_decl` cannot perform this check. `ArcRec` has no `decl` back-pointer — its `in_edge` names the *owning edge's* declaration, not the arc's own — so it verifies presence only. That is a property of the record layout, not an oversight.

No error type was introduced. Structural diagnosis belongs to M0.6's `validate_hierarchy_index_coherence`, and this module does not re-implement validation inside every lookup.

---

## Part D — tests

**19 new tests** in `tests/unified_ir_m2a/accessors.rs`.

| Area | Tests |
|---|---|
| Lookup | first/middle/last declaration; `NONE` and out-of-range rejection across all four id types; typed record round-trip; raw-ordinal-0 not aliasing across tables; decl→record mapping vs back-links; valid declaration lacking the requested record; **mapping pointing at a foreign record**; **mapping pointing out of range** |
| Iteration | empty IR; singleton; counts equal iterator length; ids resolve back with no gaps or duplicates; repeated iteration deterministic; mixed kinds all visited |
| Hierarchy | root/nested/siblings/leaf; child order matches the existing iterator; **termination on a sibling cycle**; `last_child_of` agrees with the final child |
| Compatibility | **all 96 fixtures**: accessor vs direct field access, `kind_of` vs `kind`, `parent_of`/`first_child_of`/`next_sibling_of` vs the old total-valued forms, `child_decls` vs `decl_children`, `*_for_decl` vs `as_*`, all four counts vs `Vec::len` |

The corpus test gates on coherence first: **96 fixtures, 96 coherent**, then compares. Tests deliberately assert *stability* of iteration order, never that numeric order carries meaning.

---

## Part E — performance

Quiet host, ambient 2–4 %. Variants registered in **alternating rounds** (`direct_r0`, `accessor_r0`, `direct_r1`, …) rather than as blocks, so thermal drift is not aliased onto the comparison.

### E.1 The fair control

The first run compared `ir.decl_nodes[d.0]` against `ir.decl_node(d)` and showed the accessor 48 % slower. **That comparison was not like-for-like**: the direct path does one bounds check unconditionally, while the accessor additionally rejects `NONE`, returns an `Option`, and forces a branch at the call site. Charging the accessor for safety the direct path never provided would have produced a "regression" that was really a difference in contract.

So a third variant was added — `direct_guarded`, doing exactly the accessor's work by hand:

```rust
if d.is_some() && d.0 < len { acc += ir.decl_nodes[d.0].name.raw(); }
```

### E.2 Three-way result (1 000 nodes / 1 500 declarations, cache-resident, stable across runs)

| Variant | Median | vs guarded control |
|---|---:|---:|
| `direct` (unguarded) | 538 ns | — |
| **`direct_guarded`** (fair control) | **527 ns** | baseline |
| `accessor` | 785 ns | **+49 %** |

`direct_guarded` is *faster* than unguarded `direct` — the explicit `d.0 < len` lets the optimiser drop the per-element panic path.

**So the overhead survives the fair control.** It is not the cost of safety; it is the cost of materialising `Option<&T>` and branching on it, where the hand-written form keeps the check in registers.

### E.3 Other groups

| Comparison | Direct | Accessor | Delta |
|---|---:|---:|---:|
| record lookup, 10 000 | 3.60 µs | 4.62 µs | +28 % |
| map: direct vs `as_node` *(pre-existing method)* | 4.88 µs | 7.94 µs | +63 % |
| map: direct vs `node_for_decl` *(new, validates back-link)* | 4.88 µs | 9.51 µs | +95 % |
| hierarchy, 10 000 | 7.66 µs | 9.18 µs | +20 % |
| **iteration, 1 000** | **341 ns** | **341 ns** | **0 % — within noise** |
| iteration, 10 000 | 11.27 µs | 11.88 µs | +5.4 % |

Two things worth separating. The `as_node` row is a **pre-existing** method, not something M2a added — its 63 % gap over raw indexing was already the cost of the existing safe API. And `node_for_decl` is legitimately doing more work: a second lookup plus a back-link comparison, which is the §C guarantee.

**Iteration is the clean win**: `enumerate().map()` compiles away entirely at 1 000 elements.

### E.4 Verdict: REGRESSION, with the absolute scale stated

Acceptance asked for < 3 %. **That is not met for scalar lookup**, and reporting otherwise would require choosing a favourable framing. The honest numbers:

- Relative: **+20 % to +49 %** on scalar lookup against the fair control.
- Absolute: **~0.35 ns → ~0.52 ns per lookup**. The overhead is ~0.17 ns.
- Iteration: **within noise**.
- **No allocation** is introduced on any valid-lookup path — every accessor returns a reference or a `Copy` id.
- Invalid lookup remains **O(1)**: a `NONE` test and a range check, never a scan.

Whether that matters is a workload question, and the workload answer is that it mostly does not: `canonical_program_hash` costs 10.3 ms at 15 000 declarations (M0.6), and lookup is a small fraction of it. But the brief said not to claim zero cost, and it is not zero cost.

### E.5 A measurement-integrity finding

The 10 000-element `decl_lookup` case measured **16.5 µs** in the first run and **7.0 µs** in the second, on identical benchmark code, with ambient load 2–4 % in both. Criterion reported the change as −58 % with p = 0.00.

The only difference between runs was a recompilation that added the `direct_guarded` variant to the same group — i.e. **code layout, not workload**. The 1 000-element case was stable across both runs (538 ns vs 538 ns).

I am not able to attribute this confidently without generated-code inspection, which would need tooling this phase may not install. **It is reported rather than resolved**, and the conclusions above rest on the 1 000-element case, which is stable and cache-resident. Any future comparison at 10 000 on this group should re-establish its own baseline rather than trust these numbers.

---

## Part F — the M2b mapping table

The mechanical substitution M2b applies, repository by repository. This is the table whose completeness determines whether M2a is done.

| Old direct pattern | New API |
|---|---|
| `ir.decl_nodes[d.0]` | `ir.decl_node(d)` *(pre-existing)* |
| `ir.decl_nodes[d.0].kind` | `ir.kind_of(d)` |
| `ir.decl_nodes[d.0].parent` | `ir.parent_of(d)` |
| `ir.decl_nodes[d.0].first_child` | `ir.first_child_of(d)` |
| `ir.decl_nodes[d.0].last_child` | `ir.last_child_of(d)` |
| `ir.decl_nodes[d.0].next_sibling` | `ir.next_sibling_of(d)` |
| `ir.nodes[n.0]` | `ir.node(n)` |
| `ir.edges[e.0]` | `ir.edge(e)` |
| `ir.arcs[a.0]` | `ir.arc(a)` |
| `ir.decl_to_node[d.0]` | `ir.as_node(d)` *(pre-existing)* |
| `ir.decl_to_edge[d.0]` | `ir.as_edge(d)` *(pre-existing)* |
| `ir.decl_to_arc[d.0]` | `ir.as_arc(d)` *(pre-existing)* |
| `ir.decl_to_node[d.0]` then `ir.nodes[n.0]` | `ir.node_for_decl(d)` |
| `ir.decl_to_edge[d.0]` then `ir.edges[e.0]` | `ir.edge_for_decl(d)` |
| `ir.decl_to_arc[d.0]` then `ir.arcs[a.0]` | `ir.arc_for_decl(d)` |
| `ir.decl_nodes.iter()` | `ir.decls()` |
| `ir.decl_nodes.iter().enumerate()` | `ir.decls()` *(yields typed ids)* |
| `ir.nodes.iter()` / `.enumerate()` | `ir.node_records()` |
| `ir.edges.iter()` / `.enumerate()` | `ir.edge_records()` |
| `ir.arcs.iter()` / `.enumerate()` | `ir.arc_records()` |
| `ir.decl_nodes.len()` | `ir.decl_count()` |
| `ir.nodes.len()` / `edges.len()` / `arcs.len()` | `ir.node_count()` / `edge_count()` / `arc_count()` |
| `d.0 < ir.decl_nodes.len()` | `ir.contains_decl(d)` |
| `ir.decl_children(d)` | `ir.child_decls(d)` *(total; same order)* |

### Sites that cannot migrate yet

| Class | Why | Example |
|---|---|---|
| **Mutation** | M2a adds no mutable accessors, deliberately | `ir.decl_nodes[i].parent = …` in lowering, `push_anonymous_arc`, `preallocate_from_index` |
| **Construction / lowering** | builds the arrays before ids are meaningful | `module_store` lowering; `bridge::push_decl` in `transformation` |
| **Bulk serialization** | serde reads the fields directly by derive | `Ir`'s `Serialize`/`Deserialize` |
| **Performance-critical internal algorithm** | inside the boundary already; §E measured the cost | `canonical_hash.rs`, `hash_pass.rs` — both live in `hymeko_core` and may keep indexing |
| **Malformed-state tests** | construct incoherence on purpose | M0.6 coherence corruption tests, M2a's own foreign-mapping test |

**Nothing in the read paths of the 196-site inventory needs an API this phase did not provide.** M2b can proceed repository by repository without inventing further read APIs — that is the §F completion criterion, and it is met.

---

## Part G — what was not built

No universal `Entity` trait, no `Box<dyn Entity>`, no runtime kind dispatch, no macro-generated API, no arena abstraction, no mutable facade, no query engine, no transaction facade. The only private helper is none at all — each accessor is three lines and shares `decl_node`/`as_*` as its substrate, which was cheaper than introducing an indirection.

---

## Part H — hash debt, still open

M2a changes no hash. These remain exactly as M0.6 left them, and accessor work implies **nothing** about any of them:

- `canonical_program_hash` v1 is **structural-only** — no annotations, values, weights or `bases`.
- **`bases` is absent from both** current identity systems.
- `compute_merkle_hashes` may omit incidence targets under adverse id order.
- Serialized `decl_hash` carries **no `algo_version`**.
- Rename or move affects an entire semantic subtree.
- History-independent aggregation cannot key directly by generational id.

---

## Gates

| Gate | Result |
|---|---|
| `cargo fmt --check` | see §Compliance |
| `cargo clippy --workspace --all-features --all-targets -- -D warnings` | see §Compliance |
| `cargo check --workspace --all-features` | see §Compliance |
| `cargo build --workspace --all-features` | see §Compliance |
| `cargo test --workspace --all-features --no-fail-fast` | see §Compliance |
| `cargo test -p hymeko_core --all-features --no-fail-fast` | see §Compliance |
| `cargo test -p hymeko_wgpu --no-fail-fast` | see §Compliance |
| `cargo test --doc --workspace --all-features` | see §Compliance |

---

## Compliance

- **Additive only.** `hymeko_core/src/ir/ir.rs` was **not modified** — verified by `git diff --stat`. Inherent impls may live in any module of the defining crate, so the whole surface sits in the new `ir/access.rs`.
- **Every prior field and method remains**, with unchanged signature and behaviour: `decl_node`, `decl_node_unchecked`, `decl_children`, `children`, `first_child`, `next_sibling`, `parent`, `kind`, `decl_kind`, `as_node`, `as_edge`, `as_arc`, `preallocate_from_index`, `push_anonymous_arc`, and all public fields.
- **No mutable accessor, no unchecked accessor.** Nothing returns `&Vec`, a slice, or capacity.
- **Not touched:** `Id<T>`, `Ir` field types and storage, parser, lowering, canonical hashes, hierarchy authority, `hymeko_hnn`, `hymeko_wgpu`, every sibling repository. No dependency added.
- **§6.5 anti-patterns:** none. Two requested methods were *not* added because equivalents existed — the inventory came before the design.
- **57/58 fixture drift:** untouched and unsilenced.

---

## Correction — 2026-08-29, after M2a.1

**§E's diagnosis was wrong, and §E.4's REGRESSION verdict is superseded.**

### What was wrong

§E.2 concluded: *"it is the cost of materialising `Option<&T>` and branching on it."* **`Option<&T>` is niche-optimised** — `size_of::<Option<&DeclNode>>() == size_of::<&DeclNode>() == 8`. There is no materialisation cost, and I asserted one without running the one-line check that would have refuted it.

The real cause was a **redundant sentinel branch in code M2a itself introduced**. `DeclId::NONE` is `usize::MAX`, and `[..].get(usize::MAX)` already returns `None`, so:

```rust
if id.is_none() { return None; }   // redundant
self.nodes.get(id.0)
```

Generated code, per element: **6 instructions versus 2**. With two independent conditions LLVM abandons the branch and emits `setne`/`setb`/`testb`; with one it emits a single predicted branch. LLVM cannot remove the redundancy itself — that needs proving `x < len ⟹ x ≠ usize::MAX`.

### What the numbers become

Removing the guard, on the identical cell and Criterion's own stored baseline:

| Cell | M2a | M2a.1 | Change |
|---|---:|---:|---:|
| `m2a_decl_lookup/accessor` @ 1 000 | 785 ns | **527.9 ns** | **−32.7 %** (p = 0.00) |
| `m2a_decl_lookup/accessor` @ 10 000 | — | — | **−18.3 %** (p = 0.00) |

527.9 ns against M2a's own `direct_guarded` control of 527 ns: **the gap is gone**. Corrected verdict: **SCALAR = ZERO-COST**, confirmed on random point lookup and on a dependent chain that removes load overlap.

### The benchmark model was also wrong

§E looped **sequentially over dense ids**, which measures a scan, not point lookup. M2a.1 adds random-point, dependent-chain, mixed-validity and end-to-end cells.

§E.3's `as_node` row (+63 %, later +24 %) was **also** an unfair control: it compared `.is_some()` — which loads only the 8-byte discriminant — against `as_node`, which returns the id and loads the whole 16-byte `Option<NodeId>`. With a control that extracts the id, **MAP is +1.6…2.7 %, inside its gate.**

### What stands

§E.5's measurement-integrity finding stands and is partly explained: that group is load-sensitive, moving ~70 % at 37 % ambient load. The original 16.5 → 7.0 µs shift at 2–4 % ambient remains unexplained.

The M2a measurements above are **retained unedited** as the historical record. They were correctly *reported*; they were incorrectly *explained*, and they were measured against controls that did not do equivalent work.
