# Unified IR — Phase 0: cross-repository architecture, compatibility and performance audit

**Date:** 2026-08-28
**Authorisation:** `APPROVED-CORE-EDIT: unified-ir-phase0-audit-only`
**Nature:** design and measurement only. No production source was modified.
**Objective:** produce an implementation-ready migration plan for one canonical, mutable HyMeKo IR in `hymeko_core`.

**Companion documents** (this report is the evidence; those are the design):

| Document | Contents |
|---|---|
| `docs/unified-ir-architecture.md` | Parts E–H: target architecture, transactions, distributed updates, derived views |
| `docs/unified-ir-migration.md` | Part J: no-flag-day multi-repository migration sequence |
| `docs/unified-ir-consumer-map.md` | Part D: repository-by-repository dependency and consumer table |
| `docs/unified-ir-id-inventory.md` | Part D: ID-use inventory, by category |
| `docs/plans/2026-08-28-unified-ir-phase0/` | Plan source bundle (`.tex`, `.tikz`, `.mmd`) |

---

## 0. Standing conditions

### 0.1 Measurement contamination — binding on every number in this report

A stray process was consuming CPU throughout the measurement window: **PID 4196**, `python.exe` from `shuttle_trainer` (not this project), started 14:47:40, **9210 s of CPU** by 17:22. It is an infinite busy-wait:

```python
[time.sleep(.02) for _ in iter(int,1) if j.status()=='running']
```

`iter(int, 1)` never terminates, and once the job stops being `running` the `if` filter skips the `sleep` while the comprehension keeps iterating — so it spins `j.status()` with no delay.

**PID 4196 must not be terminated without separate explicit authorisation.** It has not been.

**Consequently every timing in this report is contaminated and diagnostic only.** They live in **Appendix I** and are labelled there. They are *not* baselines, they must not be quoted as baselines, and no performance claim in the migration plan rests on them. Part I specifies the quiet-remeasurement protocol and the exact commands.

Size, count and structural findings are timing-independent and are unaffected.

### 0.2 `plan.pdf` — waived

The user has granted Phase 0 an explicit `plan.pdf` tooling waiver, and directed that no TeX, Tectonic, Pandoc or other document toolchain be installed. `plan.tex`, `plan.tikz` and `plan.mmd` are retained as the reproducible source bundle. **This closes the §11 halt condition for Phase 0.** It is recorded, not worked around; nothing was installed.

### 0.3 Two corrected conclusions

An earlier draft of this report overstated two results. Both are corrected here, and the corrections propagate to the architecture and migration documents.

| # | Was | Is |
|---|---|---|
| 1 | "Full-state commit identity is affordable… **this retires a design question**" | **Incremental state identity is not retired.** The evidence supports only: *not yet justified at the one scale measured, on a contaminated host*. It was measured at two sizes, single-threaded, with no commit-rate model at all. §I.4 re-evaluates it against update frequency, and §0.4 settles it against the targets since given: **it is required.** |
| 2 | "M2 before M1 … taking the measured 61%/construction-cost win first" | The **structural** justification for M2-before-M1 stands on its own (redundant storage, five unenforced lockstep arrays, blast radius entirely inside `core`). The **performance** justification is **provisional** until quiet remeasurement. M2's ordering does not depend on it — `docs/unified-ir-migration.md` §J.1 derives the order from an encapsulate-then-change argument that needs no measurement at all. |

### 0.4 Operating envelope — given 2026-08-28

Commit-rate targets were supplied after the structural audit closed. They are requirements, not estimates, and they convert open questions into settled decisions.

| Commit class | Sustained | Burst | Budget/commit | Coalescing |
|---|---:|---:|---:|---|
| Value / weight / feature / annotation | **100 Hz** | **250 Hz** | **10 ms** (4 ms burst) | not assumed |
| Topology | **10 Hz** | **30 Hz** | **100 ms** (33 ms burst) | **permitted** |

Plus: **inference readers must never block** on mutation or publication, and **full derived-view rebuild is required only for topology changes.**

Four consequences, specified in `docs/unified-ir-architecture.md`:

1. **Incremental / Merkle identity is required** (§F.6) — this closes §I.4. At 100 Hz the budget is 10 ms; full-state identity alone was ≈9 ms **[contaminated]** and the real canonical hash costs more. The conclusion is robust to the contamination: full-state would need to be an order of magnitude cheaper than measured to fit, and the true figure is *higher*, not lower.
2. **Topology and non-topology invalidation are separate paths** (§H.3, §H.4).
3. **Publication must use immutable snapshots** (§F.7) — readers must not block, so no reader-visible lock.
4. **Commit throughput and topology-materialization throughput are distinct metrics** (§H.5), measured and reported separately.

