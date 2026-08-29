# Unified IR — dynamic hypergraph test architecture (M0.5, Parts G–I)

**Date:** 2026-08-28 · **specification only — no implementation in this phase**
**Companions:** `reports/2026-08-28-unified-ir-m05.md` (evidence), `docs/unified-ir-architecture.md` (target), `docs/unified-ir-migration.md` (sequence)

This document specifies the tests that must exist *before* mutable IR, transactions and stable IDs are implemented. It defines them; it does not build them.

---

## 0. The reference model

Every property below is checked against an **independent slow model**, not against the implementation's own machinery.

```rust
// Deterministic, ordered, obviously-correct. Not performance-sensitive.
struct RefModel {
    decls:     BTreeMap<EntityId, RefDecl>,      // kind, name, parent, anno, value
    incidences: BTreeMap<(EdgeRef, ArcIdx), Vec<RefIncidence>>,  // sign, target, weights
    live:      BTreeSet<EntityId>,
    dead:      BTreeSet<EntityId>,               // retired ids, for stale-ID checks
    version:   SnapshotVersion,
}
```

**Binding constraints:**

- It must **not** reuse `StableArena`, the production transaction machinery, or any production delta application. A model that shares the implementation agrees with it by construction and proves nothing — this is the same rule the Phase 10B1 corrective applied to its assembler model.
- It must use `BTreeMap`/`BTreeSet` throughout: iteration order is part of what is being asserted, and a `HashMap` would make the model's own output order-unstable.
- It may be O(n) where the implementation is O(1). Slowness is the price of obviousness.

---

## Part G — dynamic-hypergraph test specification

### G.1 Entity lifecycle

| Test | Assertion |
|---|---|
| add node / edge / incidence | returns an id; model and implementation agree on live set |
| lookup by id | returns the entity just added |
| remove | id leaves the live set, enters the dead set |
| **slot reuse** | after remove + add, a *new* id may occupy the same slot index |
| **generation change** | the reused id differs from the retired one in its generation field |
| **stale-ID rejection** | the retired id resolves to `None` — never to the entity now occupying its slot |
| unaffected-ID stability | every other id still resolves to the same entity |
| serde / wire / FFI round trip | ids survive; a serialized id deserializes to an id that resolves identically |

> **The single most important test in this document is stale-ID rejection.** A dense-index scheme with slot reuse and no generation silently aliases a retired id onto its replacement, and every downstream reference then points at the wrong entity with no error. It must be tested directly, not inferred from the presence of a generation field.

### G.2 Referential integrity

| Test | Expected |
|---|---|
| remove an unreferenced node | succeeds |
| remove a **referenced** node | **rejected** — see policy below |
| explicit cascade removal | succeeds; receipt lists every cascaded removal |
| remove a hyperedge | its owned incidences are removed **atomically** with it |
| after any removal | no dangling hierarchy or incidence reference is observable |
| wrong entity kind (e.g. `remove_node` on an edge id) | rejected, distinct error |

**Proposed policy — subject to implementation review:**

- `remove_node(id)` **rejects** when the node is referenced.
- `remove_node_cascade(id)` is the explicit opt-in.
- Removing a hyperedge atomically removes its owned incidences.

This is a deliberate change from today's editor IR, where `RemoveVertex` silently `retain`s every edge's incident list — changing edge arity with no notification — and does **not** clean `Patch.vertices`, leaving dangling keys (M0.5 report §D). Reject-by-default makes the silent case impossible; cascade keeps it available where wanted.

### G.3 Transactions

| Test | Assertion |
|---|---|
| successful multi-operation commit | all effects visible atomically; version advances once |
| invalid operation in the middle | whole transaction rejected |
| **failed commit leaves the IR unchanged** | byte-identical to the pre-transaction state, model-compared |
| explicit abort | same as failure: nothing applied |
| transaction-local IDs | a delta may reference an entity created earlier in the same transaction |
| committed pending IDs | appear in the receipt's `bindings` map and resolve afterwards |
| **aborted pending IDs never become valid** | the `LocalId` resolves to nothing, now and later |
| `Batch` delegates to the transaction mechanism | not a second code path — one state machine |
| commit receipt | carries base/target version, `txn_digest`, `state_digest`, bindings |

> `Batch` delegating rather than reimplementing is a **structural** assertion, not a behavioural one. Today's `IRDelta::Batch` is a bare `for` loop with no rollback (M0.5 report §D), which is exactly the second mechanism this test forbids.

