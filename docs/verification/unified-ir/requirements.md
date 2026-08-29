# Unified IR — requirements

**Established:** 2026-08-29 (M2b) · Machine-readable status in `traceability.csv`.

Requirement IDs are stable. A requirement is never renumbered; if it is superseded, the old ID is marked and a new one added, so an old report's citation stays resolvable.

## Status vocabulary

| Status | Meaning |
|---|---|
| **VERIFIED** | A named, currently-running test establishes it. |
| **PARTIAL** | Established in part, or established by inspection where the property is not yet exercisable. The gap is stated. |
| **PLANNED** | Accepted requirement, no implementation yet. No test exists and none is faked. |
| **BLOCKED** | Cannot be verified until a named later stage lands. |

`PLANNED` and `BLOCKED` differ in cause: PLANNED is "we have not built it"; BLOCKED is "it cannot be expressed with what exists". `UIR-DYN-04` (stale-ID aliasing) is BLOCKED rather than PLANNED because dense ordinal IDs *cannot* express it — it needs generational keys from M3.

## Language decisions (U1–U6)

Recorded 2026-08-28, evidence in `reports/2026-08-28-unified-ir-m05.md` §0.5.

| # | Requirement | Status |
|---|---|---|
| **U1** | Arc order and reference-slot order are semantic and must be preserved. | VERIFIED |
| **U2** | `parent`/`children` becomes the canonical hierarchy; the path `Index` becomes a derived view; a coherence validator is required while both exist. | validator VERIFIED; **authority flip PLANNED (M2e)** |
| **U3** | Duplicate incidences are distinct entities with distinct `HyperArcId`s; multiplicity is semantic; removal is by incidence ID. | multiplicity VERIFIED; removal-by-ID PLANNED (M3/M4) |
| **U4** | `ValueR::Num` preserves all IEEE-754 bit patterns; algebraic weights must be finite; `-0.0` is distinct and bit-preserved; canonical ordering uses `total_cmp`. | bit-faithfulness VERIFIED; finiteness PLANNED (M4); `total_cmp` PARTIAL |
| **U5** | No language-level depth limit; traversal must be iterative; any future cap is a resource limit, not semantics. | VERIFIED |
| **U6** | Commit-chain identity and semantic snapshot identity are maintained separately; full `canonical_program_hash` is forbidden on the 100 Hz commit path. | PLANNED (M4) |

### Why U4's `total_cmp` clause is PARTIAL, not VERIFIED

The M0.6 audit found exactly two `partial_cmp` sites in production, both non-floating and delegating to a total `Ord`. So the rule is not *violated* — but it is also not *exercised*, because no float ordering exists in any identity path yet. It becomes verifiable when v2 introduces value ordering. Marking it VERIFIED would claim evidence that does not exist.

## Contract requirements

The accessor layer's contracts are stated as preconditions/postconditions/invariants in `hymeko_core/src/ir/access.rs` and verified per `traceability.csv` rows `UIR-ACC-*`.

Two contracts are deliberately distinct and must not be conflated in migration:

- **Map-only** (`as_node`/`as_edge`/`as_arc`) — reports the mapping verbatim, including a mapping that names a foreign or absent record.
- **Checked composite** (`node_for_decl`/`edge_for_decl`) — additionally resolves the record and validates its `decl` back-link, returning `None` rather than a record belonging to another declaration.

`arc_for_decl` is a third case: `ArcRec` has no own-declaration back-pointer (`in_edge` names the owning *edge*), so it checks presence only. That asymmetry is a property of the record layout, and `UIR-ACC-07` exists to stop someone "fixing" it into a false back-link check.

## Hash requirements

`canonical_program_hash` v1 is **structural-only** (`UIR-HASH-02`). It covers kind, path, skeleton shape and ordered incidence topology. It does **not** cover annotations, values, weights or `bases`.

`UIR-HASH-03` records that **`bases` is covered by no identity system at all** — neither v1 nor `compute_merkle_hashes`. That row is VERIFIED in the sense that the *absence* is established by test; it is an open defect, not a satisfied requirement. The distinction matters: a reader scanning for VERIFIED rows must not conclude inheritance is protected.

## Requirements not yet expressible

`UIR-DYN-01` … `UIR-DYN-10` cover the dynamic hypergraph: add/remove, stale IDs, cascade policy, transaction rollback, batch atomicity, snapshot isolation and distributed replay. **No test exists for any of them, and none was written.** A test that constructed the expected shape and asserted `true` would inflate the count and the coverage while establishing nothing.

Their design is specified in `docs/unified-ir-dynamic-test-plan.md`. They become verifiable at M3 (stable IDs) and M4 (transactions).

## Requirements coverage

Two ratios, reported separately because collapsing them would be misleading:

- **Implemented-requirement coverage** — VERIFIED ÷ (VERIFIED + PARTIAL), over requirements whose subject exists today.
- **End-state coverage** — VERIFIED ÷ all requirements, including PLANNED and BLOCKED.

The second is deliberately low and should stay low until M3/M4. A rising end-state number without new implementation would mean requirements were dropped, not met.