**This does not license using the contaminated numbers as budgets.** They are used here only for an order-of-magnitude sufficiency argument whose sign cannot flip; every actual budget still awaits a quiet baseline (§I.5).

---

## 1. Repository state at audit time

Read-only. No sibling repository was modified.

| Repository | Branch | HEAD | Working tree |
|---|---|---|---|
| `core` | main | `f84d2256514f572c1dfba7dd1c00683fa0a69628` | Phase 0 artifacts + 3 pre-existing untracked IFAC files, all preserved |
| `graph` | main | `ac341e0` | clean |
| `hotaru` | main | `21017e9` | clean |
| `network` | main | `8d88ffe` | clean |
| `parser` | **master** | `284c490` | clean |
| `tensor` | main | `c0f9719` | clean |
| `transformation` | main | `21b5826` | clean |
| `vscode-plugin` | main | `19b8269` | clean |

Not git repositories: `docs`, `hymeko_python`, `phase3-backup-20260825`, `proofs`, `vendor`.

`parser` is on `master` while every peer is on `main`. Cosmetic today; it matters the moment CI or a submodule pin assumes a uniform default branch.

**Writes made by this phase** — all inside `core`, all authorised, none to production source:

- `reports/2026-08-28-unified-ir-phase0-audit.md`
- `docs/unified-ir-architecture.md`, `docs/unified-ir-migration.md`, `docs/unified-ir-consumer-map.md`, `docs/unified-ir-id-inventory.md`
- `docs/plans/2026-08-28-unified-ir-phase0/{plan.tex,plan.tikz,plan.mmd}`
- `hymeko_core/benches/ir_bench.rs` — measurement only
- `hymeko_core/Cargo.toml` — one `[[bench]]` stanza (`autobenches = false` makes it required)

Pre-existing untracked files (`deliverables/`, `reports/2026-08-26-ifac-geoff-chase-materials.md`, `tools/build_ifac_materials.ps1`) are untouched.

---

## 2. Part A — canonical IR inventory

### 2.1 Structure

`hymeko_core/src/ir/ir.rs` (274 lines). Four arenas, three index maps, two hash side-tables:

```rust
pub struct Ir {
    pub meta:      Option<Meta>,
    pub doc_hash:  Option<HashId>,
    pub decl_hash: Vec<Option<HashId>>,      // parallel to decl_nodes
    pub arc_hash:  Vec<Option<HashId>>,      // parallel to arcs
    pub decl_nodes: Vec<DeclNode>,           // DeclId indexes
    pub nodes:      Vec<NodeRec>,            // NodeId indexes
    pub edges:      Vec<EdgeRec>,            // EdgeId indexes
    pub arcs:       Vec<ArcRec>,             // HyperArcId indexes
    pub decl_to_node: Vec<Option<NodeId>>,
    pub decl_to_edge: Vec<Option<EdgeId>>,
    pub decl_to_arc:  Vec<Option<HyperArcId>>,
}
```

### 2.2 Semantics that must survive migration

| Concept | Carrier | Lost if flattened to pairwise |
|---|---|---|
| **Declaration hierarchy** | `DeclNode.{parent, first_child, last_child, next_sibling}` | Scoping/namespacing. An intrusive 4-field list, not a `Vec` tree. |
| **Node** | `NodeRec { decl, bases }` | `bases` is *inheritance*. |
| **Hyperedge** | `EdgeRec { decl, bases, arcs }` | An edge owns **N arcs**; it is not one incidence list. Edges inherit too. |
| **Hyperarc** | `ArcRec { anno, in_edge, refs }` | The arc is the unit of incidence and carries its own annotation. |
| **Sign** | `SignedRefR::{Plus, Minus, Neutral}` | **Three-valued.** `Neutral` is not `Plus`. |
| **Per-reference weights** | `RefAtomR.weights: Option<Vec<ValueR>>` | A *vector* per reference, not a scalar. |
| **Annotation** | `AnnoR { tags: Vec<SymId>, value: Option<ValueR> }` | Tags plus recursive `ValueR` (`Str`/`Num`/`List`/`Ref`). |
| **Reference target** | `RefAtomR.target: DeclId` | Targets a **declaration** — so an arc may reference an *edge*. |

The last row is the one most easily lost: because `target` is a `DeclId`, HyMeKo expresses edge-to-edge references. Any model typing incidence as `(VertexKey, Sign)` cannot represent them — and the editor IR does exactly that (§4).

### 2.3 Duplicated and derivable mappings

