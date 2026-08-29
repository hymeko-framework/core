# Unified IR — migration (Phase 0, Part J)

**Date:** 2026-08-28 · **plan only, no production edits** · target in `docs/unified-ir-architecture.md`, evidence in `reports/2026-08-28-unified-ir-phase0-audit.md`

---

## J.1 The constraint, and the move that dissolves it

`hymeko_core` is a leaf; six crates across four repositories depend on it by `path`, one-way, at compile time. An ID or layout change in core is an instant compile break everywhere — **there is no intermediate state in which both old and new consumers build.** That is the flag day.

Naively this forces one synchronised commit across four repositories, with a coordinated revert if anything fails. For 196 dense-index sites, 23 FFI sites and a published JSON DTO, that is not an acceptable risk.

**The move: separate the two changes that were conflated.**

> The 196 dense-index sites are hazardous because they *couple call sites to the representation*. But converting `ir.decl_nodes[d.0]` into `ir.decl_node(d)` is **behaviour-preserving under the current representation** — it compiles, runs, and produces identical results today.
>
> So: **encapsulate first, while nothing changes. Then change the representation behind the encapsulation, touching zero call sites.**

This converts M1 — the widest, riskiest step, spanning four repositories — into a **core-local** change. There is no flag day because the representation change, when it finally happens, is invisible to consumers.

The ordering consequence is decisive and does **not** depend on any timing (correcting the earlier draft, which justified ordering partly on contaminated numbers):

```
M0   repairs             core-local
M0.5 ✅ DONE 2026-08-28  invariance + depth + quiet baseline
M0.6 iterative traversal core-local          ← U5; prerequisite for M2e
M2a  add accessors       core-local, purely additive
M2b  migrate call sites  per-repository, one at a time   ← the long tail
M2c  seal direct access  core-local; the point of no return
M2d  collapse maps       core-local
M2e  hierarchy authority core-local          ← U2; needs M0.6
M1   representation      CORE-LOCAL — because of M2b
M3   generational keys   core-local
M4   transactions        core-local + wire
M5   bridge removal      transformation
```

### M0.6 — make canonical-hash traversal iterative (U5)

`compute_subtree_hashes_with_kinds` recurses once per hierarchy level; the boundary is **215 levels (debug) / 458 (release)**, measured. Convert the fold to an explicit work-stack.

- **Behaviour-preserving.** The digest must be **bit-identical** before and after — the M0.5 invariance suite is the gate, plus a fixed-vector check on a real fixture.
- **No depth limit is added** (U5). Iterative traversal removes the stack ceiling; it does not impose a semantic one.
- **This is a production change to `canonical_hash.rs`** and needs §1 approval. M0.5 was explicitly forbidden from touching it.

**Why it is a prerequisite for M2e, not merely adjacent:** M2e makes the intrusive hierarchy authoritative, putting it on the traversal hot path. Making a structure authoritative while its traversal can abort the process at 215 levels is not an acceptable intermediate state.

### M2e — flip hierarchy authority (U2)

Today the canonical hash reads `Index` **path prefixes** and never reads `DeclNode.parent` — proven by M0.5. U2 inverts this: `parent`/`children` becomes canonical, the path `Index` becomes a derived versioned view.

1. Add `validate_hierarchy_coherence(ir, index)` and run it on **lowering output**, not only synthetic fixtures — the two structures are populated by different code paths, so real programs are where divergence would appear.
2. Fix any divergence found. **Expect some:** nothing has ever enforced agreement, so nothing guarantees it.
3. Switch the skeleton forest to read `parent`/`children`.
4. Derive the `Index` from the hierarchy and stamp it as a view (§E.6).
5. **Delete the validator.** Once the index is derived *from* the hierarchy, incoherence is unrepresentable and the validator is dead code. Keeping it would be ballast that implies a risk that no longer exists.

**Risk:** step 3 changes what the canonical hash reads. If any real program has a hierarchy that disagrees with its paths, its canonical hash **changes** — a stored-digest invalidation. Step 1 exists to find that before step 3, and its result decides whether a digest migration is needed.

---

## J.2 Stages

### M0 — repairs (core + transformation, independent)

