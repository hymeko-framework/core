# Unified IR — target architecture (Phase 0, Parts E–H)

**Date:** 2026-08-28 · **design only, no implementation** · evidence in `reports/2026-08-28-unified-ir-phase0-audit.md`

This document specifies the target. It does not authorise code. Every claim marked **[measured]** rests on a size or count from the audit; **[contaminated]** on a timing that must be re-established quietly; **[hypothesis]** on reasoning not yet tested.

---

## Operating envelope

**Given 2026-08-28.** These are requirements, not estimates. They convert several questions this document had left open into settled decisions, and they are the reason the answers below are what they are.

| Commit class | Sustained | Burst | Budget/commit (sustained) | Coalescing |
|---|---:|---:|---:|---|
| Value / weight / feature / annotation | **100 Hz** | **250 Hz** | **10 ms** (4 ms at burst) | not assumed |
| Topology | **10 Hz** | **30 Hz** | **100 ms** (33 ms at burst) | **permitted** |

Plus two absolute constraints:

- **Inference readers must never block** on mutation or publication.
- **Full derived-view rebuild is required only for topology changes.**

### Four consequences, each binding on a section below

**1. Incremental / Merkle identity is required — not deferred.** At 100 Hz the whole commit budget is 10 ms. Full-state identity at 10⁵ declarations was ≈9 ms **[contaminated]**, and `canonical_program_hash` is materially *more* expensive than that flat sweep (it builds a path map, skeleton forest, subtree hashes and a Prüfer encoding — audit §I.4). Full-state identity therefore consumes the entire budget or exceeds it, before any actual work. At 250 Hz burst it exceeds it outright.

> This closes audit §I.4. The earlier position — "not yet justified at the measured scale" — was correct *given no commit-rate target*. With one, the answer is settled: **incremental identity is mandatory**, and the Merkle-fold structure in §F.6 moves from "keeps the option open" to "is the design".

The conclusion does not depend on the contaminated figure's precision. For full-state identity to fit inside a 10 ms budget with room for validation, commit and publication, it would have to be roughly an order of magnitude cheaper than measured — and the real canonical hash is *more* expensive, not less.

**2. Topology and non-topology invalidation are separate paths.** A 10× rate difference and a rebuild-only-on-topology rule mean these cannot share a mechanism. §H.3 and §H.4 specify them separately; §E.6 splits the dense view's storage to make it possible.

**3. Publication must use immutable snapshots.** "Readers must not block" forbids any reader-visible lock. §F.7 specifies the mechanism.

**4. Commit throughput and topology-materialization throughput are distinct metrics.** They are decoupled by design and must be measured, budgeted and reported separately. §H.5.

---

## Language and architecture contracts

**Decided 2026-08-28.** These are binding contracts, not preferences. Evidence in `reports/2026-08-28-unified-ir-m05.md` §0.5.

| # | Contract |
|---|---|
| **U1** | **Arc order and reference-slot order are semantic and must be preserved.** Reordering arcs within an edge, or references within an arc, changes program meaning. |
| **U2** | **The canonical hierarchy is `parent`/`children` in `Ir`.** The path `Index` becomes a **derived, versioned view**. A **coherence validator** is required for as long as both exist. |
| **U3** | **Duplicate incidences are distinct entities with distinct `HyperArcId`s.** Multiplicity is semantic. **Removal is by incidence ID**, never by pattern. |
| **U4** | **`ValueR::Num` preserves all IEEE-754 bit patterns.** Algebraic incidence/edge weights must be **finite**. **Negative zero is permitted and bit-preserved.** Canonical ordering uses **`total_cmp`**, never `partial_cmp`. |
| **U5** | **No language-level depth limit.** Recursive traversal must become **iterative before** the U2 migration. Any future depth cap is a configurable **resource** limit, outside language semantics. |
| **U6** | **Two identities.** An **O(delta), history-sensitive commit-chain digest** on every commit; a **history-independent canonical semantic hash** for checkpoint, audit and convergence. **Full `canonical_program_hash` is forbidden on the 100 Hz commit hot path.** |

### U2 inverts current behaviour — the largest single migration item

**[measured, M0.5]** `canonical_program_hash` today builds its skeleton forest from `Index` **path prefixes** and never reads `DeclNode.parent`, `first_child` or `next_sibling`. A dedicated test proves it: two IRs with deliberately contradicting intrusive hierarchies hash identically.

U2 makes the intrusive hierarchy authoritative and demotes the path index to a derived view. So the migration must **flip which structure is truth**, and while both exist a **coherence validator** must prove they agree:

```
validate_hierarchy_coherence(ir, index) -> Result<(), Incoherence>
    for every declaration d:
        path_parent(index, d)  ==  ir.decl_nodes[d].parent
    and every parent/child link is mutually consistent
    and the parent relation is acyclic
```

Three properties of this validator matter:

- It is **temporary by construction** — once the index is derived *from* the hierarchy, incoherence becomes unrepresentable and the validator becomes dead code. It should be deleted then, not kept as ballast.
- It must run on **lowering output**, not only on synthetic fixtures: the two structures are populated by different code paths today, so real programs are where a divergence would actually appear.
- **U5 is a hard prerequisite.** U2 puts the intrusive hierarchy on the traversal hot path, and that traversal is still the recursive one with a measured 215-level (debug) overflow boundary. Making a structure authoritative while its traversal can abort the process is not an acceptable intermediate state.

### U4's consequence for existing code

`total_cmp` is not a drop-in for `partial_cmp`. Every sort, comparison or dedup over `ValueR::Num` must be audited: `partial_cmp` returns `None` on NaN, so a sort built on it is not merely wrong but **unstable**, and an unstable sort feeding a digest makes the digest nondeterministic. Because U4 keeps NaN *representable*, this is reachable rather than theoretical. **No such audit has been performed** — recorded as migration work.

The `-0.0` rule has a matching consequence: digests must hash the **bit pattern**, not a normalised value, or `-0.0` and `+0.0` would collide despite being distinguishable under `total_cmp`.

### E.1 The choice that frames the rest

Two requirements pull in opposite directions:

| Requirement | Wants |
|---|---|
| Editing, transactions, distributed replication | **Stable identity** — a key that survives deletion and means the same thing on two machines |
| Compute: `HyperGraphView`, COO/CSR, GPU kernels, Laplacians | **Dense contiguous indices** — `0..n`, cache-friendly, uploadable to a GPU |

A GPU kernel cannot index by a generational key. A replica cannot address an entity by an array offset that shifts on deletion. **No single representation satisfies both**, and the current design's failure is that it tries: it uses dense offsets for everything and therefore cannot delete safely, cannot replicate, and cannot detect a stale view.