| Mapping | Authoritative | Redundant copy | Writers | Dense readers | Divergence mode |
|---|---|---|---|---|---|
| decl → node | `decl_to_node[d]` | `NodeRec.decl` | lowering; `resize` L144; `bridge::push_decl` | `ir.decl_to_node[d.0]` | node's `decl` disagrees with the map |
| decl → edge | `decl_to_edge[d]` | `EdgeRec.decl` | lowering; `resize` L145; bridge | `ir.decl_to_edge[d.0]` | same |
| decl → arc | `decl_to_arc[d]` | `ArcRec.in_edge` — **not the inverse**, it names the *edge* decl | lowering; `resize` L146; bridge | `ir.decl_to_arc[d.0]` | the "inverse" of `decl_to_arc` is not `in_edge`; the asymmetry is itself the hazard |
| edge → arcs | `EdgeRec.arcs: Vec<HyperArcId>` | `ArcRec.in_edge: DeclId` | lowering; bridge | `ir.edges[e.0].arcs` | edge disowns a claiming arc. **Type mismatch:** forward `HyperArcId`, back `DeclId` |
| decl → hash | `decl_hash[d]` | — | `hash_pass` | positional | length drift vs `decl_nodes` |
| arc → hash | `arc_hash[a]` | — | `hash_pass` | positional | length drift vs `arcs` |
| hierarchy | 4 intrusive fields | — | lowering | pointer-chase | `last_child` unreachable from `first_child`; `next_sibling` cycle |

**Five arrays must grow in lockstep on every declaration insert** — `decl_nodes`, `decl_hash`, `decl_to_node`, `decl_to_edge`, `decl_to_arc` — and *nothing in the type system couples them*.

This is not hypothetical. **`bridge.rs` re-implements the lockstep by hand** in its own `push_decl` (bridge.rs L58–65), duplicating five pushes that `ir.rs` also performs. Two independent implementations of an unenforced invariant, in two repositories. A third writer would make three.

Today `Ir` is built once and read, so the invariant holds by construction. **Under mutation it becomes five independent chances to desynchronise per insert, and every reader indexes densely** (`self.decl_nodes[id.0]`, not `.get()`). Drift is an OOB panic or a silent wrong answer.

This is the strongest structural argument for the migration, and it does not depend on any timing.

---

## 3. Part B — ID audit

### 3.1 The `#[repr(C)]` claim

```rust
#[repr(C)] // Stable layout for serde and FFI
pub struct Id<T>(pub usize, PhantomData<T>);
```

**False three ways.** (1) `repr(C)` fixes field *order*, not *width*; the field is `usize`, so the type is 4 bytes on a 32-bit target and 8 on 64-bit — not a stable FFI or storage layout. (2) `repr(C)` is irrelevant to serde, which works on the impl, not the layout. (3) `Id::NONE == usize::MAX` serializes as `18446744073709551615` on 64-bit and `4294967295` on 32-bit; a 64-bit `NONE` read on a 32-bit host is not `NONE` but an out-of-range index, and D1 makes it undetectable.

### 3.2 Verified defects

`ids.rs` declares **no derives**; every trait is hand-written in `ids_impl.rs`.

| # | Defect | Consequence |
|---|---|---|
| D1 | `Serialize` writes a **bare `usize`, no type tag** (`self.0.serialize()`) | Serialized `NodeId(3)` is byte-identical to `EdgeId(3)`. The phantom type is erased exactly at the boundary where it is most needed. |
| D2 | `NONE = usize::MAX` does not round-trip across word size | Silent cross-architecture replica divergence. |
| D3 | `#[repr(C)]` comment false | Misleads the next implementer. |
| D4 | `pub usize` field + `From<usize>` + `From<Id<T>> for usize` | `EdgeId::from(node_id.0)` compiles. Type safety is advisory. |
| D5 | `Ord`/`PartialOrd` order by raw index | Ordering reliance silently assumes **allocation order** — valid only while IDs are dense and append-only, i.e. only while the IR is immutable. |
| D6 | **Two absence encodings for one concept; the redundant one costs 2×** | `DeclNode.parent` uses the `DeclId::NONE` sentinel; `Ir.decl_to_*` uses `Option<NodeId>`. `usize::MAX` is a *valid* `usize`, so `Id<T>` has **no niche**: `size_of::<Option<NodeId>>() == 16` against `size_of::<NodeId>() == 8`. The type already encodes absence and the code pays 8 bytes per slot to encode it again. |

D6 is a size fact, independent of the contaminated timings.

### 3.3 Counts

The full inventory, by category and repository, is `docs/unified-ir-id-inventory.md`. Headline: **`transformation` holds 127 dense-index sites against `hymeko_core`'s 52** — the emitter is the largest consumer of "an ID is a dense array offset", and it is a sibling repository. §D explains why that forces coordination.

---

## 4. Part C — editor IR and bridge

### 4.1 Concept mapping, both directions

`HyMeKoEditorIR` (`transformation/hymeko_emitter/src/editor_ir.rs`) against `Ir`:

| Editor concept | Core `Ir` counterpart | Direction fidelity | Classification |
|---|---|---|---|
| `Vertex.name` | `DeclNode.name: SymId` via `Interner` | both, lossless | **canonical** |
| `HyperEdge.name` | `DeclNode.name` (kind `Edge`) | both, lossless | **canonical** |
| `HyperEdge.incident: Vec<(VertexKey, Sign)>` | `ArcRec.refs: Vec<SignedRefR>` | **asymmetric — see §4.3** | **canonical** |
| `Sign::{Plus,Minus,Neutral}` | `SignedRefR::{Plus,Minus,Neutral}` | both, lossless (1:1) | **canonical** |
| `HyperEdge.weight: f64` | `RefAtomR.weights: Option<Vec<ValueR>>` | **neither** — editor→arena drops it; arena→editor synthesises `1.0` | **canonical**, currently broken |
| `Vertex.level: i8` (G-SPHF level) | *none* | editor-only; reverse synthesises `0` | **canonical** — a G-SPHF level is semantics, not presentation |
| `Vertex.attributes: Vec<Attribute>` | `AnnoR { tags, value }` — a real counterpart exists | **neither direction implemented** | **canonical** |
| `Vertex.position: Option<Position>` | *none* | editor-only | **presentation metadata** |
| `Patch` / `PatchKey` | *none* | editor-only | **presentation metadata** (grouping/selection) |
| `HyperEdge.patch_id` | *none* | editor-only | **presentation metadata** |
| — | `NodeRec.bases` / `EdgeRec.bases` (inheritance) | **dropped, undocumented** | **canonical** |
| — | `DeclNode.{parent,first_child,…}` hierarchy | **flattened**, all parents `NONE` | **canonical** |
| — | `RefAtomR.target` to an *Edge* decl | **filtered out** (tested, intentional) | **canonical** |
| — | `Ir.meta`, `doc_hash`, `arc_hash` | dropped | derived/provenance |

**Classification rule applied:** a concept is *canonical* if it changes what the program means, compiles to, or hashes as. It is *presentation metadata* if removing it changes only what a human sees on a canvas. By that rule `position`, `Patch` and `patch_id` are presentation; **`level` and `attributes` are not**, despite living only in the editor today — a G-SPHF level is a semantic property, and `attributes` has a direct `AnnoR` counterpart that is simply unwired.

### 4.2 Information loss and synthesis

`to_compiler_ir` (editor → arena) **drops**: `level`, `attributes`, `position`, `patch_id`, `weight`, and all `Patch` records. The module doc-comment lists the first four honestly.

`from_compiler_ir` (arena → editor) **drops**: `bases`, `AnnoR` (tags and value), `weights`, declaration hierarchy, and edge-targeted references — and **synthesises** `level: 0`, `attributes: []`, `position: None`, `patch_id: None`, `weight: 1.0`.

**The documentation asymmetry is the finding.** The bridge's doc-comment carefully enumerates the *editor-only* fields that do not survive to the arena, then says nothing about the *arena-only* fields that do not survive to the editor. `bases` and `AnnoR` are dropped silently and are absent from every documented list.

**And the loss is not uniform across consumers.** Three consumers read the same `Ir` with three different fidelities:

| Consumer | `bases` | `AnnoR` tags | `value` |
|---|---|---|---|
| `emit_hymeko.rs` (text emitter) | **preserved** | preserved | preserved |
| `hymeko_formats::snapshot` (WASM + Python `NodeDto`) | **preserved** (`bases: Vec<String>`) | **preserved** (`tags: Vec<String>`) | **preserved** (`value: Option<String>`) |
| `bridge.rs` (editor) | **dropped** | **dropped** | **dropped** |

The WASM snapshot path — the *other* browser-facing consumer — already carries exactly the fields the editor bridge discards. So the loss is not a WASM constraint or a serialization limit; it is specific to the bridge. Anyone reasoning about "the emitter" as one thing will get this wrong.

### 4.3 Round-trip asymmetry: N arcs become 1

The two directions do not compose symmetrically.

- `to_compiler_ir` emits **exactly one `ArcRec` per hyperedge** (bridge.rs L118–137: one `push_decl(HyperArc)`, one `ir.arcs.push`, one `edge_rec.arcs.push`).
- `from_compiler_ir` **concatenates every arc of an edge into one `incident` list** (L180–188: nested loop over `edge.arcs` then `arc.refs`, all pushed into one `Vec`).

Therefore:

| Round trip | Result |
|---|---|
| editor → arena → editor | **structurally faithful** (1 arc in, 1 arc out) |
| **arena → editor → arena** | **N arcs collapse to 1, irreversibly** |

The bridge's documented contract covers only the first: *"`from_compiler_ir(to_compiler_ir(editor)) ≡structural editor`"*. It is honest about the direction it claims. **The other direction is lossy and is neither documented nor tested** — the round-trip test (`test_bridge.rs` L236–237) asserts `nodes.len()` and `edges.len()` and **never asserts `arcs.len()`**, which is precisely the quantity that collapses.