No representation change; each item is independently revertable.

| Item | Where | Content |
|---|---|---|
| M0.1 | `transformation` | Make `IRDelta::Batch` atomic **or** correct its documentation. The type-level doc claims atomicity, the method-level doc denies it (audit §4.5). Interim recommendation: **fix the documentation now**, implement atomicity in M4 where validate-then-commit makes it free. Fixing it properly here would mean building a second transaction mechanism — exactly the duplication the Phase 8 rule forbids. |
| M0.2 | `core` | Delete the false `#[repr(C)] // Stable layout for serde and FFI` comment (D3). |
| M0.3 | `transformation` | Document the bridge's arena→editor losses: `bases`, `AnnoR`, `weights`, arc collapse, edge-target filtering (audit §4.2). Currently undocumented. |
| M0.4 | `core` | **`Ir::validate()`** — the §E.5 invariants as a checkable predicate. **Write it now, while `Ir` is still immutable and therefore still correct.** It is the oracle every later stage is tested against, and it is the highest-value item in the plan. |
| M0.5 | `core` | Add the **canonical-hash-invariance test** (audit §4.4): permute an editor IR's construction history, bridge both to `Ir`, assert equal `canonical_program_hash`. **This is a correctness test and needs no quiet host** — it can land immediately. If it fails, §4.4's hypothesis is refuted and M3's ordering assumptions need rework before, not after. |

**Rollback:** each item is one commit. M0.4/M0.5 are additive and may stay even if the rest reverts.

### M2a — accessors (core, purely additive)

Add the complete accessor surface to `Ir`; change no field, remove nothing.

```rust
impl Ir {
    fn decl_node(&self, d: DeclId) -> &DeclNode;
    fn decl_node_mut(&mut self, d: DeclId) -> &mut DeclNode;
    fn node_for_decl(&self, d: DeclId) -> Option<NodeId>;
    fn arcs_of_edge(&self, e: EdgeId) -> impl Iterator<Item = HyperArcId> + '_;
    fn push_declaration(&mut self, …) -> DeclId;   // the five-array lockstep, in ONE place
}
```

`push_declaration` matters most: it removes the second hand-written lockstep writer in `bridge.rs` (audit §2.3). Two implementations of an unenforced invariant become one — a §6.1 consolidation, not a new abstraction.

**Nothing breaks.** Consumers keep compiling unchanged.

**Rollback:** delete the additions.

### M2b — migrate call sites (per repository, independently landable)

The long tail: 196 dense-index sites → accessors. Order by risk, lowest first.

| Order | Repository | Sites | Note |
|---|---|---|---|
| 1 | `hymeko_core` | 52 | Same repo; validates the accessor surface before exporting the churn. |
| 2 | `hymeko_hnn` | 6 | Small; exercises the view-construction path. |
| 3 | `network`, `hotaru`, `graph` | 11 | Light coupling. |
| 4 | **`transformation`** | **127** | Largest. **`bridge.rs` first** — it is the only *writer*, so it must adopt `push_declaration` before anything else changes. |
| 5 | `hymeko_python`, `vscode-plugin` | surface | `raw()`-heavy already; mostly conformant. |

Each row is a separate commit in a separate repository, each independently revertable, each verified by the existing test suite plus `Ir::validate()`. **At no point is any repository uncompilable.**

**Rollback:** per repository, per commit. This is the whole reason for the split.

### M2c — seal direct access (core; the point of no return)

Only after M2b is complete everywhere:

- `Ir`'s arena and map fields become non-`pub` (or `pub(crate)`).
- **Remove `From<usize> for Id<T>` and `From<Id<T>> for usize`; remove `pub` from `Id`'s field** (defect D4).

> This step exists to make the remaining work **fail loudly**. While D4 stands, a representation change still *compiles* at any missed site and then indexes the wrong element — a silent wrong answer (audit §5.1c). Removing D4 converts every missed site into a compile error.
>
> **Sealing is a safety precondition of M1, not cleanup.** M1 must not begin until M2c is green.

**Rollback:** restore the impls. Cheap, and the failure mode is loud, so a missed site is found immediately rather than in production.