### G.4 Mutation operations

For **every** operation: success; missing/stale ID; wrong entity kind; invariant violation; atomic failure; hash/version effect; derived-view invalidation.

| Operation | Axis | Notes |
|---|---|---|
| `AddNode` | topology | returns `LocalId` |
| `RemoveNode` | topology | reject-if-referenced (§G.2) |
| `AddHyperEdge` | topology | |
| `RemoveHyperEdge` | topology | owned incidences removed atomically |
| `AddIncidence` | topology | |
| `RemoveIncidence` | topology | |
| `UpdateIncidenceSign` | **topology** | see the sign note below |
| `UpdateIncidenceWeights` | weights | the 100 Hz path |
| `RenameDeclaration` | topology | changes the path, therefore the canonical hash |
| `MoveDeclaration` | topology | must reject cycles |
| `AttachAnnotation` | annotations | must not invalidate compute views |
| `DetachAnnotation` | annotations | idempotent — see below |
| `SetValue` | features/values | |
| `Batch` | union of members | |

**Two operations need their axis argued, not assumed:**

- **`UpdateIncidenceSign` is topology, not weights.** Sign is three-valued (`Plus`/`Minus`/`Neutral`) and participates in the canonical incidence bytes; a sign flip changes signed-Laplacian *structure*, not just a coefficient. Classifying it as a weight update would let a 100 Hz path silently skip a structural rebuild. If a future signed view proves sign-agnostic in its structure, this can be revisited **with a test**, not by assumption.
- **Annotation operations must be idempotent and inverse.** Today's `AttachAttribute` pushes without a duplicate check while `DetachAttribute` removes *all* matches, so attach·attach·detach ≠ attach. The replacement (`SetAnno`, set-valued) must be tested for idempotence explicitly — it is a precondition for the commutativity claims in §G.7.

### G.5 Hierarchy

| Test | Expected |
|---|---|
| reparent | new parent observed; old parent loses the child |
| rename | path changes; canonical hash changes |
| sibling uniqueness | duplicate sibling name rejected |
| **cycle rejection** | a reparent that would create a cycle is rejected, and the IR is unchanged |
| root rules | a root has no parent; a decl cannot become its own root |
| deep hierarchy | see the depth bound below |
| remove non-empty subtree | rejected without cascade; permitted with it |
| explicit cascade semantics | receipt lists every removed descendant |

> **Depth bound — measured, and lower than expected (U5: no language-level limit; the recursion is the defect, and M0.6 makes traversal iterative before the U2 authority flip).** M0.5 measured the canonical hash's recursion limit at **215 levels (debug) / 458 (release)**, against a real-corpus maximum of **5**. Hierarchy tests must therefore stay well under 100 in-process: a stack overflow is an *abort*, not a catchable panic, and takes the whole test binary with it. Deep cases run as a child process. This is not hypothetical — the M0.5 depth test tripped it at depth 1000 and killed 150+ unrelated tests.

### G.6 Incidences

| Test | Expected |
|---|---|
| `Plus` / `Minus` / `Neutral` | round-trip; `Neutral` is never conflated with `Plus` |
| weighted / unweighted | `None` weights and `Some(vec![])` are distinguishable |
| **duplicate incidence policy** | **UNRESOLVED — see below** |
| multiple incidences between the same entities | permitted; each addressable |
| **NaN / Inf weight policy** | **UNRESOLVED — see below** |
| empty / multiple weight values | a weight is `Vec<ValueR>`, not a scalar |
| ordering determinism | two equal-content IRs produce identical incidence ordering |
| **arc order within an edge (U1)** | **semantic** — permuting arcs changes the canonical hash |
| **reference slot within an arc (U1)** | **semantic** — permuting slots changes the canonical hash |

**Both policies are now decided (2026-08-28).**

**U3 — duplicate incidences are permitted as distinct entities with distinct `HyperArcId`s. Multiplicity is semantic. Removal is by incidence ID.**

Consistent with what the hash already does: it writes the slot index, so it already distinguishes duplicates. Binding consequences for the tests:

| Test | Assertion |
|---|---|
| add two identical `(sign, target)` incidences | both exist, with **different** `HyperArcId`s |
| the canonical hash | **differs** from the single-incidence graph — multiplicity is meaning |
| `RemoveIncidence(id)` | removes **exactly** that one; the duplicate survives |
| `RemoveIncidence` by pattern | **must not exist in the API** — it would delete an arbitrary member of a multiset |

