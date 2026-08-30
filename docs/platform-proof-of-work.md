# HyMeKo platform Proof-of-Work: PoW-0 (static spine) and PoW-1 (dynamic, design)

**Written:** 2026-08-30 · **Base:** `0402d98` · No `APPROVED-CORE-EDIT` used. PoW-0 is
tests + docs; PoW-1 is a **design package only** — nothing dynamic is implemented.

## PoW-0 — the static computational spine (PROVEN)

An executable integration proof that the existing public path composes end to end,
on real hardware:

```
HyMeKo source (golden) → RealParser → ModuleStore/lowering → canonical Ir
  → coherence validation → HyperGraphView → star-expansion CSR
  → CPU serial & Rayon SpMM → WGPU resident SpMM + activation → readback
  → numerical & structural equivalence
```

Evidence: `hymeko_wgpu/tests/pow0_vertical_slice.rs` — **11 tests, all PASS**
(the GPU tests ran on the RTX 5080, not skipped). Golden fixture:
`hymeko_wgpu/tests/data/pow0_service_network.hymeko` (a redundant service/network
scenario: switches, hosts, services, one 4-way VLAN failure-domain hyperedge, a
second overlapping failure domain, a signed service-dependency edge with a neutral
advisory incidence, two placement edges; signs +/−/~, weights, hierarchy).

| test | proves |
|---|---|
| `pow0_source_ingests_to_the_expected_canonical_shape` | real source lowers to decls=19, nodes=9, edges=5, arcs=5, incidences=14 |
| `pow0_canonical_identity_is_coherent_frozen_and_deterministic` | coherence passes; canon hash frozen `00d4c01a…`; two compiles identical incl. name/kind mapping |
| `pow0_view_orientations_agree_and_rebuild_is_equivalent` | node-major ≡ edge-major incidence multiset; deterministic rebuild |
| `pow0_star_csr_matches_the_signed_bipartite_definition` | CSR shape/row_ptr/sorted/nnz/finite + structure vs an **independent** star-definition oracle (not the builder) |
| `pow0_cpu_spmm_serial_and_rayon_agree_at_multiple_feature_dims` | serial ≡ Rayon **bitwise** at fd 8/32/128 |
| `pow0_gpu_resident_spmm_activation_matches_cpu` | one-encoder/one-submit resident SpMM→ReLU == CPU within abs/rel 1e-5, NaN/Inf structural |
| `pow0_repeated_gpu_ops_reuse_buffers_without_changing_the_result` | buffer reuse across submits; no state leak |
| `pow0_full_pipeline_is_repeatable` | hash + CSR structure + CPU values repeatable |
| `pow0_missing_source_file_is_a_typed_error` / `pow0_parser_rejects_malformed_source` / `pow0_empty_and_singleton_edges_build_valid_csr` | negatives/boundaries |

Deeper negatives (malformed-IR rejection by the strict view builder; duplicate
incidences U3; weight bit-faithfulness U4; workgroup-indivisible dims; invalid CSR
pre-submit; type-level arena provenance) are **cited** from existing named tests
(`derived_views`, `hash_coverage`, `spmm`, `arena`, `session`) rather than
duplicated — see the traceability CSV.

**PoW-0 does NOT prove** mutation, transactions, CBOR, or distributed
synchronisation. Those are PoW-1.

## PoW-1 — the dynamic proof (DESIGN ONLY; BLOCKED on core approvals)

Target path (all future):

```
HyMeKo/CBOR snapshot → canonical mutable IR v0 → derived views
  → endpoint event → stable-ID semantic transaction → atomic commit v1
  → versioned view rebuild/refresh → ir_stream delivery → replica apply
  → matching state digest and compute result
```

### PoW-1 scenario (to be proven once the tokens land)

1. **Value-only update** — `UpdateIncidenceWeights` on a VLAN link; view refresh
   without topology rebuild.
2. **Topology update** — `RemoveIncidence` then `AddIncidence` (failover host swap).
3. **Rejected transaction** — remove a referenced entity without resolving its
   incidences → typed rejection, no partial commit.
4. **Stale entity-ID rejection** — an operation naming a freed generational id.
5. **Stale base-version rejection** — a transaction against an old `SnapshotVersion`.
6. **Idempotent replay** — the exact committed transaction re-applied is a no-op.
7. **Conflicting replay** — a different transaction at the same base version rejected.
8. **Second-replica convergence** — a replica applying the `ir_stream` reaches the
   same canonical digest.
9. **Weight-only refresh vs full rebuild** — incremental refresh equals a clean
   rebuild numerically.
10. **Topology rebuild vs clean parse** — the mutated snapshot equals a clean parse
    of the "after" golden (the second snapshot PoW-0 optionally carries as a future
    oracle — a *full-snapshot comparison*, explicitly **not** a transaction).