### M2d — collapse the derived mappings (core-local)

Now representation work begins, behind sealed accessors.

- Remove `decl_to_node`/`_edge`/`_arc` as stored state; replace with the inline `payload: PayloadIdx` of architecture §E.3.
- Reconcile `EdgeRec.arcs` ↔ `ArcRec.in_edge`: owned arc list authoritative, `in_edge` derived (§E.5).
- Reclassify `decl_hash`/`arc_hash` as derived cache.

**Gates:** `Ir::validate()`, full suite, and `ir_bench` — **on a quiet host** (audit §I.5). The plan *expects* construction to improve; a null result **falsifies the cost model** and the step is re-examined rather than waved through. The structural justification stands regardless: five lockstep arrays become two, and the elimination is by construction, not by validation.

**Rollback:** the removed maps are derivable by construction; restoration is mechanical and `Ir::validate()` proves it correct.

### M1 — ID representation (core-local, because of M2b)

- Payload `usize` → `NonZeroU32`; `NONE` becomes the niche (D2, D6).
- Type tag on `Serialize` (D1) — **wire-format change**, §J.4.
- `u32` caps declarations at ≈4.29 × 10⁹: state it as an explicit contract, or use `NonZeroU64` and forgo the niche saving.

**Rollback:** one commit, because M2b/M2c localised it. This is the stage the whole sequence exists to make safe.

### M3 — generational keys and mutability (core-local)

- Arenas → generational keys (§E.2). `Ord`-by-allocation (D5) is **removed**, not redefined.
- **Prerequisite:** triage the 580-entry order column (`docs/unified-ir-id-inventory.md` §4) and fix any site relying on ID order semantically. Traversal order feeds `canonical_program_hash`; M0.5's test is the safety net.
- Only now is `Ir` safely mutable. `Ir::validate()` becomes a post-mutation invariant in test builds.

### M4 — transactions (core + wire)

Architecture §F. Transaction-local IDs, validation overlay, infallible commit, atomic `Batch`, commit receipt. Attaches to the **existing** Phase 10B1 assembler — content-aware replay, bounded O(1) window, conflict detection — which must not be rebuilt.

The operating envelope (100 Hz non-topology / 10 Hz topology, non-blocking readers) makes four further items **required M4 scope**:

| M4.x | Item | Driver |
|---|---|---|
| M4.1 | **Merkle-fold state digest**, incrementally updatable, agreeing bit-for-bit with its full computation | 10 ms commit budget (§F.6) |
| M4.2 | **Immutable snapshot publication** — atomic publish, refcount reclamation, bounded retention, `SubmissionEpoch` respected for GPU buffers | readers must not block (§F.7) |
| M4.3 | **Structure/value split** in dense views, structure shared by `Arc` across value-only commits | 100 Hz without deep-copying incidence tables (§E.6) |
| M4.4 | **Incremental non-topology refresh**; full rebuild only on topology, coalescing permitted | §H.4 |

**Sequencing inside M4:** M4.1 first — it is the item most likely to invalidate the design.

**M0.5 has now measured the two inputs M4.1 needs, and the answer is not the one anticipated:**

| Question | Answer | Source |
|---|---|---|
| Is the declaration tree deep enough that O(changed × depth) hurts? | **No.** Median 3, p99 5, max 5 across 96 fixtures / 6 840 declarations. The fold's depth term is negligible. | M0.5 §5 |
| Does a Merkle fold bring identity into budget? | **Not on its own.** `canonical_program_hash` is **10.74 ms at 15 000 declarations** against a 10 ms budget — and the cost lives in the *whole-graph* stages (path map, skeleton forest, Prüfer, incidence sort), not the subtree fold. | M0.5 §6 |

**M4.1 is now decided, not open (U6).** Maintain **two identities**: an O(delta), history-sensitive **commit-chain digest** on every commit, and a history-independent **canonical semantic hash** for checkpoint, audit and convergence. **Full `canonical_program_hash` is forbidden on the 100 Hz commit hot path.** M4.1 therefore builds the chain digest and the checkpoint format; it does *not* attempt to make the whole-graph canonicalisation stages incremental.