**Decision: stable-primary + dense-derived.** One authoritative store keyed by stable IDs; dense views *derived* from it, each stamped with the source version they were built from. This is not a compromise — it is the recognition that these are two different objects.

### E.2 Comparison of the four candidates

| Scheme | Deletion | Use-after-free | Replica-stable | Dense for compute | Memory under churn | Verdict |
|---|---|---|---|---|---|---|
| **Generational arena** (index + generation) | safe, slot reused | **detected** (generation mismatch) | no (slot order is local) | no | bounded | **primary store** |
| **Tombstones** | slot burned forever | not detected (slot still readable) | no | degrades — iteration must skip holes | **unbounded** under churn | rejected |
| **Monotonic ID + map** | safe | detected (absent from map) | **yes**, if allocation is deterministic | no — adds a lookup per access | bounded + map overhead | **distributed ID only** (§G.4) |
| **Stable-primary + dense-derived** | safe | detected | yes at the primary | **yes at the view** | bounded | **the architecture** |

**Tombstones rejected** on two grounds: memory grows without bound under edit churn, which the editor produces by design; and a tombstoned slot remains *readable*, so a stale ID silently returns the deleted entity's last state instead of faulting. That is strictly worse than a generational mismatch, which faults.

**Monotonic-ID-with-map rejected as the primary access path** — it puts a hash lookup in front of every arena access on the compile hot path — but adopted for the *distributed* identity in §G.4, where its replica-stability is exactly what is needed.

### E.3 One arena or several? — the decision

The question is really: where does the `decl → payload` mapping live?

Today it lives in **three dense parallel maps** (`decl_to_node`, `decl_to_edge`, `decl_to_arc`), which cost 960 048 B at 20 001 declarations, 54% of all derived state **[measured]**, and which are three of the five arrays nothing keeps in lockstep.

Three ways to remove them:

| Option | Shape | Cost |
|---|---|---|
| **(a) Single arena, enum payload** | `Vec<DeclRecord>` where the payload is `enum { Node{…}, Edge{…}, Arc{…} }` | Every record sized to the largest variant. `NodeRec` is 32 B, `EdgeRec` 56 B, `ArcRec` ~80 B **[measured]** — so every node pays ~80 B for 32 B of content. Destroys the struct-of-arrays layout §5 of `CLAUDE.md` requires for the hot numeric path. |
| **(b) Keep 3 maps, make them a rebuildable cache** | as today, but not authoritative | Keeps the 960 KB and keeps three arrays to desynchronise. Solves nothing structural. |
| **(c) Single declaration arena, inline payload reference, separate homogeneous payload arenas** | `DeclNode { kind, name, hierarchy…, anno, payload: PayloadIdx }` | One `u32` inline per declaration replaces three `Option<Id>` maps. |

**Decision: (c).**

```rust
// Authoritative. Homogeneous payload arenas preserve struct-of-arrays.
struct DeclNode {
    kind:    DeclKind,      // already exists — already the discriminant
    name:    SymId,
    parent:  DeclKey, first_child: DeclKey, last_child: DeclKey, next_sibling: DeclKey,
    anno:    AnnoR,
    payload: PayloadIdx,    // NEW: u32 index, interpreted per `kind`
}
```

Why (c) wins:

1. **The discriminant already exists.** `DeclKind { Node, Edge, HyperArc }` is stored today *and* re-encoded by which of the three maps is non-`None`. The maps are a redundant second copy of `kind`. Option (c) deletes the copy; options (a) and (b) keep paying for it.
2. **Three maps become one inline `u32`.** 3 × 16 B per declaration → 4 B. **[measured]** At 20 001 declarations that is 960 048 B → 80 004 B, and the saving is *structural* — the maps cannot desynchronise because they no longer exist.
3. **Struct-of-arrays survives.** Payload arenas stay homogeneous, so the numeric path keeps contiguous typed storage. Option (a) would force an 80-byte enum on every node and violate §5.
4. **Five lockstep arrays become two.** `decl_nodes` + `decl_hash`; `decl_to_*` are gone by construction, not by validation.

**`arc_hash` and `decl_hash` remain** as side-tables but are reclassified as **derived cache** (§E.5), not authoritative state — so a length drift is a cache miss, not corruption.

### E.4 `SymId` policy — deliberately separate

`SymId` is **not** an entity ID and must not be migrated with them.

| Property | Entity IDs (`DeclKey`, arc keys) | `SymId` |
|---|---|---|
| Allocation | positional — depends on insertion order | **content-addressed** — `intern("foo")` always yields the same id within a run |
| Deletion | supported; needs generation | interner is **append-only**; symbols are never removed |
| Needs generation counter | **yes** | **no** — nothing is ever freed, so use-after-free cannot occur |
| Replica-stable | no (see §G.4) | **achievable** — see below |

Because the interner is append-only and content-addressed, `SymId` stays a **plain dense index**. It gains nothing from generations and would pay memory for them.

**One consequence worth stating now:** `SymId` is *allocation-ordered* today (first-seen wins), so two replicas that intern the same strings in different orders assign different `SymId`s. Making the interner **content-ordered at snapshot boundaries** (sort by string, assign ids) would make `SymId` replica-stable without touching entity IDs — a cheap, independent win available at any time. It is **not** required for single-writer (§G.1) and is listed as a §G.4 prerequisite for multi-writer.

### E.5 Authoritative facts versus derived indexes

The line: **a fact is authoritative if losing it loses information**; it is derived if it can be rebuilt from authoritative facts alone.

| Authoritative | Derived (rebuildable, must be version-stamped) |
|---|---|
| `DeclNode`: kind, name, hierarchy links, `anno`, `payload` | `decl_to_node` / `_edge` / `_arc` — **eliminated** by §E.3 |
| Node payload: `bases` | `decl_hash`, `arc_hash` — content-derived caches |
| Edge payload: `bases`, owned arc list | name → declaration lookup index |
| Arc payload: `anno`, `in_edge`, `refs` (targets, signs, weights) | `HyperGraphView` incidence tables |
| `Ir.meta` | COO / CSR matrices, Laplacians |
| Interner contents | P-graph projections |
| — | GPU buffers |
| — | `doc_hash` (a function of everything above) |

**Rule:** derived state is never a `pub` field peers can write. It is produced by a constructor that takes the authoritative state and returns a value stamped with that state's version (§H). Today `decl_to_node` is a `pub` field written from two repositories — that is exactly the failure this rule prevents.

