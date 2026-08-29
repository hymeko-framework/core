# Unified IR — verification & validation

**Established:** 2026-08-29 (M2b) · **Authorisation:** `APPROVED-CORE-EDIT: unified-ir-m2b-vv-core-consumers`

This directory is the durable V&V record for the unified mutable IR migration. It replaces the practice of leaving each phase's tests in a directory named after the phase, which stops being navigable the moment the phase is over.

## Layout

```
hymeko_core/tests/unified_ir/
  support/       builders, semantic oracle, corpus access — no tests
  verification/  spec conformance on synthetic inputs with known answers
  validation/    behaviour on the real 96-fixture corpus and on real consumers
```

**Verification** answers *"does the implementation meet its specification?"* — hand-built inputs, known expected outcomes, no reliance on the corpus being present.

**Validation** answers *"does it hold on the programs this project actually has, and for the consumers that actually read it?"* — the 96-fixture corpus and before/after consumer equivalence.

The distinction is not decorative. A verification test that passes while validation fails means the specification is wrong; the reverse means the implementation is. Collapsing them loses that signal.

## Phase provenance

Directory names no longer carry it, so it is recorded here and in each module header.

| Module | Origin | What it established |
|---|---|---|
| `support/builder.rs` | M0.5 | `TestIr` with path/dense-ID decoupling |
| `support/oracle.rs` | M0.5 | semantic form independent of the canonical hash |
| `support/corpus.rs` | **M2b** | consolidates four copies of the same fixture walk |
| `verification/identity.rs` | M0.5 | canonical-hash invariance (C1–C10) |
| `verification/hierarchy.rs` | M0.5 | declaration depth, traversal limits |
| `verification/frozen_hash.rs` | M0.6 | frozen v1 known-answer vectors |
| `verification/hash_coverage.rs` | M0.6 | field-by-field hash coverage matrix |
| `verification/coherence.rs` | M0.6 | hierarchy/Index coherence + corruption cases |
| `verification/access.rs` | M2a | accessor contracts |
| `verification/ids.rs` | M2a.1 | sentinel/out-of-range equivalence |
| `validation/corpus.rs` | M0.5–M2a | the corpus-spanning tests, regrouped |
| `validation/consumer_equivalence.rs` | **M2b** | migrated consumers produce identical results |

## Files here

| File | Contents |
|---|---|
| `requirements.md` | the requirement set, with preconditions/postconditions/invariants |
| `traceability.csv` | requirement → verification method → test names → status |
| `coverage-baseline.md` | measured coverage, exclusions, and the toolchain's limits |

## What coverage is not

Line coverage says a line executed. It does not say the line was *checked*, that its result was asserted, or that a wrong result would have been noticed. M2b's fault-injection runs make the point concretely: four deliberate defects were introduced into fully-covered code, and one of them — conflating the map-only and checked-composite contracts — was initially caught by a **single** test, despite the mutated lines being at 94% coverage. Coverage located nothing; the mutation did.

Requirements coverage (`traceability.csv`) is the primary measure here. Line coverage is a secondary check for *unreached* code, which is a different and lesser question.