Two further M0.5 results constrain it: the subtree fold is **recursive** with a 215-level (debug) stack boundary (→ U5, and see M0.6 below), and **arc/reference slot order is semantic** (U1) — so any incremental scheme must reproduce slot ordering exactly, and any rewrite that reorders arcs changes program identity.

Also includes the `resuming_at` signature change (§G.3) and the `SnapshotVersion` **annotation axis** (§H.3) — the latter now a correctness requirement, since annotation edits arrive at 100 Hz and without their own axis would over-invalidate compute views a hundred times a second. **Both are separate approvals**, recorded not made.

### M5 — bridge removal (transformation)

See §J.5 for the criteria.

---

## J.3 Compatibility adapters

| Boundary | Adapter | Retired when |
|---|---|---|
| **Rust API** | M2a accessors; deprecated field access via `#[deprecated]` during M2b | M2c |
| **`Id` representation** | The accessor layer *is* the adapter — no conversion shim needed, because no consumer sees the representation after M2c | never needed |
| **Serialization (D1 type tag)** | Envelope **version negotiation**: writer emits tagged, reader accepts tagged *or* untagged for one release | one release after M1 |
| **Persistence** (`ir_writer`, PGIP) | Format version field; reader supports N and N−1 | one release after M1 |
| **JSON DTO** (`snapshot::NodeDto`, `target_id: usize`) | **Currently versionless.** Add a `schema_version` field **in M0** — before it needs to change | permanent (keep the field) |
| **pyo3 ABI** (23 sites) | Keep IDs opaque in Python; expose `raw()`-style accessors, never arithmetic | permanent |

**The JSON DTO is the one to fix early.** It is a published, versionless wire format consumed by a browser and by Python. Adding `schema_version` while the schema has not yet changed is free; adding it during a breaking change is not. **This is the only adapter that belongs in M0.**

---

## J.4 When public ID contracts may change

| Contract | May change at | Requires |
|---|---|---|
| `Id<T>` in-memory layout | **M1** | M2c green (no consumer observes it) |
| `Id<T>` serialized form | **M1** | Envelope version bump + one release of dual-read |
| On-disk persisted IDs | **M1** | Format version + N−1 reader support |
| JSON DTO `target_id` | **after** `schema_version` ships (M0) | Version negotiation; browser and Python updated |
| pyo3 ABI | **M1**, with a version bump | Python package release note; IDs already opaque |
| `resuming_at` signature | **M4** | Separate approval (§G.3) |
| `SnapshotVersion` axes | **M4** | Separate approval; wire change (§H.3) |

**Rule:** an ID contract may change only when *every* observer of it is either (a) sealed behind an accessor, or (b) able to negotiate a version. Nothing changes on the strength of "we updated all the callers we know about" — the JSON DTO and the pyo3 ABI have observers outside this workspace.

---

## J.5 Bridge-removal criteria

`bridge.rs` exists to reconcile a mutable editor model with an immutable compiler model. **M3 removes that reason.** But removal is gated, not assumed — all of:

1. **M3 complete** — `Ir` is mutable with stable keys.
2. **M4 complete** — the native delta vocabulary (§F.4) covers every `IRDelta` the editor issues, *including* the three the editor cannot currently express: hierarchy (`SetParent`), per-reference vector weights (`SetWeights`), and inheritance (`SetBases`).
3. **Presentation metadata has a home.** `position`, `Patch`, `patch_id` are classified presentation (audit §4.1) and must **not** enter the canonical IR or its digest — moving a box on a canvas must not invalidate a GPU buffer. They need an editor-side sidecar keyed by canonical ID *before* the bridge goes.
4. **`level` and `attributes` are canonicalised.** Both classified canonical (§4.1): `level` is a G-SPHF semantic property, `attributes` maps onto `AnnoR`. Neither has an arena home today. Until they do, deleting the bridge *loses editor data*.
5. **The arc-collapse defect cannot recur.** The editor must express N arcs per edge (audit §4.3), and a test must assert `arcs.len()` across a round trip — the assertion the current round-trip test omits.
6. **Canonical-hash invariance holds** (M0.5).