**U4 — `ValueR::Num` preserves all IEEE-754 bit patterns; algebraic incidence/edge weights must be finite; negative zero is permitted and bit-preserved; canonical ordering uses `total_cmp`, never `partial_cmp`.**

Note this is *not* the blanket rejection earlier drafts recommended. It separates two roles: the value carrier stays bit-faithful, the algebraic weight position is validated finite.

| Test | Assertion |
|---|---|
| `ValueR::Num(NaN)` as a general value | round-trips **bit-identically** through serde and the digest |
| NaN in an **algebraic weight** | **rejected at validation**, transaction unchanged |
| `+Inf` / `-Inf` in an algebraic weight | rejected |
| `-0.0` weight | **accepted, and distinguishable from `+0.0`** in ordering and digest |
| ordering over mixed values incl. NaN and `-0.0` | **total and deterministic** under `total_cmp`; repeated sorts are stable |
| digest of `-0.0` vs `+0.0` | **differs** — hash the bit pattern, never a normalised value |

> **A regression test is owed here.** `partial_cmp` returns `None` on NaN, so any existing sort built on it is not merely wrong but *unstable* — and an unstable sort feeding a digest makes the digest nondeterministic. Because U4 keeps NaN representable, this is reachable rather than theoretical. **The `partial_cmp` audit has not been performed**; it is migration work, and the test above is what proves it done.

### G.7 Derived views

Views: `HyperGraphView`, edge-major and node-major incidence, COO, CSR, clique/star expansion, GPU resources.

| Test | Assertion |
|---|---|
| source version propagation | every view carries the `SnapshotVersion` and digest it was built from |
| stale-view detection | a view built from version *n* is detectably stale against *n+1* |
| **weight-only refresh** | structure buffers are **not** rebuilt; identity of the structure allocation is preserved |
| feature/value refresh | value arrays updated; structure untouched |
| **topology invalidates both orientations** | edge-major *and* node-major rebuilt; neither silently retains stale indices |
| removed elements never appear | a removed node is absent from every view after refresh |
| stable↔dense mapping is deterministic | the same IR always yields the same dense ordering |

> Weight-only refresh preserving the *structure allocation identity* is the assertion that actually proves the §E.6 split works. Checking only that values are correct would pass even if the whole view were rebuilt — which is the thing the 100 Hz budget forbids.

### G.8 Distributed / replay

| Test | Assertion |
|---|---|
| exact transaction replay | recognised as duplicate; state unchanged |
| conflicting same-version transaction | reported as conflict, never silently merged |
| stale base | rejected with the expected-versus-actual version |
| sequence gap | detected; resync requested |
| snapshot recovery | replica restored from snapshot + digest matches the writer |
| **commit digest persistence** | after `resuming_at`, a same-version replay is still distinguishable from a fork |
| single-writer replica convergence | replicas converge bit-for-bit on identical streams |
| commuting operation permutations | permuted orders converge to the same state |

> Commit-digest persistence is the gap M0.5 confirmed in code: `resuming_at` sets only the version, leaving `current_commit_digest` `None`, so a resumed replica cannot tell a legitimate replay from a fork. The test must cover the **resumed** path specifically, because the in-memory path already works.

---

## Part H — model-based mutation testing

### H.1 Generator