### Proposed approval scopes, in dependency order (each a separate token)

Each names files / public API / invariants / compatibility / rollback / tests /
coverage / memory+perf budget / falsifier. They reuse the mechanisms already
specified in `docs/unified-ir-architecture.md` Parts F–H and implemented in
`ir_stream`; they introduce **no** new delta vocabulary.

1. **`APPROVED-CORE-EDIT: seal-canonical-entity-ids`**
   - *Files:* `ir/ir.rs`, `common/ids.rs`. *API:* make `DeclId`/`NodeId`/… opaque
     (no public `.0` construction outside core). *Invariant:* an id can only be
     minted by core; mis-indexing becomes a compile error (`UIR-M2C-01`).
     *Compat:* accessors already the sole read path (M2a). *Rollback:* revert the
     field-privacy commit. *Tests:* compile-fail twin per sealed constructor.
     *Coverage:* accessor rows unchanged. *Budget:* zero runtime cost.
     *Falsifier:* any consumer that still constructs an id by integer.
2. **`APPROVED-CORE-EDIT: canonical-generational-arenas`**
   - *Files:* `ir/ir.rs` arenas + `common/ids.rs`. *API:* generational keys; free
     + reuse slots. *Invariant:* a stale key resolves to `None`, never a reused
     entity (`UIR-DYN-04` becomes expressible). *Compat:* `Option<Id>` size must
     not grow (`UIR-M2C-04` — today 16 B). *Rollback:* revert. *Tests:* stale-id
     rejection; ABA. *Budget:* `Option<Id>` ≤ 16 B; no per-lookup cost.
     *Falsifier:* a resurrected id, or an enlarged `Option<Id>`.
3. **`APPROVED-CORE-EDIT: canonical-ir-transactions`**
   - *Files:* new `ir/txn.rs`. *API:* `Transaction` with the Part-F delta set
     (`AddIncidence`/`RemoveIncidence`/`UpdateIncidenceWeights`/…), validate →
     atomic commit or typed reject. *Invariant:* all-or-nothing; no partial state.
     *Compat:* read APIs unchanged; `Ir` gains mutation behind the txn seam only.
     *Rollback:* revert; snapshots immutable. *Tests:* PoW-1 scenarios 1–3.
     *Budget:* commit O(delta), not O(graph). *Falsifier:* an observable partial
     commit.
4. **`APPROVED-CORE-EDIT: canonical-versioned-snapshots`**
   - *Files:* `ir/snapshot.rs` (+ reuse `ir_stream::SnapshotVersion`). *API:*
     immutable published snapshot per commit; base-version guard. *Invariant:*
     readers see a consistent immutable version; a stale base is rejected.
     *Compat:* `SnapshotVersion` already exists. *Rollback:* revert. *Tests:*
     scenarios 5–6; snapshot isolation. *Budget:* snapshot publish is structural
     sharing, not a full clone. *Falsifier:* a reader observing a torn version.
5. **`APPROVED-CORE-EDIT: canonical-cbor-codec`** *(also the standalone next step;
     see `2026-08-30-unified-ir-platform-conformance.md` §6)*
   - *Files:* `writers/cbor_writer.rs` (+`encode`/`decode`), `Cargo.toml`
     (`ciborium` transitive-dev → direct prod), new `cbor_roundtrip` test. *API:*
     bounded decode that recomputes/verifies `canon_hash`+`index`, workspace-
     relative `root_path`, a schema-version field. *Invariant:* `decode(encode) ≡
     parse(emit)`. *Rollback:* one revert. *Tests:* round-trip on the corpus.
     *Budget:* decode peak < 2× `Ir` heap. *Falsifier:* a lossy round-trip.
6. **`APPROVED-CORE-EDIT: ir-stream-canonical-apply`**
   - *Files:* `ir_stream/apply.rs` bridging `CommittedTransaction` → the txn API on
     a replica. *API:* apply a validated envelope as a transaction; idempotent
     replay; version conflict handling (the assembler already models these).
     *Invariant:* a replica converges to the source digest. *Compat:* `ir_stream`
     wire unchanged (no entity ids added — `UIR-M2C-02`). *Rollback:* revert.
     *Tests:* scenarios 6–8. *Budget:* apply O(delta). *Falsifier:* replica
     divergence, or an id leaking onto the wire.

These are **six separate tokens**, deliberately not combined. Token 5 is the
smallest independently-valuable step and is recommended first (it needs neither
stable ids nor transactions, and it removes the measured JSON-vs-packed 20×
snapshot overhead).

## Non-goals honoured

No second canonical model, no second product CLI, no new façade crate, no
test-only IR/transaction/delta framework. `hymeko_core::ir::Ir` remains the sole
canonical model; every CPU/HNN/CSR/WGPU object in PoW-0 is a disposable derived
view of it.