### 4.4 Ordering drift and unstable key mapping

- `to_compiler_ir` iterates `editor.vertices.iter()` — **`SlotMap` iteration is slot order**, which depends on the insert/remove *history*, not on content. Two editor IRs with identical content but different edit histories therefore produce **different `DeclId` assignments**. Every `DeclId` in the produced `Ir` is a function of editing history.
- The bridge itself states keys are not preserved: *"Slotmap keys are not preserved across the round-trip — the reverse walk allocates fresh keys."* So editor identity is destroyed on every arena crossing. There is no stable cross-boundary entity identity anywhere in the system today.
- `vertex_to_decl` and `node_decl_to_vkey` are `BTreeMap`s — deterministic *given* the keys, but the keys are history-dependent, so determinism of the map does not rescue determinism of the result.
- `to_compiler_ir` uses `filter_map(|(vkey, sign)| vertex_to_decl.get(vkey).map(...))`: an incident entry whose vertex is missing is **silently dropped**, not reported. Combined with §4.5 this is a silent-arity-change path.

**Consequence for canonical hashing.** `canonical_program_hash` is deliberately name- and layout-invariant (Prüfer skeleton + canonical incidence bytes), so it is plausibly robust to this reordering — but *plausibly* is the operative word: **it has not been tested against a permuted-history editor IR**, and that test does not exist. Until it does, "the hash is stable across editor history" is a hypothesis, not a result. §I.6 specifies the test.

### 4.5 `Batch` is non-atomic — demonstrated

Documented as *"Atomic mutation unit"* and *"succeeds-or-fails as one unit"*. Implemented (editor_ir.rs L192–197):

```rust
IRDelta::Batch { deltas } => {
    for d in deltas {
        self.apply(d)?;      // early return; prior deltas stay applied
    }
    Ok(())
}
```

**Demonstration** (no code run; the failure is exhibited by construction from the code above and the error paths in the same file):

> Let the IR hold vertices `{A}` and no edges. Apply
> `Batch { deltas: [ AddVertex{X}, RemoveEdge{ nonexistent_key } ] }`.
> `AddVertex` executes `self.vertices.insert(data)` and returns `Ok`. `RemoveEdge` executes `self.hyperedges.remove(key).ok_or(IRError::NotFound)?` and returns `Err(NotFound)`, which `?` propagates out of the loop.
> **Post-state: vertices `{A, X}`, and the caller holds `Err`.** A caller that treats `Err` as "nothing happened" now disagrees with the IR about the vertex set — and cannot repair it, because §4.6 means it never learned `X`'s key.

The `apply` doc-comment concedes this (*"if any fails, the already-applied ones are not rolled back"*), so **the documentation contradicts itself**: the type-level doc claims atomicity, the method-level doc denies it. The method-level doc is the accurate one. There is no test asserting either.

### 4.6 `apply` discards the allocated key — deltas are not gossipable

`editor_ir.rs` L128 (signature) and L131, L143, L189 (the three insert sites):

```rust
pub fn apply(&mut self, delta: IRDelta) -> Result<(), IRError> {     // L128
    IRDelta::AddVertex    { data } => { self.vertices.insert(data);   Ok(()) }  // L131
    IRDelta::AddHyperEdge { data } => { self.hyperedges.insert(data); Ok(()) }  // L143
    IRDelta::AddPatch     { data } => { self.patches.insert(data);    Ok(()) }  // L189
```

`SlotMap::insert` returns the new key. **All three discard it.** The signature returns `Result<(), IRError>`, so a caller applying `AddVertex` **cannot learn the `VertexKey` that was assigned.**

This breaks the module's own stated purpose. The module doc (`editor_ir.rs` L8–10) says the deltas exist so *"a P2P layer can gossip the deltas verbatim (they `#[derive(Serialize, Deserialize)]`)"*. It cannot:

1. Replica A applies `AddVertex{v}` and gets key `k_A`.
2. Replica B applies the identical gossiped delta and gets key `k_B`, in general `≠ k_A` (slot reuse differs with each replica's deletion history).
3. Replica A now gossips `AddHyperEdge{ incident: [(k_A, Plus)] }`.
4. On B, `k_A` is meaningless. `to_compiler_ir`'s `filter_map` **silently drops the incidence**, so B's edge is built with lower arity than A's — no error, no divergence signal.

**Verbatim delta gossip is structurally impossible with the current delta vocabulary**, and its failure mode is silent arity divergence rather than a fault. This is the primary constraint on Part F/G and is why transaction-local IDs are mandatory, not stylistic.

### 4.7 Dangling references and cascade asymmetry

- `RemoveVertex` **does** cascade into hyperedges: `for he in self.hyperedges.values_mut() { he.incident.retain(|(v,_)| *v != key) }`. It is a silent structural mutation of edges triggered by a vertex delete — an edge can lose arity or become empty with no notification. That is a legitimate policy, but it is O(edges) per vertex removal and it is invisible to the caller.
- `RemoveVertex` **does not** cascade into `Patch.vertices: Vec<VertexKey>`. **Patches retain dangling vertex keys.** There is also **no `RemovePatch` delta at all** (grep count: 0), so patches accumulate and can never be cleaned.
- `UpdateSign { key, arc_index, sign }` addresses an arc **positionally** into `incident` — and `RemoveVertex`'s `retain` *shifts those positions*. An `UpdateSign` that was valid when issued can silently address a different arc after a concurrent vertex removal. This is the clearest non-commutative pair in the vocabulary (§G).
- `AttachAttribute` pushes without a duplicate check; `DetachAttribute` retains-out **all** matching keys. So attach·attach·detach ≠ attach: the operations are neither idempotent nor inverse.

---

## 5. Part D — consumer map

Full tables: `docs/unified-ir-consumer-map.md` and `docs/unified-ir-id-inventory.md`.

### 5.1 Why 127 of 196 `transformation` sites force a coordinated migration

*(Corrected counts — the tightened inventory in §D supersedes an earlier looser regex; the conclusion is unchanged and the ratio is if anything sharper.)*

`transformation` holds **127 dense-index sites against `hymeko_core`'s 52** — the emitter indexes core's arenas more than core does. The reason it cannot be migrated independently, in four parts:

1. **The dependency is one-way and compile-time.** `transformation` depends on `hymeko_core` (`use hymeko::common::ids::…`, `use hymeko::ir::ir::…`). Core cannot depend back. So an ID representation change in core is an **immediate compile break** in `transformation` — there is no version in which both work.
2. **`transformation` writes core's invariants, it does not merely read them.** `bridge.rs::push_decl` performs the five-array lockstep push itself (§2.3). It is a *second writer* of a core invariant living in another repository. Any change to that invariant must land in both repositories in the same change, or the second writer corrupts what the first repairs.
3. **The break is silent, not loud.** The dangerous sites are `ir.decl_nodes[node.decl.0]`, `ir.arcs[arc_id.0]`, `ir.decl_to_node[did.0] = Some(nid)`. Under a stable-key scheme these still *compile* if `Id` keeps a `usize`-like surface (D4's `From<usize>` guarantees it) and then index the wrong element. **A migration that leaves D4 in place converts a compile error into a wrong answer.** Removing `From<usize>`/`pub .0` is therefore a *safety* requirement for the migration, not a cleanup.
4. **The consumers are not only Rust.** `hymeko_python` exposes **23 FFI sites**, and `hymeko_formats::snapshot` serialises `target_id: usize` into the WASM/browser JSON DTO. Those are published contracts; changing the ID representation changes a wire format that a browser and a Python package already consume.

**Conclusion:** M1 (ID representation) is a synchronised multi-repository change and cannot be staged inside `core` alone. `docs/unified-ir-migration.md` §J.3 specifies the adapter that makes it staged anyway.

---

## 6. Findings, ranked

1. **`apply` discards the allocated key (§4.6).** Delta gossip — the editor IR's stated purpose — is structurally impossible, and it fails silently as arity divergence. Blocks Parts F and G outright.
2. **Five unenforced lockstep arrays, with a second hand-written writer in another repository (§2.3).** The blocking issue for mutability. Timing-independent.
3. **`Batch` is documented atomic and is not (§4.5).** A live defect on the exact component transactions would build on.
4. **No derived view records its source identity or version (§H).** `HyperGraphView::from_ir` keeps no link to the `Ir` it came from; grep for version/generation/dirty/invalidate/stale in the view modules returns **nothing**. Every view is silently stale the moment the IR becomes mutable.
5. **`resuming_at` restores the version but not the commit digest (§G).** A resumed consumer cannot authenticate a same-version replay.
6. **Six `Id<T>` defects (§3.2)**; D1 and D2 are correctness issues for a distributed IR, and D4 converts the migration's compile errors into silent wrong answers (§5.1.3).
7. **Bridge drops `bases`/`AnnoR` undocumented, while two other consumers preserve them (§4.2).** Not a platform constraint — bridge-specific.
8. **arena→editor→arena collapses N arcs to 1, untested and undocumented (§4.3).**
9. **`Patch.vertices` dangles after `RemoveVertex`; no `RemovePatch` exists (§4.7).**
10. **Canonical-hash stability under editor-history permutation is untested (§4.4)** — plausible, unproven.
11. `parser` on `master` while peers are on `main` (§1).

---

## Appendix I — measurements

### I.1 Status: CONTAMINATED — DIAGNOSTIC ONLY

Collected while PID 4196 was consuming a core (§0.1). **These are not baselines and must not be published as such.** They are retained because they were the basis of the (now corrected) conclusions in §0.3 and because their *relative* structure motivates what to measure properly.

Host: Intel Core Ultra 9 275HX, 24 cores/24 logical, Windows 11. Criterion 0.8.2, `harness = false`, release. Single-threaded. Ambient load not recorded — itself a defect in the collection, corrected by the protocol in §I.5.

### I.2 Construction (contaminated)

| Config | decls | incidences | median | ns/decl |
|---|---:|---:|---:|---:|
| n1000 e500 a4 | 2 001 | 2 000 | 123.97 µs | 62 |
| n10000 e5000 a4 | 20 001 | 20 000 | 2.3255 ms | 116 |
| n100000 e50000 a4 | 200 001 | 200 000 | 37.865 ms | 189 |
| n10000 e5000 a32 | 20 001 | 160 000 | 4.2087 ms | 210 |

Apparent structure — **hypothesis, not result**: per-declaration cost rises with scale, and declarations appear ~8.6× more expensive per unit than incidences (13.5 ns/incidence derived from the a4→a32 delta). If it survives quiet remeasurement it supports M2; **M2 does not depend on it** (§0.3).

### I.3 State identity (contaminated)

| Config | elements | median | throughput | serialized JSON |
|---|---:|---:|---:|---:|
| n1000 | 4 001 | 89.54 µs | 22.34 Melem/s | 575 871 B |
| n10000 | 40 001 | 894.55 µs | 22.36 Melem/s | 5 860 377 B |

The JSON sizes are **exact and uncontaminated** (byte counts, not timings): **293 B per incidence**. That is fat for a wire format and is why the Phase 10A binary envelope exists. `serde_json` must not become the transport.

### I.4 Re-evaluating incremental identity — corrected conclusion

The earlier claim that full-state hashing "retires" incremental identity was wrong: it generalised from two sizes on a contaminated host with **no commit-rate model**. Re-evaluated against update frequency, using the contaminated figure as an order-of-magnitude placeholder (≈22 Melem/s ⇒ ≈9 ms at 10⁵ declarations):

| Scenario | Commit rate | Full-state cost/s at 10⁵ decls | Verdict |
|---|---|---|---|
| Batch compile / CI | ≪1 Hz | negligible | full-state sufficient |
| Interactive editing, commit-on-idle | ~1 Hz | ~0.9% of one core | full-state sufficient |
| **Per-keystroke commit** | **10–30 Hz** | **9–27% of one core, for an unchanged 99.99% of the graph** | **incremental required** |
| **Live inference, IR mutating under a running model** | **10–100 Hz** | **9–90% of one core** | **incremental required** |
| Multi-replica gossip, per-delta digest | per delta | scales with delta rate, not delta size | **incremental required** |

The last two are **the stated final-system requirement** — "the IR changing dynamically while inference may already be running". So incremental identity is not merely un-retired; **it is required in the target regime and only avoidable in the batch regime.**

Two structural points independent of any timing:

- Full-state hashing is **O(size of graph) per commit**, while a delta is O(size of change). At high commit rates on a large graph the ratio is unbounded, regardless of the constant.
- `canonical_program_hash` is **not** a running digest: it builds a `did_to_path` reverse map, a skeleton forest, subtree hashes and a Prüfer encoding, each a full traversal. Its constant is materially worse than the flat BLAKE3 sweep measured in §I.3, so §I.3 is a *lower bound* on real canonical-identity cost, not an estimate of it.

**Corrected position:** full-state identity is adequate for batch and interactive-idle commit rates; incremental identity is required for per-keystroke and live-inference rates.

**Settled by the §0.4 targets.** The given envelope — 100 Hz sustained / 250 Hz burst for value, weight, feature and annotation commits — lands squarely in the two rows marked *incremental required*. The question is closed: `docs/unified-ir-architecture.md` §F.6 specifies the Merkle-fold digest, and it is **required M4 scope**, not a preserved option.

Two things this does *not* settle, both now on the §I.6 list: the **declaration-tree depth distribution**, on which the fold's O(changed × depth) cost depends and which nothing has measured; and whether `canonical_program_hash` can itself be made incremental or whether a second cheaper hot-path digest is needed with the canonical hash reserved for publication boundaries.

### I.5 Quiet remeasurement protocol

**Precondition — do not measure until one holds:** PID 4196 is resolved (with separate explicit authorisation), or the machine is otherwise verified quiet.

Quietness gate, recorded *with* every run:

```powershell
Get-Process | Where-Object { $_.CPU -gt 30 } |
  Select-Object ProcessName,Id,CPU,StartTime | Format-Table -Auto
(Get-CimInstance Win32_Processor).LoadPercentage
```

Accept only if no unexpected process is accumulating CPU and `LoadPercentage` is in single digits before and after. Record both readings in the report; a run without them is not a baseline.

Commands, exactly:

```bash
# Baseline, all groups
cargo bench -p hymeko_core --bench ir_bench 2>&1 | tee reports/bench/ir_bench-<host>-<UTC>.log

# Single group
cargo bench -p hymeko_core --bench ir_bench -- ir_construction
cargo bench -p hymeko_core --bench ir_bench -- ir_state_identity
cargo bench -p hymeko_core --bench ir_bench -- ir_redundancy

# A/B against a saved baseline (Criterion compares automatically)
cargo bench -p hymeko_core --bench ir_bench -- --save-baseline quiet-pre
cargo bench -p hymeko_core --bench ir_bench -- --baseline quiet-pre
```

**Alternation, not sequencing.** Competing variants (pre/post M2, `usize` vs `NonZeroU32`) must be measured **interleaved** — `A,B,A,B,…`, at least 3 alternations — not "all A then all B". Thermal drift and background load are time-correlated, so block ordering aliases them into the effect. Report median/IQR/worst per §3 of `CLAUDE.md`.

### I.6 To measure once quiet — coverage required beyond today's bench

`ir_bench.rs` covers construction, identity and redundancy only. The following are **unmeasured** and named by the brief:

| Target | Why | Exists? |
|---|---|---|
| `Ir` **lookup** (`decl_node`, `node_for_decl`, hierarchy walk) | M2 replaces stored maps with a derived index; lookup is the axis that could regress | no |
| `Ir` **clone** | transactional overlay/rollback (§F) may clone; the cost bounds the design | no |
| **Canonical hash** (`canonical_program_hash`) | the *real* identity cost; §I.3 measured a flat sweep, not this | no |
| **Serialization** (binary envelope, not `serde_json`) | wire cost for §G | Phase 10A `ir_stream_bench` covers the envelope, not `Ir` |
| **Bridge** `to_compiler_ir` / `from_compiler_ir` | the editor's per-crossing cost | no |
| **`HyperGraphView::from_ir`** | full rebuild cost — the number that decides incremental-vs-rebuild in §H | no |
| **COO/CSR** construction | same, for the tensor path | `csr_bench` exists; not IR-sourced |
| **Canonical-hash invariance under editor-history permutation** | §4.4 hypothesis; correctness, not speed | no |

The last is a **correctness test, not a benchmark**, and does not need a quiet host — it can be written before the spinner is resolved.

**Added by the §0.4 envelope.** These size the M4 design and did not exist as questions before the targets were given:

| Target | Why it is now required |
|---|---|
| **Declaration-tree depth distribution** | The Merkle fold costs O(changed × depth). Depth is user-authored and unbounded. **This is the single input the identity design most needs**, and it is a *structural* measurement — it does not need a quiet host. |
| **Incremental value-patch cost at 10⁵ declarations** | Decides whether the 10 ms non-topology budget holds. |
| **Topology rebuild throughput, with coalescing** | The question is not "does one rebuild fit in 100 ms" but "how many deltas must coalesce to sustain 10 Hz / burst 30 Hz". A throughput question, not a latency one. |
| **Materialization lag** | Committed version minus materialized topology version, sustained and worst case. The metric that exposes derived views falling behind while commit throughput still looks healthy (§H.5). |
| **Snapshot retention depth under a slow reader** | §F.7's memory-leak trigger. At 100 Hz a reader holding a snapshot for 10 s pins 1000 versions. |

---

## 7. Compliance

- **CORE.YAML items touched:** none. One `[[bench]]` stanza under this phase's explicit authorisation; `autobenches = false` makes it required.
- **Production source changed:** none. No ID representation, `Ir` layout, parser, lowering, emitter or bridge change; no transaction implementation; no dependency or lockfile change.
- **Sibling repositories:** read-only, all clean at the §1 HEADs.
- **PID 4196:** not terminated. Not investigated beyond reading its command line.
- **Pre-existing untracked files:** preserved.
- **Gates:** `cargo clippy --benches -- -D warnings` clean; `cargo fmt --check` clean; bench target verified to actually build (`ir_bench-*.exe`) rather than trusted to a green `cargo build`.
- **Test suite:** 103 + 152 pass. One pre-existing failure, `test_load_meta_geometry` (`meta kinematics node count drifted, left: 58, right: 57`) — the **known 57/58 fixture drift**, which standing instruction requires be left untouched and unsilenced. Verified pre-existing by stashing this phase's only tracked change and reproducing the identical failure. Left as found.
- **§6.5 anti-patterns:** none introduced.
- **Waiver:** `plan.pdf` — explicit user waiver (§0.2). No toolchain installed.
- **Not committed.** Per instruction.