Until all six hold, the bridge stays and is *documented* (M0.3), not deleted. **Deleting it early would silently lose `level`, `attributes`, patches and layout** — trading a documented loss for an undocumented one.

---

## J.6 Rollback points and gates

| After | Rollback | Gate to proceed |
|---|---|---|
| M0 | one commit each | suite green; M0.4 and M0.5 pass |
| M2a | delete additions | suite green; no consumer touched |
| M2b (per repo) | per-repo revert | that repo's suite green; `Ir::validate()` |
| **M2c** | restore `From` impls | **all** repos green — the point of no return |
| M2d | maps are derivable; restore mechanically | `validate()` + **quiet** `ir_bench` (§I.5) |
| M1 | one commit (localised by M2b/M2c) | quiet bench; dual-read verified |
| M3 | one commit | order column triaged; M0.5 passes |
| M4 | feature-gated until green | 10B1 suite + new transaction suite |
| M5 | restore the bridge | all six §J.5 criteria |

**Two standing gates.** No stage that claims a performance result may cite a measurement taken while PID 4196 (or any unexplained load) is consuming CPU — audit §I.5 defines the quietness check and requires ambient load recorded with every run. And no stage may be reported complete without its report (§9 of `CLAUDE.md`).

---

## J.7 What this plan deliberately does not do

- **No CRDT / multi-writer merge.** §G.1 is single-writer; §G.5 is recorded so the vocabulary does not foreclose it.
- **No `SymId` change** in the entity-ID migration. Content-ordering is a separate, independently landable win (§E.4).

**Two items left this list on 2026-08-28**, when the commit-rate targets were given. Both are now **required M4 scope**, not deferred:

- **Incremental / Merkle state identity.** At 100 Hz sustained the commit budget is 10 ms and full-state identity alone was ≈9 ms **[contaminated]**, with the real canonical hash costing more. Architecture §F.6.
- **Incremental non-topology view refresh.** Full rebuild is permitted only on the topology axis; weights, features and annotations must refresh incrementally. Architecture §H.4.

This is not a reversal of the §6.5 #18 discipline. That rule bars optimising against a *guessed* bottleneck; here the rate is a stated requirement, and the remaining measurement sizes the implementation rather than choosing it.
- **No estimates in wall-clock days.** The earlier draft gave day figures; they rested partly on contaminated timings and on an unknown — how fast 127 sites in a repository this phase never built can be migrated. The stage *ordering* and *gates* are the plan; scheduling needs a quiet baseline and one measured pilot repository first.

---

## M0.6 status (2026-08-28)

**M0.6 is complete.** `refactor(ir): make canonical traversal iterative`.

- Recursive subtree fold → iterative post-order. Output **bit-identical**, guarded by 5 frozen vectors and a 96-fixture corpus digest. ~4% *faster*, not slower.
- Depth 10 000 and 16 384 hash cleanly (previously fatal at 458). **U5 satisfied**, so its prerequisite on M2e is discharged.
- `ir/coherence.rs` added (additive, read-only). Corpus: **96/96 clean, 0 diverged**.

### M2e is no longer blocked by coherence — but is not unconditional

The M2e text above predicted divergence and told step 2 to "fix any divergence found. **Expect some.**" **That prediction was wrong**: agreement holds across the entire corpus. Step 2 has nothing to repair, and the digest-migration risk step 1 was meant to expose does not materialise for current programs.

What remains before M2e:

1. **§1 approval** — it changes production hashing.
2. **An `algo_version` decision.** Switching the skeleton source from paths to links is a v1-output change *in principle* even though no current fixture moves. It must be a deliberate version bump or a documented no-op, never a silent switch.
3. The validator should be **deleted** when the index becomes derived, as originally specified — it is temporary by construction.

### M2a may begin

Nothing in M0.5 or M0.6 blocks the accessor layer. It is additive, core-local, and its call-site migration (M2b) is unaffected by the hashing work.

---

## M2a status (2026-08-28)

**M2a is complete.** `feat(ir): add storage-independent read accessors`.

