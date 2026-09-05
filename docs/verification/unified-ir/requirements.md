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

---

## Taxonomy correction — 2026-08-29 (M2b V&V corrective)

The earlier status vocabulary conflated three different things under `VERIFIED`, and the resulting satisfaction rates were wrong. Corrected additively; the original section is left above so the error stays visible.

### Row kinds

Every traceability row now declares what it *is*, because a rate that mixes them is not a rate of anything:

| Kind | Meaning | Counts toward satisfaction? |
|---|---|---|
| **NORMATIVE_REQUIREMENT** | the system *shall* do this | **yes** |
| **DESIGN_CONSTRAINT** | an architectural rule the implementation must respect | no — tracked separately |
| **CHARACTERIZATION** | records what the code *currently does*; not a demand | **no** |
| **KNOWN_DEFECT** | a recorded defect | **no** — counted as defects |

### Statuses

`PASS` · `FAIL` · `PARTIAL` · `NOT_IMPLEMENTED` · `BLOCKED`

`NOT_IMPLEMENTED` means the requirement's subject does not exist yet. `BLOCKED` means it cannot even be expressed with what exists — `UIR-DYN-04` (stale-ID aliasing) is BLOCKED rather than NOT_IMPLEMENTED because dense ordinal IDs *cannot* express it.

### What was wrong

`UIR-HASH-03` read *"bases is covered by no current identity system"* with status `VERIFIED`. That is a **characterization of a gap**, and marking it VERIFIED put a defect in the numerator of the satisfaction rate. A reader scanning for green rows would have concluded inheritance was protected.

It is now split:

| Row | Kind | Status |
|---|---|---|
| `UIR-IDENT-01` — *semantic snapshot identity covers inheritance (bases)* | NORMATIVE_REQUIREMENT | **NOT_IMPLEMENTED** |
| `UIR-DEFECT-01` — *no current identity system covers bases* | KNOWN_DEFECT | **FAIL** |

The same test (`hash_coverage_matrix`) remains the evidence for both. It always was evidence of a *gap*; only the label was wrong.

`UIR-IDENT-02` was added for the same reason, covering annotations, values and weights.

### Silent-skip requirements rewritten

`UIR-M2B-03` and `UIR-M2B-06` previously asserted that malformed input was *survived*. Surviving and silently truncating are the same observable, so those rows certified the defect. They now require:

> malformed input causes no UB, is never silently accepted, produces a **typed error** on the strict API, and **documented fail-fast** on the compatibility API.

### Corrected rates

Four numbers, reported separately because any single blended figure misleads:

| Measure | Definition | Value |
|---|---|---|
| **Traceability / evidence coverage** | rows citing at least one running test ÷ all rows | **41 / 63 = 65.1 %** |
| **Implemented normative satisfaction** | normative PASS ÷ normative rows whose subject exists (PASS + FAIL + PARTIAL) | **31 / 31 = 100 %** |
| **End-state normative satisfaction** | normative PASS ÷ all normative rows | **31 / 49 = 63.3 %** |
| **Known defects** | rows of kind KNOWN_DEFECT | **4** |

**Retraction.** The previously reported **96.9 %** ("implemented") and **70.5 %** ("end-state") are withdrawn. They used a denominator of `VERIFIED + PARTIAL` over a set that mixed requirements with characterizations, and their numerator included the `bases` gap as satisfied.

**Read the 100 % correctly.** It says every normative requirement *whose subject exists today* is met. It does **not** say the system is complete — 18 normative requirements are NOT_IMPLEMENTED or BLOCKED, and 4 known defects stand. The defect count is where the bad news lives, which is precisely why it is reported as its own number rather than folded into a percentage.

---

## Fixture-coherence correction — 2026-08-29 (`unified-ir-hyperspec-coherence`)

Appended, not edited in place. Full account in
`reports/2026-08-29-unified-ir-hyperspec-coherence.md`.

### `UIR-M2B-06` was PASS in name only

The row reads *"malformed IR in derived-view construction … is never silently
accepted"*. The M2b corrective made that true for a dangling arc, a dangling
target and a missing node record — but it kept an escape:

```rust
None if ir.decl_count() > 0 => return Err(DanglingTarget { .. }),
None => continue,   // no declaration table to consult
```

An unknown target in an `Ir` with an empty declaration table was still **skipped
silently**. So for the one input shape that actually reached the branch, the
requirement was false. **Retrospective status for that interval: PARTIAL**, not
PASS. The row is PASS again only from the commit that removed the escape.

The escape existed to accommodate one producer — `HyperSpec`, an incoherent
synthetic fixture (`UIR-DEFECT-04`). Weakening the consumer to fit a broken
producer is the wrong direction; the fix is on the producer.