**One redundancy to resolve while here.** `EdgeRec.arcs: Vec<HyperArcId>` (forward) and `ArcRec.in_edge: DeclId` (back) encode the same relation with **mismatched types** **[measured]**. Make the **owned arc list authoritative** and `in_edge` derived: an arc's owner is discoverable from the owning edge, and one-writer beats two.

### E.6 Version-tagged dense views

Every dense view carries the identity of what it was built from:

```rust
struct ViewStamp {
    source_digest: [u8; 32],   // canonical digest of the Ir at build time
    version:       SnapshotVersion,
    built_from:    IrGeneration,  // cheap monotonic counter; fast-path check
}
```

Three fields because they answer three different questions at three different costs: `built_from` is an integer compare (the common case), `version` says *which axis* changed (§H.2), `source_digest` is the cryptographic answer for a replica that cannot trust a local counter.

This is the single largest gap in the system today: **no derived view records its source at all** (§H.1).

#### Structure and values must be separately owned

The envelope's 10× rate split (100 Hz non-topology against 10 Hz topology) is not satisfiable while a dense view is one object. A weight update at 100 Hz must not touch — or reallocate, or re-upload — the index buffers that only a topology change can invalidate.

So every dense view splits its storage along the axis boundary:

```rust
struct DenseView {
    structure: Arc<ViewStructure>,   // offsets, indices, incidence tables
                                     // stamped with the TOPOLOGY axis only
    values:    ValueArrays,          // weights, features, per-reference payloads
                                     // stamped with weights/features axes
}
```

- A **non-topology** commit clones nothing structural: it patches `values` in place (or publishes new value arrays sharing the same `Arc<ViewStructure>`). Cost is O(changed values).
- A **topology** commit rebuilds `structure` and republishes. Cost is O(graph), and the envelope's 100 ms sustained budget plus permitted coalescing is what makes that affordable.

Sharing `structure` by `Arc` is also what makes §F.7's snapshot publication cheap: a value-only commit publishes a new snapshot whose structure is the *same allocation* as its predecessor's. Without the split, every one of 100 commits per second would deep-copy the incidence tables.

---

## Part F — transactions

**Specification only. No implementation in Phase 0.**

### F.1 The problem to solve first

§4.6 of the audit: `apply` returns `Result<(), IRError>` and **discards the key `SlotMap::insert` returned**. A caller cannot learn the identity of what it just created. Consequently deltas cannot be gossiped, replayed, or referenced — the failure mode being *silent arity divergence*, not an error.

Every design decision below follows from fixing that.

### F.2 Transaction-local IDs

Within an open transaction, new entities are addressed by a **transaction-local ID** that is distinct *at the type level* from a committed key:

```rust
enum Ref {
    Committed(DeclKey),   // exists in the base state
    Local(LocalId),       // created earlier in *this* transaction
}
```

- `LocalId` is a transaction-scoped counter starting at 0. It is **deterministic**: the *n*-th creation in a transaction is always `LocalId(n)`, on every replica, regardless of local slot-reuse history.
- A delta may reference a `Local` only if that `LocalId` was created earlier **in the same transaction** — checkable during validation, and the check is what makes replay safe.
- At commit, the transaction emits a **binding map** `LocalId → DeclKey` in the receipt (§F.6). This is how the caller learns what it created — the thing `apply` cannot do today.

This makes a transaction **position-independent**: the same delta sequence applied to the same base state yields the same logical result on any replica, even though the physical keys differ.

### F.3 Validation overlay and infallible commit

Two phases, and the split is the point:

```
stage:    deltas → overlay (records intent; no mutation of the base IR)
validate: overlay + base → Result<ValidatedTransaction, TxnError>   ← every failure lives here
commit:   ValidatedTransaction + base → CommitReceipt               ← cannot fail
```

- The **overlay** holds created/modified/deleted entities and the `LocalId` bindings. The base `Ir` is untouched while a transaction is open, so a concurrent reader always sees a consistent state.
- **Validation** performs every fallible check: `Local` references resolve, `Committed` references still exist and have the expected generation, arc indices are in range, no dangling reference is created, the five §E.5 invariants hold on the *post-state*, and the hierarchy stays acyclic.
- **Commit is infallible by construction.** After validation the operation is a memory write. This is what makes atomic `Batch` free (§F.5) rather than something bolted on with clones.

`Ir::validate()` from the migration's M0.4 is the same predicate, reused — not a second implementation. **One state machine, per the Phase 8 rule.**

### F.4 Required delta operations

The editor's vocabulary, mapped onto full `Ir` semantics. Bold rows are **new capability the editor cannot express today**.