Deterministic, seeded, no external dependency (the Phase 10B1 corrective's LCG pattern). It emits **both legal and illegal** operations — illegal ones are how rejection paths get covered — over a shrinking-friendly representation.

### H.2 After every commit, compare against the model

live entities · hierarchy · incidences · signs · weights · annotations · values · stale-ID results · versions · canonical hash · derived-view contents.

### H.3 Required properties

| # | Property |
|---|---|
| P1 | **No panic** for any generated sequence, legal or illegal |
| P2 | A failed transaction changes **nothing** |
| P3 | Version **never regresses** |
| P4 | A stale ID **never aliases** a replacement entity |
| P5 | No dangling reference is **observable** |
| P6 | Equivalent committed histories **converge** |
| P7 | Non-commuting histories **report conflict** |
| P8 | Memory stays **bounded** under churn after reclamation |

P8 needs a stated reclamation point: with snapshot publication (architecture §F.7) a snapshot lives until its last reader drops it, so "bounded" means *after* readers release — the test must drop them explicitly and then assert, or it will measure reader lifetime instead of a leak.

### H.4 Shrinking

On failure, minimise on four axes before reporting: **operation count** (drop operations that do not change the verdict), **entity count**, **value magnitude** (weights toward 0/1), and **sequence prefix** (find the earliest divergence). Report the minimised sequence, the seed, and the first diverging field. A raw 10 000-operation failing trace is not a bug report.

---

## Part I — future dynamic performance matrix

**Specification only. Do not implement these benchmarks yet.**

### I.1 Axes

- **Batch sizes:** 1, 10, 100, 1 000 operations.
- **Rates:** values/weights/features **100 Hz sustained, 250 Hz burst**; topology **10 Hz sustained, 30 Hz burst**.
- **Workloads:** 90% read / 10% write · add-remove churn · repeated slot reuse · deep-hierarchy edits · high-cardinality edge mutation · snapshot publication while inference readers run.

### I.2 Metrics

Commit **p50/p95/p99** (not means — the burst budget is a tail property) · validation · allocation · Merkle update · derived refresh · publication · **reader pause** · memory fragmentation · resident inference regression.

### I.3 Required future gates

1. Readers **never** observe partial commits.
2. The inference hot path receives **no mutation branch**.
3. **No full topology rebuild on value-only updates.**
4. Snapshot lifetime is safe for in-flight CPU **and GPU** work (reuse `SubmissionEpoch`; do not add a second mechanism).

### I.4 The measurement that already invalidates a design assumption

M0.5 measured `canonical_program_hash` on a quiet host: **10.74 ms at 15 000 declarations**, 26.9 ms at 160 000 incidences.

The 100 Hz non-topology budget is **10 ms per commit**. Full-state canonical hashing therefore **already exceeds the entire budget at 15 000 declarations** — a graph roughly 7× smaller than the 10⁵ target. Phase 0's flat-sweep proxy suggested ~9 ms at 10⁵ declarations; the real hash is about **12× more expensive per declaration** than that proxy implied.

Consequences, carried into the architecture and migration documents:

- Incremental identity is not merely required at scale — **full-state identity is out of budget at scale already reached by real programs**.
- Because the corpus is shallow (max depth 5), the Merkle fold's O(changed × depth) term is **cheap**: the concern the fold raised is not the concern that materialised.
- The expensive stages are the whole-graph ones — path map, skeleton forest, Prüfer encoding, incidence sort — none of which is incremental today. **A Merkle fold over subtree digests does not by itself fix this.**

**Decided (U6): two identities.** An **O(delta), history-sensitive commit-chain digest** on every commit; a **history-independent canonical semantic hash** for checkpoint, audit and convergence. **Full `canonical_program_hash` is forbidden on the 100 Hz commit hot path.**

### I.5 Tests the two-identity scheme requires

| Test | Assertion |
|---|---|
| chain digest is **O(delta)** | cost is flat in graph size and scales with change size — measured across 10², 10³, 10⁴ declarations at constant delta |
| chain digest is **history-sensitive** | two different change sequences reaching the same graph produce **different** chain digests |
| canonical hash is **history-independent** | the same two graphs produce the **same** canonical hash — this is exactly the M0.5 C1/C5/C6 property, now load-bearing for convergence |
| **the pair together** | history-sensitive and history-independent identities disagree on the above by design; a test asserting they agree would be wrong |
| chain digest commits to enough | delta content, base digest and resulting version all participate — two different sequences cannot chain to the same value |
| checkpoint carries **both** | a replica resuming from a checkpoint can verify its state canonically *and* continue the chain |
| **hot path never calls `canonical_program_hash`** | structural assertion, not timing: the 100 Hz commit path must not reach it. Enforce by construction (separate module boundary) rather than by a timing threshold that could pass on a fast host |

The last row deserves its enforcement mechanism chosen deliberately. A timing-based gate ("commits stay under 10 ms") would pass on a small graph even if the hot path *did* call the canonical hash, and would then fail mysteriously in production at scale. A structural gate — the hot path cannot even name the function — fails at compile time and cannot be outrun by a fast machine.

---

## M0.6 addendum (2026-08-28)

**The identity tests in §I.5 must not be written against `canonical_program_hash` v1.** M0.6 measured its coverage: v1 misses annotations, values, weights and `bases`, and `compute_merkle_hashes` misses `bases` and incidence targets. Neither is a full semantic identity, so a convergence test built on either would pass while the graphs genuinely differ.

The `semantic_snapshot_hash` v2 tests therefore need a **field-coverage gate of their own**, run before any convergence assertion:

| Test | Assertion |
|---|---|
| coverage matrix over v2 | **every** semantic field changes the digest — the M0.6 matrix, re-run against v2 with no ❌ remaining |
| `bases` coverage | v2 distinguishes two graphs differing only in inheritance (**both current hashes fail this**) |
| annotation / weight coverage | v2 distinguishes graphs differing only in annotations or weights (**v1 fails this**) |
| IEEE completeness | `-0.0`≠`+0.0`, distinct NaN payloads distinct, `±Inf` distinct — via `to_bits()`, not normalisation |
| ordering (U1) | slot and arc order change the digest |
| multiplicity (U3) | duplicate incidences change the digest |
| ID independence | dense and generational ID permutation does **not** change the digest |

**A coherence gate is also now available.** `validate_hierarchy_index_coherence` returns bounded, deterministically ordered findings and is verified against nine deliberate corruptions. Model-based tests (§H) should assert it holds after **every** commit, which turns the §2.3 lockstep invariants from convention into a checked postcondition.

---

## M2b addendum (2026-08-29) — where these tests will live

The V&V structure this plan's tests belong in now exists:

```
hymeko_core/tests/unified_ir/
  support/ · verification/ · validation/
```

Everything specified in Parts G and H is **BLOCKED** in `docs/verification/unified-ir/traceability.csv` as `UIR-DYN-01`…`UIR-DYN-10`, against M3 (stable IDs) and M4 (transactions).

**No placeholder tests were written.** A test that constructed the expected shape and asserted `true` would raise the test count and the coverage percentage while establishing nothing — and would then have to be *found and removed* when the real implementation arrived. The requirements are recorded; the tests are not faked.

The reference-model constraint in §0 stands and is now enforceable: `support/` holds the shared builder and oracle, so a model that accidentally reused production machinery would be visible as an import from `hymeko::` rather than from `support::`.

---

## M2b-corrective addendum (2026-08-29) — strict consumers, total readers

A policy this plan implied but never stated, made explicit after M2b shipped two consumers that silently accepted malformed input:

> **The accessor layer is total and `Option`-valued. Turning absence into a visible failure is the consumer's job, not the reader's.**

The two are not in tension — they are a division of labour. A read API that panicked would make diagnosis impossible; a consumer that shrugged makes corruption invisible. M2b got the first half right and the second half wrong: `IrWriter` returned `Ok(())` on a dangling link and emitted a truncated document, and `HyperGraphView` skipped a dangling arc with a bare `continue`, producing a view with fewer incidences than the IR describes.

### Consumer classes

| Class | Contract on malformed input | Example |
|---|---|---|
| **Total reader** | returns `Option`; never panics, never errors | `Ir::decl_node`, `Ir::as_node`, `Ir::child_decls` |
| **Diagnostic consumer** | renders a visible marker and continues | `util::pretty_print` — `(arc_rec missing)` is diagnosis, which is the whole point of a debug printer |
| **Strict consumer** | typed error | `IrWriter::write_all` → `io::ErrorKind::InvalidData`; `HyperGraphView::try_from_ir` → `ViewBuildError` |
| **Compatibility shim** | documented fail-fast, delegating to the strict path | `HyperGraphView::from_ir` — `# Panics`, preserving the pre-M2b contract |

**Silence is not a class.** Any consumer that neither reports nor renders is a defect, and `UIR-M2B-03` / `UIR-M2B-06` now say so.

### What a malformed-input test must assert

Not "it survived". Surviving and silently truncating are the same observable. Assert the **discriminating** property:

- the exact error variant (`ViewBuildError::DanglingArc`, not merely `is_err()`);
- the error kind (`io::ErrorKind::InvalidData`);
- that the message names the offending id;
- and, in the same module, that **valid** input is unchanged — otherwise strictness is bought with false positives.

### One distinction the tests must preserve

An **edge-to-edge reference is not malformed.** `HyperGraphView` is vertex-centric and has always skipped references to `Edge` declarations; that is the rewrite engine's legitimate output. Only a `Node` declaration with no record, or a reference to a declaration that does not exist, is an error.

Getting this wrong in the corrective phase broke three pre-existing tests before it was caught — see `reports/2026-08-29-unified-ir-m2b-vv-corrective.md` §2.