### `UIR-DEFECT-04` is closed, not deleted

| Field | Value |
|---|---|
| Status | **FIXED** (was FAIL) |
| Evidence | `validation::workload_coherence::*` — four coherence regressions |
| Fix | `HyperSpec::program_with_weights` builds `decl_nodes`, all three id maps, record back-links, and the arc-to-edge child chain |

The row stays in the table so the defect's history remains resolvable. `FIXED` is
added to the status vocabulary for `KNOWN_DEFECT` rows; it means the defect is
closed with a named regression test, as distinct from a requirement's `PASS`.

### New rows

| Row | Kind | Status |
|---|---|---|
| `UIR-WL-01` — a synthetic fixture used as evidence for a production path is itself a coherent `Ir` | NORMATIVE_REQUIREMENT | PASS |
| `UIR-WL-02` — the weighted and unweighted variants differ only in `RefAtomR.weights` | NORMATIVE_REQUIREMENT | PASS |
| `UIR-WL-03` — workload declaration identity is a function of the index, never of the seed | DESIGN_CONSTRAINT | PASS |

`UIR-WL-01` is the general form of what `UIR-DEFECT-04` was a single instance of:
a fixture that production could never produce is not evidence about production.

### Recomputed rates

| Measure | M2b corrective | now |
|---|---|---|
| Traceability / evidence coverage | 41 / 63 = 65.1 % | **45 / 66 = 68.2 %** |
| Implemented normative satisfaction | 31 / 31 = 100 % | **33 / 33 = 100 %** |
| End-state normative satisfaction | 31 / 49 = 63.3 % | **33 / 51 = 64.7 %** |
| Open known defects | 4 | **3** |
| Fixed defects | 0 | **1** |

Row kinds: 51 NORMATIVE_REQUIREMENT, 6 DESIGN_CONSTRAINT, 5 CHARACTERIZATION,
4 KNOWN_DEFECT (3 open, 1 fixed). Only normative rows enter the satisfaction
denominator; a fixed defect leaves the open count and does not enter the
numerator of any satisfaction rate.

## Platform-conformance addendum — 2026-08-30 (`unified-ir-platform-conformance`)

**Authorisation:** none required — tests + docs + `benches/ir_bench.rs` only; no
production `hymeko_core/src` change. Adds three requirement families that the
M2b matrix did not cover. Machine-readable rows in `traceability.csv`.

### CLI (`UIR-CLI-01..04`)

The product `hymeko` CLI (package `hymeko_core`, `src/main.rs`) had **no**
conformance test. Three now run against the pre-built `CARGO_BIN_EXE_hymeko_core`
(no recursive Cargo): success (exit 0, prints), usage error (exit 2), missing file
(exit 1) — all **PASS**. `UIR-CLI-04` records the single-CLI *invariant* as
**PARTIAL**: `hymeko` is minimal and has no subcommands, while `pgraph` and
`hymeko_pgraph_dump` (graph repo) are parallel product/debug CLIs and
`parser/src/main.rs` is a `Hello, world!` stub. Consolidation is a later
production change; see `docs/unified-ir-single-cli.md`.

### CBOR readiness (`UIR-CBOR-01..05`)

Implemented and independently validated on 2026-09-04. The public
`encode_compiled`/`encode` facade and bounded, fail-closed `decode` facade close
`UIR-CBOR-01/02`; the 105-program frozen corpus round-trip closes
`UIR-CBOR-03` — all **PASS**. `ciborium 0.2.2` is now a pinned direct production
dependency. Decode bounds the CBOR item tree before Serde allocation, rebuilds
the interner and path index, checks IR/hierarchy/namespace coherence, recomputes
the canonical hash, and rejects non-portable paths, indefinite items, trailing
bytes, malformed IDs, and hostile lengths.

`UIR-CBOR-04/05` remain **PASS** characterizations with their risk closed:
schema-v1 retains duplicated derived/cache fields for compatibility, but decode
never trusts them without reconstruction and comparison; `root_path` and import
module keys are encoded as normalized, workspace-relative portable paths. The
implementation record is `reports/2026-09-04-canonical-cbor-codec.md`; the
2026-08-30 conformance report remains the historical pre-implementation baseline.

### Source emission completeness (`UIR-EMIT-01..02`)

`IrWriter` is a **skeleton-only** serialiser: it emits names + hierarchy but not
annotations, values, bases, or hyperarc incidences (arcs stub to
`/* Arc references pending */`). `UIR-EMIT-01` is **PARTIAL** (a new test proves
annotations are dropped); `UIR-EMIT-02` (lossless emit→reparse round-trip) is
**NOT_IMPLEMENTED**, blocked by the incomplete emission. This is distinct from the
`transformation` crate's `emit_hymeko`, which is complete and whose round-trip is
tested there.