| Delta | References | Notes |
|---|---|---|
| `AddDecl { kind, name, parent: Ref, anno }` | parent | Returns a `LocalId`. Replaces `AddVertex`/`AddHyperEdge` — one op, `kind`-parameterised (§6.5 #1: the axis belongs in the payload, not the name). |
| `RemoveDecl { target: Ref, policy }` | target | `policy` explicit — see §F.5. |
| `SetParent { target, new_parent }` | both | **Hierarchy is unrepresentable in the editor IR today.** Must reject cycles. |
| `AddArc { edge: Ref, refs: Vec<SignedRef>, anno }` | edge, each target | **N arcs per edge**, the semantics the bridge collapses (§4.3). |
| `RemoveArc { arc: Ref }` | arc | |
| `SetArcRefs { arc, refs }` | arc | Replaces positional `UpdateSign`; see below. |
| `SetSign { arc, ref_index, sign }` | arc | Positional — **must carry the arc's expected ref count** as a precondition, or it inherits the §4.7 drift. |
| **`SetWeights { arc, ref_index, weights: Option<Vec<ValueR>> }`** | arc | Per-reference **vector** weights. The editor has only a scalar `f64` per *edge*. |
| **`SetBases { target, bases: Vec<SignedRef> }`** | target, each base | **Inheritance. Dropped entirely by the bridge** (§4.2). |
| `SetAnno { target, anno }` | target | Replaces `Attach`/`DetachAttribute`; **idempotent by construction**, unlike the current push/retain pair (§4.7). |
| `Batch { deltas }` | — | Atomic. §F.5. |

**`SetAnno` replaces attach/detach deliberately.** `AttachAttribute` pushes without a duplicate check and `DetachAttribute` removes *all* matches, so the pair is neither idempotent nor inverse (§4.7). A set-valued operation is both, which is a precondition for the merge analysis in §G.5.

**Presentation metadata does not appear here.** `position`, `Patch`, `patch_id` stay editor-side (§4.1). They are versioned separately or not at all; they must never enter the canonical digest, or moving a box on a canvas would invalidate every GPU buffer.

### F.5 Deletion, dangling references, and atomic `Batch`

**Deletion policy is explicit per delta.** The current implicit cascade (`RemoveVertex` silently `retain`s every edge's incident list, changing arity with no notification — §4.7) is not acceptable as a default:

| Policy | Behaviour | Use |
|---|---|---|
| `Reject` | Validation **fails** if anything still references the target. | **Default.** Loud. |
| `Cascade` | Removes referencing arcs/refs too, and **lists every cascaded removal in the receipt**. | Explicit bulk delete. |
| `Orphan` | Not offered. | Would recreate dangling references. |

Two audit findings this closes: `Patch.vertices` retaining dangling keys after `RemoveVertex`, and there being **no `RemovePatch` delta at all** (§4.7). Under `Reject`-by-default, neither can arise.

**Atomic `Batch`** is then trivial, and this is the payoff of §F.3:

> Stage every delta into the overlay. Validate the **whole batch** against the post-state. Commit infallibly. A batch either validates entirely or mutates nothing — no clone, no rollback path, no partial application.

Contrast with today's `for d in deltas { self.apply(d)?; }`, which has no rollback and leaves prior deltas applied (§4.5). The atomicity is not added; it falls out of validate-then-commit.

### F.6 Commit receipt, versions, canonical digest

```rust
struct CommitReceipt {
    base_version:   SnapshotVersion,
    target_version: SnapshotVersion,
    state_digest:   [u8; 32],          // canonical digest of the post-state
    txn_digest:     [u8; 32],          // content identity of the transaction itself
    bindings:       Vec<(LocalId, DeclKey)>,   // ← what §4.6 cannot provide
    cascaded:       Vec<DeclKey>,              // non-empty only under Cascade
    axes_touched:   AxisMask,                  // drives §H invalidation
}
```

`bindings` is the field whose absence breaks delta gossip today.

**Two digests, deliberately.** `txn_digest` identifies *the change* (already implemented, Phase 10B1 content-aware replay). `state_digest` identifies *the result*. Replay detection needs the first; view invalidation and replica authentication need the second. Conflating them would make a legitimate replay after resync indistinguishable from a conflict — the exact bug corrected in the Phase 10B1 corrective when sequence numbers were removed from the transaction digest.

**Version advance:** `axes_touched` sets which of `SnapshotVersion`'s axes increment. A weights-only edit must not bump `topology`, or every topology-derived view (CSR structure, GPU index buffers) rebuilds for nothing.

**Identity scheme — Merkle fold, required.** Define `state_digest` as a **Merkle fold over per-declaration subtree digests**, never a flat sweep:

- full computation = one pass over the whole graph;
- incremental update = recompute only the digests on the path from each changed declaration to the root — **O(changed × depth)**, independent of graph size;
- both yield **the same digest**, so a replica computing it fully and a writer computing it incrementally agree bit-for-bit. This equality is what lets a replica verify a writer's incremental result without trusting the increment.

**This is mandatory under the operating envelope, not optional.** At 100 Hz sustained the per-commit budget is 10 ms, and full-state identity alone was ≈9 ms at 10⁵ declarations **[contaminated]** — with the real `canonical_program_hash` costing more than that flat sweep. Only an update whose cost scales with the *change* rather than the *graph* fits.

Two design obligations follow, both load-bearing at 100 Hz:

1. ~~**The fold must be shallow, or depth must be bounded.**~~ **RESOLVED by M0.5 — this was the wrong worry.** Real declaration depth was measured across 96 lowered fixtures and 6 840 declarations: **median 3, p99 5, maximum 5**. At depth 5 the fold's O(changed × depth) term is ~5 digest recomputations per changed declaration — negligible. The concern this obligation raised is not the concern that materialised; see obligation 3.
2. **Annotation edits must be as cheap as weight edits.** Annotations sit in the 100 Hz bucket, so `SetAnno` must touch only its declaration's subtree digest and the path to the root — never trigger a structural recomputation.
3. **The whole-graph stages, not the fold, are the problem. [measured, M0.5]** `canonical_program_hash` costs **10.74 ms at 15 000 declarations** on a quiet host — the *entire* 100 Hz commit budget, at a graph 7× smaller than the 10⁵ target, and 26.9 ms at 160 000 incidences. Phase 0's flat-sweep proxy was ~12× optimistic per declaration, exactly as it warned it might be.

   The cost is not in the subtree fold. It is in `build_did_to_path`, `build_skeleton_forest`, the Prüfer encoding and the incidence sort — **every one of them a whole-graph pass**. So:

   > **A Merkle fold over subtree digests does not by itself bring identity into budget.**

   → **Decided (U6): two identities, with different jobs.**

   | | **Commit-chain digest** | **Canonical semantic hash** |
   |---|---|---|
   | Question it answers | "is this the same *sequence of changes* I already applied?" | "do two independently-built graphs *mean* the same thing?" |
   | Cost | **O(delta)** | O(graph) — measured 10.74 ms @ 15 000 decls |
   | History | **sensitive** — chains over the previous digest | **insensitive** — identical for equivalent histories |
   | Runs on | **every commit**, including the 100 Hz path | checkpoint, audit, convergence |
   | On the hot path? | yes | **forbidden** |

   Neither substitutes for the other, and that is the point. Replay and conflict detection need to know whether *this exact change* was already seen — inherently history-sensitive, and cheap because it folds only the delta into the previous digest. Convergence and audit need to know whether two replicas that took different routes arrived at the same graph — inherently history-**in**sensitive, and expensive because it must canonicalise the whole structure.

   **The M0.5 invariance results are what make the second identity trustworthy.** C1, C2, C3, C5, C6 and C10 each produced identical canonical hashes from IRs whose dense IDs genuinely differed, so "history-independent" is measured, not assumed. Without that evidence a convergence check built on this hash would be resting on hope.

   Two obligations this creates:

   - The commit-chain digest must commit to **enough** to be a real identity — the delta content, the base digest, and the resulting version — or two different change sequences could chain to the same value.
   - A checkpoint must record **both**, so a replica resuming from it can verify its own state canonically *and* continue the chain. Recording only the chain digest would make cross-replica verification impossible; recording only the canonical hash would break replay detection.

`canonical_program_hash` already computes per-subtree hashes (`compute_subtree_hashes_with_kinds`), so the fold ingredient exists — but note it is **recursive**, with a measured stack-overflow boundary of 215 levels (debug) / 458 (release). Against a real maximum depth of 5 the margin is 43–92×, so this is a latent hazard rather than a live one; no depth limit is enforced anywhere.

### F.7 Publication — immutable snapshots

**"Inference readers must never block on mutation or publication"** forbids any reader-visible lock, and forbids mutating in place anything a reader may be traversing. A `RwLock` is out: at 100 Hz a writer would contend with every inference pass, and a long pass would stall the writer.

**Mechanism:**

```
writer:  overlay → validate → commit → build snapshot → atomic publish
reader:  atomic load → Arc clone → hold for the whole pass → drop
```

- The published object is an **immutable snapshot**. Once visible it is never mutated — publication replaces a pointer, it does not edit a graph.
- Readers acquire by an atomic load plus one refcount increment. **No lock, no wait, and the writer's rate is invisible to them.**
- A reader holds its snapshot for the whole inference pass, so it sees **one consistent version throughout** — never half of an update. This is a correctness requirement, not just a performance one: a model reading topology from version *n* and weights from *n+1* would compute on a state that never existed.
- Reclamation is by refcount: the old snapshot is freed when its last reader drops it. A slow reader delays reclamation; it never blocks the writer.

**Value-only publication must be cheap, and the §E.6 split is what makes it so.** A non-topology commit publishes a snapshot that shares its predecessor's `Arc<ViewStructure>` — the incidence tables are the *same allocation*, not a copy. At 100 Hz, deep-copying structure per commit would dominate everything else.

**Two obligations this creates:**

1. **Bounded snapshot retention.** Readers holding snapshots keep them alive. At 100 Hz, a reader holding one for 10 s pins 1000 versions if each is distinct. Value-only versions share structure so the marginal cost is the value arrays — but that is not zero, and unbounded retention is a memory leak with a slow reader as its trigger. **The retention bound is a required, explicitly stated contract**, not an emergent property.
2. **GPU buffers must not be freed while in flight.** `hymeko_wgpu` already tracks `SubmissionEpoch` and a completion watermark for exactly this. Snapshot reclamation must respect it — **reuse that mechanism, do not add a second one** (Phase 8 rule).

**Writer-side back-pressure is out of scope but must not be designed out.** If a writer outruns reclamation the correct response is to coalesce (already permitted for topology) or to stall the *writer*, never to invalidate a snapshot a reader holds.

---

## Part G — distributed updates

**Specification only. No CRDT implementation in this phase.**

### G.1 Initial model: single writer, many replicas

One writer owns the canonical IR and produces a totally ordered stream of committed transactions. Replicas apply them in order and never originate changes.

This is deliberately the weakest useful model, and it is sufficient for the stated final system — an IR stream over IPC feeding a running inference — because that has exactly one authority. It also makes every §G.5 conflict question *deferred* rather than *answered wrongly*.

Guarantees: total order, deterministic replay, replicas converge bit-for-bit, and a replica can detect it is behind or forked (§G.3).

### G.2 Mapping `CommittedTransaction` to native `IRDelta`

Phase 10B1 already delivers the transport-side machine — content-aware replay, O(1) bounded replay window, conflict detection. **It must not be rebuilt** (Phase 8 rule). What is missing is only the payload interpretation:

```
CommittedTransaction {            →   native application
    stream_id, txn_id,            →   provenance, carried into the receipt
    kind: TransactionKind,        →   Snapshot ⇒ replace state; Delta ⇒ apply deltas
    base_version,                 →   precondition: must equal replica's current_version
    target_version,               →   postcondition: replica's version after apply
    digest: [u8; 32],             →   txn_digest — replay identity (already correct)
    sections: Vec<StagedSection>, →   decode → Vec<IRDelta>   ← THE MISSING STEP
}
```

The only genuinely new work is a **section codec for the delta vocabulary** (§F.4): section kind → delta type, with the same checked-arithmetic discipline as the Phase 10A envelope, and **never** casting wire bytes to a Rust struct.

Ordering: `sections` is a sequence, `LocalId`s are assigned in creation order (§F.2), so decoding is deterministic and the *n*-th created entity is `LocalId(n)` on every replica. The `bindings` map in the receipt then lets each replica resolve its own physical keys locally.

### G.3 `resuming_at` must carry the digest — a concrete gap

```rust
pub fn resuming_at(version: SnapshotVersion, limits: AssemblerLimits) -> Self {
    let mut a = Self::new(limits);
    a.current = version;        // ← current_commit_digest stays None
    a
}
```

**[measured]** A resumed consumer holds the version but not the digest that established it. Therefore:

> A replica resuming at version *V* receives a commit targeting *V*. Is it the same transaction it already applied (a legitimate replay after reconnect), or a **different** transaction that reached the same version (a fork)? **With only the version, these are indistinguishable.**

The current in-memory assembler avoids this because `current_commit_digest` is populated on the commit path — but it is `None` after every resume, which is precisely when replay is most likely.

**Required:** persist and restore the pair.

```rust
pub fn resuming_at(
    version: SnapshotVersion,
    commit_digest: Option<[u8; 32]>,   // the txn_digest that established `version`
    state_digest:  Option<[u8; 32]>,   // the resulting canonical state digest
    limits: AssemblerLimits,
) -> Self
```

Then on receiving a commit targeting the current version: equal `txn_digest` ⇒ `ExactDuplicate`; different ⇒ `ConflictingReplay`. `state_digest` additionally lets a replica verify it actually *holds* what it claims — catching silent local corruption that no version comparison can.

**This is a signature change to a Phase 10B1 API and requires its own approval.** Recorded here, not made.

### G.4 64-bit local generational versus namespaced 128-bit

| | Local generational (now) | Namespaced (future) |
|---|---|---|
| Layout | `{ index: u32, generation: u32 }` = 64 bit | `{ namespace: u32, actor: u32, counter: u64 }` = 128 bit |
| Allocation | positional; slot reuse | monotonic per actor; **never reused** |
| Same entity, two replicas | **different key** — needs the `bindings` map | **identical key** — self-describing |
| Cost | 8 B, niche-friendly, fits the compute path | 16 B, doubles every reference field |
| Multi-writer | impossible without coordination | the enabling primitive |

**Decision: 64-bit local generational now**, because single-writer (§G.1) is the target model and `bindings` already carries the translation. 128-bit is **not** premature-optimised into the design — but two properties are preserved so it stays reachable:

1. **Keys are opaque.** No consumer may assume a key is an array offset; that is the point of removing D4. A 64→128-bit widening then touches the type, not 196 call sites.
2. **Deltas reference `Ref`, not raw keys** (§F.2). The wire format already distinguishes `Committed`/`Local`, so an actor-namespaced variant is a third case, not a redesign.

**Prerequisite for the future step, recorded now:** `SymId` must become content-ordered (§E.4), or two actors will disagree on symbol identity even with namespaced entity IDs.

### G.5 Operation classification

For the future merge analysis. Under §G.1 every operation is totally ordered, so **this table is not yet load-bearing** — it is recorded so the vocabulary in §F.4 is not designed into a corner.

| Class | Operations | Why |
|---|---|---|
| **Commutative** | `SetAnno` on *distinct* targets; `SetWeights` on distinct arcs; `AddDecl` (distinct `LocalId`s, no ordering between creations) | Disjoint footprints. Order cannot matter. |
| **Conditionally mergeable** | `SetAnno` on the *same* target (last-writer-wins by commit order); `SetWeights` on the same arc; `SetParent` (mergeable **only if** the result stays acyclic — a global check, not local) | Deterministic tie-break needed; `SetParent`'s condition is not locally checkable. |
| **Conflicting** | `RemoveDecl` vs *any* operation on the target or its referrers; `AddArc` vs `RemoveDecl` of a referenced target; **`SetSign`/`SetArcRefs` vs anything changing arc arity**; `SetParent` cycles | Genuine conflicts requiring a policy decision. |

**The positional hazard, stated precisely.** `SetSign { arc, ref_index, .. }` addresses a reference *by position*. Any concurrent operation changing that arc's reference count silently changes which reference is addressed — the same defect as today's `UpdateSign` versus `RemoveVertex`'s `retain` (§4.7). **Mitigation, adopted into §F.4:** `SetSign` carries the arc's expected reference count as a validation precondition, converting a silent mis-target into a rejected transaction. This is why the operation is listed as conflicting rather than conditionally mergeable.

---

## Part H — derived views

### H.1 The current state: no view knows where it came from

**[measured]** Grepping the derived-view modules for `version|generation|dirty|invalidate|stale` returns **nothing**. `HyperGraphView::from_ir(ir, cfg, ex) -> Self` captures no reference, no version, no digest.

While `Ir` is immutable this is harmless: a view cannot go stale because its source cannot change. **The moment `Ir` becomes mutable, every derived view is silently stale with no mechanism to detect it** — and the views feed GPU buffers and inference. Under the stated final requirement (IR mutating while inference runs) this is the highest-severity gap in Part H.

### H.2 Source identity on every derived view

Every view carries a `ViewStamp` (§E.6). Applied:

| View | Location | Rebuild cost | Stamp |
|---|---|---|---|
| Compiler dense view (`decl_to_*` replacement) | `hymeko_core` | cheap, O(decls) | required |
| `HyperGraphView` | `hymeko_hnn` | **unmeasured** — §I.6 names it | required |
| COO / CSR | `hymeko_core::tensor` | unmeasured from IR | required |
| P-graph projection | `graph/hymeko_pgraph` | unmeasured | required |
| GPU buffers | `hymeko_wgpu` | upload-bound | required — **plus** the `SubmissionEpoch` already tracked, so a buffer is never freed while in flight |

Checking is tiered so the common case is an integer compare: `built_from` counter (ns) → `SnapshotVersion` axes (ns) → `source_digest` (only across a trust boundary).

### H.3 Invalidation by axis

`SnapshotVersion` already decomposes as `topology / weights / features / params`. Mapping the required axes onto it:

| Axis | Changed by | Invalidates | Survives |
|---|---|---|---|
| **Topology** | `AddDecl`, `RemoveDecl`, `AddArc`, `RemoveArc`, `SetArcRefs`, `SetParent`, `SetBases` | everything — CSR structure, incidence tables, GPU index buffers, P-graph | nothing |
| **Weights** | `SetWeights`, `SetSign` | CSR *values*, GPU value buffers | **CSR structure, index buffers, incidence tables** |
| **Features / values** | feature tensors, `ValueR` payloads | value buffers | all structure |
| **Annotations** | `SetAnno`, tags | **only** annotation-derived views (queries, presentation) | **all compute views** |

Two notes:

- **Weights-only edits must not disturb structure.** This is the single highest-value invalidation distinction, because weight updates are the frequent case during training and the structural rebuild is the expensive one.
- **`SnapshotVersion` has no annotation axis, and the envelope makes that a defect rather than an untidiness.** It has `params` (model parameters, dimension-indexed), which is not the same thing. With nowhere to record an annotation change, an annotation edit must conservatively bump a coarser axis — and annotations sit in the **100 Hz** bucket. Over-invalidating at 100 Hz would trigger spurious compute-view rebuilds a hundred times a second for edits that provably cannot affect any compute view.

  **Required:** add a fifth axis; do not overload `params`. This is a wire-format change, so it lands with the §G.3 `resuming_at` change under the same approval — but it is now a **correctness-of-invalidation requirement**, not a tidiness preference.

### H.4 Incremental refresh versus full rebuild

**The envelope settles the policy; measurement now sizes the implementation rather than choosing it.**

> **Full derived-view rebuild is permitted only on the topology axis.** Every non-topology axis — weights, features/values, annotations — must refresh incrementally.

This is forced arithmetic, not preference: non-topology commits arrive at 100 Hz sustained (10 ms budget) while topology commits arrive at 10 Hz with coalescing permitted (100 ms budget, and multiple deltas may be merged before materialization). A rebuild that fits comfortably in the topology budget would consume the entire non-topology budget ten times over.

The per-view consequences — **[hypothesis]** as to cost, **settled** as to policy:

| View | Axis | Expected | Why |
|---|---|---|---|
| CSR **values** | weights | **incremental** — scatter into the existing array | structure unchanged; indices already correct |
| CSR **structure** | topology | **rebuild** | sorted, prefix-summed; a single insertion shifts the tail |
| `HyperGraphView` | weights | incremental | flat weight array, positions stable |
| `HyperGraphView` | topology | rebuild | `from_ir` sorts and reduces pairs |
| GPU buffers | weights | incremental — sub-buffer write | avoids reallocation and epoch churn |
| GPU buffers | topology | rebuild + realloc | size changes |

**What measurement is still for.** §6.5 #18 forbids optimising before profiling — but that rule bars *guessing at a bottleneck*, not *implementing a stated requirement*. The requirement is given. What remains unmeasured is whether each incremental path is fast **enough**: a value scatter that fits in 10 ms at 10⁵ declarations is done, and one that does not needs a different data layout. Measurement sizes the implementation; it no longer chooses between incremental and rebuild.

The topology path needs its own measurement for a different reason: **coalescing is permitted, so the question is not "does one rebuild fit in 100 ms" but "how many topology deltas must be coalesced for the rebuild to keep up at 10 Hz sustained, 30 Hz burst."** That is a throughput question, not a latency one — §H.5.

### H.5 Two distinct metrics

Commit throughput and topology-materialization throughput are **decoupled by design** and must be budgeted, measured and reported separately. Conflating them hides the failure mode that matters.

| Metric | Question | Target | Path |
|---|---|---|---|
| **Commit throughput** | How fast can the writer validate, commit, update identity, and publish? | **100 Hz sustained / 250 Hz burst** (non-topology); 10 Hz / 30 Hz (topology) | overlay → validate → Merkle update → snapshot publish |
| **Topology-materialization throughput** | How fast can dense views be rebuilt and republished after topology change? | **10 Hz sustained / 30 Hz burst**, coalescing permitted | rebuild `ViewStructure` → republish |

They decouple because a non-topology commit publishes a snapshot sharing its predecessor's `Arc<ViewStructure>` (§E.6) and materializes nothing.

**Why reporting them together would mislead.** A single "updates/s" figure can look healthy while topology materialization lags without bound — commits keep landing at 100 Hz because they do no materialization, and the *derived views* fall progressively behind the authoritative state. Inference then reads a stale-but-internally-consistent snapshot and produces plausible wrong answers, with no metric showing distress. The lag between the newest committed version and the newest materialized topology is itself a required metric, and it is the one an operator should alarm on.

**Consequences for §I.6.** The measurement list must therefore cover both axes separately, and must add:

- **materialization lag** — committed version minus materialized topology version, sustained and worst-case;
- **snapshot retention depth** — live snapshots under a slow reader (§F.7's leak trigger);
- **declaration-tree depth distribution** — the input the Merkle fold's O(changed × depth) cost depends on (§F.6);
- **incremental value-patch cost at 10⁵ declarations** — the number that decides whether the 10 ms budget holds.

---

## M0.6 addendum — measured hash coverage, and what it changes (2026-08-28)

### The two existing hashes, and what neither covers

| | `canonical_program_hash` v1 | `compute_merkle_hashes` |
|---|---|---|
| Hierarchy source | **`Index` path prefixes** | **intrusive `decl_children`** |
| Versioned | `CanonHashCfg` | **none** |
| Covers annotations / values / weights | **no** | yes |
| Covers `bases` (inheritance) | **no** | **no** |
| Covers incidence target | yes | **no — ordering defect** |
| History-independent | **yes** (proved) | not established |

Two findings bind the v2 design:

1. **v1 is `V1-STRUCTURAL-ONLY`.** It covers kind, path, skeleton shape and ordered incidence topology; it does **not** cover annotations, values, weights or `bases`. It is frozen at `algo_version = 1` and **must not be used as the sole full-state convergence identity**. U6's second identity is `semantic_snapshot_hash` v2, which does not yet exist.
2. **`bases` is invisible to both hashes.** No identity in the repository covers inheritance today.

### `semantic_snapshot_hash` v2 — specification

Requirements: history-independent; ID-independent; **complete** over every semantic field; deterministic for all IEEE bit patterns; explicit about ordering (U1) and multiplicity (U3); built on the canonical hierarchy (U2); `algo_version`-tagged; incrementally maintainable.

Encoding: per-stage 8-byte **domain separation**; a 1-byte **field tag** before every field; `u64` **length framing** on every variable-length item; `f64` as **`to_bits().to_le_bytes()`** (never normalised — this is what keeps `-0.0` distinct and NaN payloads intact under U4); ordering, where required, by **`total_cmp`**.

**Incrementality is not free.** M0.6 made the subtree fold iterative and total cost moved ~4%, which shows the fold was never the bottleneck. Four **whole-graph** stages are: path reconstruction (eliminated by U2), skeleton construction (needs dirty-subtree marking), **Prüfer encoding** (inherently global — must be *replaced*, not patched), and incidence sorting (needs an incrementally maintained ordered index).

**Recommended: per-entity stable semantic digests plus order-independent keyed aggregation** — it drops the Prüfer stage entirely, which is what makes incrementality reachable, and costs O(changed × depth) with a measured depth of 5. **Falsifier:** if keyed aggregation cannot express U1's ordered slots, this fails and incrementally maintained authenticated ordered indexes become necessary. The mitigation is to encode slot index as a tagged field *inside* each incidence digest, so ordering never depends on aggregation order.

### Coherence: measured clean

`validate_hierarchy_index_coherence` (`ir/coherence.rs`, additive, read-only) reports **96 lowered / 96 clean / 0 diverged**. The intrusive hierarchy and the path `Index` already agree on the whole corpus, so the U2 authority flip would not move any current digest. It remains a v1-output change in principle and needs an `algo_version` decision rather than a silent switch.

### Traversal

The canonical subtree fold is **iterative** as of M0.6 — bit-identical output, no recursion proportional to depth, ~4% faster. Depth 16 384 verified clean; the remaining limit is memory, because **path storage is O(depth²)** — a further argument for U2, where parent links are O(1) per declaration.

---

## M2a addendum — the encapsulation boundary exists (2026-08-28)

`hymeko_core/src/ir/access.rs` provides the storage-independent read surface. Its contracts assume **no** dense ids, ordinal stability, absence of tombstones, or parallel maps — a generational or tombstoned arena satisfies all of them by skipping vacant slots, without a call-site change. Implementations index ordinally *inside* `Ir`, which is the boundary's purpose.

Iteration order is documented as deterministic for one snapshot and **not part of program semantics** — consistent with M0.5's finding that the canonical hash sorts before folding precisely because storage order is representational.

**Absence is `Option`, not error.** `node_for_decl` and `edge_for_decl` additionally reject a record whose `decl` back-link names a *different* declaration, so a malformed mapping yields absence rather than a silent wrong answer. `arc_for_decl` cannot do this: `ArcRec` has no own-declaration back-pointer (`in_edge` names the owning edge), so it checks presence only — a property of the record layout, recorded rather than papered over. Structural diagnosis stays in `ir/coherence.rs` and is not re-implemented per lookup.

**Zero-cost, measured (corrected by M2a.1, 2026-08-29):** scalar lookup is within noise of a hand-written guarded control on random-point *and* dependent-chain latency; iteration identical to raw `Vec` iteration; mapping within 3 %. No allocation on any valid-lookup path; invalid lookup O(1).

The M2a report initially recorded +20…49 % and attributed it to `Option<&T>` materialisation. That was wrong twice over: `Option<&T>` is niche-optimised (8 bytes), and the real cause was a redundant `is_none()` guard M2a itself introduced — `NONE` is `usize::MAX`, which `Vec::get` already rejects. Removing it cost 4 fewer instructions per lookup and closed the gap entirely (−32.7 %, p = 0.00). See `reports/2026-08-29-unified-ir-m2a1.md`.

## Platform-conformance addendum — 2026-08-30 (serialization roles, CBOR readiness, CLI)

Audit + gate only (`reports/2026-08-30-unified-ir-platform-conformance.md`); no
production source changed. This fixes the serialization-role vocabulary the Parts
above assumed but did not tabulate, and records the measured status of each.

### Serialization roles (binding)

| role | artifact | owner | status @ `cebc254` |
|---|---|---|---|
| source of truth | `hymeko_core::ir::Ir` (arena) | core | canonical, in-memory |
| resident human format | HyMeKo source text | core `parser` / `writers::ir_writer` | **PARTIAL** — `IrWriter` is skeleton-only (names + hierarchy; no annotations/values/bases/incidences; arcs stub). `UIR-EMIT-01/02` |
| compact lossless binary | CBOR `writers::cbor_writer::CborPayload` | core | **NOT IMPLEMENTED** — serde-derive schema only; no encoder, no bounded decoder; constructed nowhere; `ciborium` is transitive-via-Criterion, not a production dep. `UIR-CBOR-01..03` |
| transport framing | `ir_stream` (envelope/section/assembler/version/limits) | core | **implemented** — bounded, validated, no-transmute, `SnapshotVersion` authority, carries **no entity IDs**. `UIR-M2C-02` |
| UI/debug view | JSON `SnapshotDto` (in the `transformation` repo's `hymeko_formats`) | consumer | derived lossy view only; never canonical |

**Correction to any prior implication:** `CborPayload` is **not** a working codec.
It is a schema declaration. Do not describe it as a lossless binary snapshot until
both an encoder and a bounded, hash-verifying decoder exist and the semantic
round-trip `cbor_decode(cbor_encode(x)) ≡ parse(emit_hymeko(x))` is tested. The
proposed scope for that work is `APPROVED-CORE-EDIT: canonical-cbor-codec`
(conformance report §6). JSON must never be promoted to a canonical wire format;
the resident canonical serializations are HyMeKo text (human) and CBOR (binary,
once wired).

### Canonical CBOR implementation update — 2026-09-04

The table above is retained as the historical `cebc254` audit. Its CBOR status is
now superseded: Core exposes `encode_compiled`/`encode` and bounded `decode`, with
`ciborium = 0.2.2` as a direct production dependency. The decoder performs an
allocation-free definite-length preflight, reconstructs the interner and index,
validates IDs, paths, hierarchy, namespaces and aliases, and compares a recomputed
canonical hash before returning `DecodedCborPayload`. The public facade round-trips
all 105 lowerable frozen-corpus programs. Transport remains byte-opaque and JSON
remains a derived view. Evidence and limits are in
`reports/2026-09-04-canonical-cbor-codec.md` (`UIR-CBOR-01..05`). The ownership
model's layer 1 below is therefore text **PARTIAL**, CBOR **VERIFIED** as of this
update; its original 2026-08-30 status wording is historical.

### CLI

Target: one user-facing `hymeko` CLI with subcommands; the P-graph engine linked
as a library, not a second binary. Current: two product CLIs (`hymeko`, `pgraph`)
+ a debug CLI + a stub `main`. Inventory and consolidation plan in
`docs/unified-ir-single-cli.md`; conformance in `UIR-CLI-01..04`.

## Ownership & layering model — 2026-08-30 (Phase H, from measured evidence)

Derived from the conformance audit, not a new abstract framework. Each layer is
named with its current status so the model is falsifiable.

| # | layer | owner (crate) | status |
|---|---|---|---|
| 1 | HyMeKo source ⇄ CBOR are representations of one semantics | core `parser`/`writers` | text VERIFIED (skeleton); CBOR NOT_IMPLEMENTED |
| 2 | canonical entity state | `hymeko_core::ir::Ir` | VERIFIED (sole owner; accessor layer is the read contract) |
| 3 | immutable/versioned snapshots for concurrent readers | core (`Ir` clone; `ir_stream::SnapshotVersion`) | clone VERIFIED; version-bound snapshot publication NOT_IMPLEMENTED (M4) |
| 4 | derived-view builders own conversion + validation | hnn `HyperGraphView::try_from_ir` | VERIFIED (uses the accessor layer; rejects malformed) |
| 5 | CPU/Rayon/WGPU/CUDA/FPGA consume derived views | core `tensor`, `hymeko_wgpu` | CPU+WGPU VERIFIED; CUDA/FPGA absent |
| 6 | endpoint interactions → stable-ID semantic commands | — | NOT_IMPLEMENTED (needs stable IDs, M3) |
| 7 | only the core transaction validates & publishes | — | NOT_IMPLEMENTED (M4; design in Part F) |
| 8 | views rebuilt or incrementally refreshed from committed versions | hnn | rebuild VERIFIED; incremental refresh NOT_IMPLEMENTED (Part H.4) |
| 9 | one library façade + one product CLI | core | PARTIAL (`UIR-CLI-04`) |

### Abstraction gate (applied to any new type)

A trait/factory/repository/DTO/context object is admissible only if it names: the
concrete duplication it removes; ≥2 real consumers or strategies; ownership +
lifetime; the allocation boundary; the error contract; the concurrency contract;
static vs enum vs dynamic dispatch; and whether it belongs in core or a
derived/backend crate. This task admitted **none** — the audit found no
non-core duplication warranting a new abstraction (see
`docs/unified-ir-duplication-and-profile-audit.md`).

### Boundaries (binding)

- **Serialization vs transport:** serialization (HyMeKo text, CBOR) turns `Ir`
  ⇄ bytes; transport (`ir_stream`) frames/sequences/version-checks byte payloads
  and **carries no entity IDs** (`UIR-M2C-02`). A serializer never frames; the
  transport never interprets IR semantics.
- **Parallel-execution boundary:** parallelism lives in the *view/backend* layer
  (CSR `spmv_into`/`spmm_into` serial+Rayon, wgpu), never in canonical
  construction or the accessor layer. The Rayon path is bitwise-identical to
  serial by contract (`APPROVED-CORE-EDIT: csr-spmv-spmm-into`).
- **Single-CLI boundary:** library capability is exposed once as a library API and
  once as `hymeko` subcommands; a backend or consumer never ships its own product
  binary.
- **Forbidden dependency directions:** `hymeko_core` depends on nothing in the
  framework (only `parser` + `serde`); `hymeko_wgpu` → `hymeko_core` only, never
  `hymeko_hnn`; `hymeko_hnn` → `hymeko_core`; no consumer repo is a dependency of
  core; no derived view (COO/CSR/P-graph/Hotaru/GPU buffer) may become a second
  owner of canonical state.