The stage description above anticipated an accessor set including `push_declaration`. **That part was not built and is not part of M2a**: `push_declaration` is a *mutating* API, and M2a's brief explicitly forbids mutable accessors. Consolidating the second lockstep writer in `bridge.rs` therefore moves to the first stage that may add mutation — M2d or M4. The read surface is complete without it.

Two requested read methods were **not added** because equivalents already existed and were already safe (`decl_node`, `as_node`/`as_edge`/`as_arc`). Adding them under new names would have been cosmetic duplication.

**Cost, measured rather than assumed:** scalar lookup through an accessor is **+20 % to +49 %** against a hand-written guarded control — about **0.17 ns per lookup** in absolute terms — while iteration is **within noise**. It is *not* zero-cost, and M2b should not claim it is. The absolute impact on real workloads is small: `canonical_program_hash` is 10.3 ms at 15 000 declarations and lookup is a minor fraction of it.

**M2b may begin.** The mapping table in `docs/unified-ir-consumer-map.md` is complete for every read path in the 196-site inventory.

---

## M2a.1 status (2026-08-29)

**M2a.1 is complete.** `perf(ir): optimize storage-independent accessors`.

The M2a cost caveat above is **withdrawn**. Scalar lookup through an accessor is now **zero-cost**, measured on random point lookup and on a dependent chain that removes load overlap; iteration is identical to raw `Vec` iteration; mapping is within 3 %.

The +20…49 % M2a reported was a **redundant sentinel guard M2a itself introduced**, not an inherent abstraction cost — `DeclId::NONE` is `usize::MAX` and `Vec::get` already rejects it, so the explicit `is_none()` check cost 4 extra instructions per lookup and pushed LLVM into a branchless `setne`/`setb`/`testb` chain instead of one predicted branch. Removing it: **−32.7 % at 1 000, −18.3 % at 10 000, p = 0.00**.

### M2b migration rules (binding)

1. **Dense scans use the iterators** — `decls()`, `node_records()`, `edge_records()`, `arc_records()`. Measured identical to raw `Vec` iteration.
2. **Point lookups use the accessors** — `decl_node`, `node`, `edge`, `arc`. Measured zero-cost.
3. **Mapping-only migration uses `as_node`/`as_edge`/`as_arc`.** Do **not** substitute `node_for_decl` mechanically — it also resolves the record and validates the back-link, which is more work and a different contract.
4. **Do not** collapse repeated accessor lookups into a hidden dense iterator. The contracts are deliberately distinct.

**M2b may begin on performance grounds.**

---

## M2b status — first stage complete (2026-08-29)

**This repository's read-only consumers are migrated.** `test(ir): establish unified IR V&V baseline`, `refactor(ir): migrate core read consumers to accessors`, `refactor(hnn): migrate IR reads to canonical accessors`.

| Consumer | Sites before | after |
|---|---:|---:|
| `util/pretty_print.rs` | 6 | 0 |
| `writers/ir_writer.rs` | 3 | 0 |
| `hymeko_hnn/.../hypergraphview.rs` | 7 | 0 |

**Unexplained remaining: 0.** Retained direct access, each with a category: `ir/ir.rs` (storage implementation), `ir/lower.rs` (construction), `ir/canonical_hash.rs` and `ir/hash_pass.rs` (representation-owner algorithms), `ir/coherence.rs` (must read raw storage to *detect* incoherence), serde derives, and malformed-state tests.

`generators.rs` appeared in the grep with 10 hits and is **not an IR consumer** — `self.edges` there is the generator's own `Vec<Vec<usize>>`. Counting it would have inflated the inventory.

**Performance:** `HyperGraphView::from_ir` measured **+0.18 %** by alternating pre/post source on one quiet host, three rounds. Criterion's first reading (+21.3 % vs a stale stored baseline, taken while ambient load rose to 30 %) did not reproduce.

**A durable V&V structure now exists** at `hymeko_core/tests/unified_ir/{support,verification,validation}` with requirements and traceability under `docs/verification/unified-ir/`. Phase-numbered test directories are gone; provenance moved into documentation.

**Next M2b repository: `../network`.** Entry condition: this repository pushed and green; `network` clean and non-diverged; its dependency path resolving to the published core.