### Recomputed rates (matrix-wide, `traceability.csv`)

77 PASS · 3 PARTIAL · 3 FAIL (known defects) · 9 NOT_IMPLEMENTED · 17 BLOCKED ·
1 FIXED · 1 PASS_WITH_REGRESSION (111 rows). The remaining future contracts
(stable IDs, mutation/transaction/replay and related consumers) remain explicit;
the CBOR rows moved only with executable facade, hostile-input, corpus, hash and
memory evidence.

## Distributed admission verification — 2026-09-05

This update is scoped to the existing Core `ir_stream` admission boundary.
It does not change the historical matrix-wide totals above or promote the
`UIR-DYN-*` native mutation/publication requirements to PASS. The distinct
[Lean specification](distributed-semantics.lean) is a normative abstraction,
not a translation of Rust. The [audit report](../../../reports/2026-09-05-distributed-semantics.md)
and [baseline mapping](../../../reports/distributed-semantics/traceability.tsv)
keep **model proof**, **runtime observation** and **missing implementation** separate.

| Requirement | Lean model | Actual Core correction `8a0c49d` |
|---|---|---|
| `DIST-ORDER` — componentwise version order | PROVED: reflexive, transitive, antisymmetric | Existing `SnapshotVersion::dominates`; not a new exhaustive Rust proof |
| `DIST-MONOTONE` — accepted transitions cannot regress any component | PROVED: one step and finite replay | PASS: former snapshot/delta regression cases now return exact `IncoherentVersionBump` without state change |
| `DIST-RESUME` — an equal established version with unknown identity is not installed | PROVED | PASS after `resuming_at(V)`, `resuming_at(ZERO)` and resync reset |
| `DIST-BOOTSTRAP` — a fresh assembler may install ZERO | PROVED by closed example | PASS: public-API probe |
| `DIST-STALE` — a delta with a different base does not install | PROVED | PASS: public-API stale-base probe |
| `DIST-PRESERVE` — rejection leaves modeled held state unchanged | PROVED | PARTIAL: checked version/digest observations only; no actual IR application in this probe |
| `DIST-IDEMPOTENT` — applying the same transaction twice preserves the once-applied state | PROVED, including adjacent duplicate elimination in a list | PARTIAL: known-identity replay after reset and the existing assembler suite pass; full semantic-state application remains outside this boundary |
| `DIST-SNAPSHOT` — same coherent snapshot strictly newer than both replicas yields equal modeled state | PROVED under these explicit premises | PARTIAL: one resumed assembler advances; no full-state two-replica run |
| `DIST-BINDING` — transaction identity authenticates every relevant payload bit | NOT MODELED: identity/payload tags are independent | Not established by this proof or seven probes; cryptographic assumptions remain explicit |
| `DIST-INSTALL` — complete semantic application, durable atomic publication and installation acknowledgment | NOT MODELED | NOT_IMPLEMENTED as an end-to-end integration path here |
| `DIST-LIVE` — eventual delivery, worker recovery and distributed progress | NOT MODELED | NOT_RUN: transport/scheduler experiment remains a follow-up |

The 19 named Lean theorems are checked with installed Lean 4.19.0 and `Std`,
without added dependencies. The axiom audit lists only the standard `propext`
and `Quot.sound` where used; there is no project-declared axiom or admitted proof.
State idempotence does not guarantee a duplicate *outcome*: a retried advancing
delta may return stale-base while preserving state. Snapshot convergence does
not prove snapshot/delta equivalence, delivery, or resolution of two divergent
replicas already at the same target version.

`Nat` components abstract the ordering of bounded `u64` versions without modeling
increment/overflow. Whole-transaction admission abstracts away wire and sequence
validation, staging limits and reset handling. The payload token is not a decoded
canonical IR. Equal structural v1 hashes are insufficient as full-state equality
evidence. Numerical associativity/distributivity and hyperedge partition exchange
are separate obligations, with no new algebra or grammar approved by this update.

The user approved `APPROVED-CORE-EDIT: distributed-admission-monotone-resume`.
Correction `8a0c49d` changes the existing assembler, with no public-signature,
wire, dependency or fixture change. All nine regression cases pass on Amaterasu;
the original seven retain their expected outcomes. The test source moved from
the audit directory into `hymeko_core/tests/ir_stream_admission.rs` and is
registered in the existing harness. The old source and FAIL evidence remain
available at audit commit `1befefb`; they are not rewritten as historical PASS.
[Current report](../../../reports/2026-09-05-distributed-admission-fix.md) and
[current mapping](../../../reports/distributed-admission-fix/traceability.tsv)
record observed conformance. Rust PASS here means the named checks passed; it
does not turn the abstract Lean proof into a whole-implementation refinement proof.
