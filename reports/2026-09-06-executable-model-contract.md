# Tier 2A/2B — the optional executable-model contract, and a port axis independent of the sign

**Date:** 2026-09-06, 03:47–04:15 CEST · **Classification:** `ProtectedCoreEdit + PlanFirst + UncommittedWorkingTree`
**APPROVED-CORE-EDIT:** `executable-model-contract` · **Nothing committed or pushed.** No worktree created or removed.

---

## 1. Repository / branch / HEAD / remote tip (after fetch)

| Repository | Branch | HEAD | Remote tip | ahead/behind |
| --- | --- | --- | --- | --- |
| `hymeko/core` | `main` | `4acba44` | `4acba44` | 0 / 0 |
| `hymeko/nagare` | `feat/examples-learning-integration` | `f3660cc` | `f3660cc` | 0 / 0 |
| `hymeko/hymeko` | `docs/roadmap-registry` | `1032a2d` | `1032a2d` | 0 / 0 |
| `hymeko/graph` | `main` | `5078980` | `b234c48` | behind 28 (lock pins `9655f07`) |
| `hymeko/parser` | `master` | `284c490` | `284c490` | 0 / 0 |
| `hymeko/tensor` | `chore/retire-hymeko-graph` | `8d0e546` | `8d0e546` | 0 / 0 |

**No upstream drift**: the gap re-reproduced on exactly the tip the previous boundary measured.

## 2. One checkout each, and no new worktree

| Repository | worktrees before | after |
| --- | --- | --- |
| nagare / core / hymeko / graph / parser / tensor | 1 / 7 / 1 / 4 / 1 / 5 | 1 / 7 / 1 / 4 / 1 / 5 |

Work happened in `hymeko/core` and `hymeko/nagare` only. No temporary or parallel checkout was made
at any point, including for the baseline comparison.

## 3. A baseline failure that is now RESOLVED upstream

`unified_ir::validation::corpus::v1_corpus_digest_is_unchanged` — reported failing on 2026-09-05
(caused by `7019940`'s non-output-preserving incidence-traversal rewrite) — **passes on `4acba44`**.
The entry-state core suite is **395 passed, 0 failed, 7 ignored**, `clippy --all-targets -D warnings`
0, `fmt --check` 0. The earlier "not green" record is superseded.

## 4. Witness, before

Re-measured by direct inspection of `4acba44`, not inherited. One row `CanonicalOwned` (semantic
model digest, upstream). Rows 2, 4, 7, 9 `Missing`; rows 1, 3, 5, 6, 8, 10 `CarrierOnly`. The IR
entity set in `ir/ir.rs` was unchanged, no role/shape/dtype/direction type existed anywhere in
`hymeko_core/src`, and `ir::coherence` inspected no annotation.

## 5. What was built

Planned first: `docs/plans/2026-09-06-executable-model-contract/{plan.tex,plan.pdf,plan.tikz,plan.mmd}`,
4 pages, compiled before any production file was touched.

### The design question, answered before the code

**Most HyMeKo declarations are not tensors or operators.** The corpus describes service networks,
robotics meta-graphs and P-graphs; none of the 111 tracked `.hymeko` fixtures is a neural model. A
mandatory `role`/`shape`/`dtype` on every `DeclNode` would have been a category error. The contract
is therefore an **optional facet**:

- `Ir.decl_model: Vec<Option<ModelFacet>>` — a side table keyed by `DeclId`, exactly as `decl_hash`
  and `arc_hash` are. Chosen over a `DeclNode` field because `DeclNode` has 17 literal construction
  sites across core and its consumers while `Ir` has one, and because struct-of-arrays is what the
  surrounding per-declaration data already uses.
- `ModelFacet::Tensor(TensorContract{role, shape, dtype})` / `ModelFacet::Operator(OperatorContract{identity})`.
- A document is an **executable model** exactly when at least one declaration carries a facet. Only
  then does `validate_model_contract` engage; otherwise it returns `Ok` without checking anything.
- `PortDirection{Unspecified, In, Out}` on `RefAtomR`, **orthogonal to `SignedRefR`** and never
  derived from it.

`OperatorContract.identity` is an interned symbol **in a typed position**: core owns that the
declaration *is* an operator and that this symbol *is* its identity. Core deliberately does not own
the vocabulary — no operator registry entered core.

## 6. Compatibility, proven rather than assumed

| Question | Evidence |
| --- | --- |
| Absolute V2 digest goldens? | **None.** `semantic_digest.rs` contains zero 64-hex constants; every assertion is relative. The only absolute goldens are v1-family (`KAT_CORPUS_DIGEST`, four `frozen_hash` KATs, `sparse_workload_kat`). |
| v1 untouched? | `canonical_hash.rs` not modified. Asserted by `v1_is_blind_to_the_model_facet`, and by the unchanged corpus and frozen KATs in the full suite. |
| Legacy wire byte-identical? | `decl_model` is serialized **only when a facet exists** (the `namespace_roots` precedent) and `PortDirection::Unspecified` is `skip_serializing_if`. Asserted by `a_document_without_a_facet_emits_no_facet_field` and `a_pre_direction_payload_decodes_as_unspecified`. |
| Legacy payload decodes? | `#[serde(default)]` on both, plus a **fail-closed** length check: a facet table that disagrees with `decl_nodes` is a typed decode error, not silent padding. |
| Digest version | `SEMANTIC_DIGEST_ALGO_VERSION` 2 → 3, because the covered set grew. Safe precisely because no artifact pins a V2 value. |

**A regression was caught by my own gate, not by review.** Emitting `direction` unconditionally grew
the wire for legacy documents; `a_pre_direction_payload_decodes_as_unspecified` failed, and the fix
was `skip_serializing_if`. That is recorded because a compatibility claim that was never tested
against its own failure mode is not evidence.

## 7. Gates and real exit codes

```
git fetch --all --prune x6                                        0
ENTRY  cargo test -p hymeko_core                                  0   395 passed / 0 failed / 7 ignored
ENTRY  cargo clippy -p hymeko_core --all-targets -D warnings      0   0 warnings
ENTRY  cargo fmt -p hymeko_core --check                           0
ENTRY  v1_corpus_digest_is_unchanged                              0   PASSES (was failing 2026-09-05)
ENTRY  nagare: merkle_digest_soundness / canonical_mlp_witness /
       credit_vocabulary_characterization / contract_consumer_gate 0  14 / 2 / 8 / 10 passed
PLAN   pdflatex x3                                                0   4 pages, 0 undefined refs
2A     model_contract (20 tests)                                  0
2A     cargo test -p hymeko_core (full)                          101 → then 0 after the intended
                                                                      ALGO 2→3 assertion update
2B     model_contract (30 tests)                                  0
FINAL  cargo test -p hymeko_core (full)                           0   425 passed / 0 failed / 7 ignored
FINAL  cargo clippy -p hymeko_core --all-targets -D warnings      0   0 warnings
FINAL  cargo fmt -p hymeko_core --check                           0
FINAL  P2-C sha256sum -c (95 files)                               0   all preserved
```

**First divergence:** the single intended one — `v1_and_v2_are_domain_and_version_separated` asserted
`ALGO == 2`. Updated to 3 with the reason recorded inline, not flipped silently.

## 8. Witness, after — by the core tests that now prove each row

The Nagare `canonical_mlp_witness` **cannot be re-run against this work yet**: it consumes the
locked core commit `4acba44`, and moving the lock requires a core commit, which this boundary is not
authorised to make. The row evidence is therefore given as the core tests that own each property.

| # | Capability | Before | After | Proving test |
| --- | --- | --- | --- | --- |
| 1 | tensor entity | `CarrierOnly` | **`CanonicalOwned`** | `the_typed_contract_is_recovered_from_the_ir_alone` |
| 2 | role | `Missing` | **`CanonicalOwned`** | same, + `a_role_change_moves_the_digest` |
| 3 | shape | `CarrierOnly` | **`CanonicalOwned`** | `a_shape_change_moves_the_digest`, `a_rank_change_moves_the_digest` |
| 4 | dtype | `Missing` | **`CanonicalOwned`** | `a_dtype_change_moves_the_digest` |
| 5 | typed operator identity | `CarrierOnly` | **`CanonicalOwned`** | `an_operator_identity_change_moves_the_digest` |
| 6 | typed ports | `CarrierOnly` | **`CanonicalOwned`** | `the_round_trip_preserves_both_axes` |
| 7 | direction, separate from the sign | `Missing` | **`CanonicalOwned`** | `direction_moves_the_digest_at_a_fixed_sign`, `sign_moves_the_digest_at_a_fixed_direction`, `the_four_sign_direction_combinations_are_all_distinct` |
| 8 | validation | `CarrierOnly` | **`CanonicalOwned`** | `validate_model_contract` + six negative controls |
| 9 | shape/dtype inference | `Missing` | `Missing` | **out of scope by design** — Tier 2A provides the typed basis; inference is Nagare's lowering |
| 10 | operator-level serialization | `CarrierOnly` | **`CanonicalOwned`** | `the_round_trip_preserves_the_typed_contract_and_the_digest` |
| 11 | semantic model digest | `CanonicalOwned` | `CanonicalOwned` | unchanged (algorithm 3) |

**Nine rows moved. Row 9 is deliberately still open** and is not a core gap: inference needs a typed
input, which now exists.

## 9. Negative controls (all failing before, passing now)

tensor facet on an edge · operator facet on a node · model without an input · model without an
output · empty operator identity · a missing typed output **with every tag intact** · producing a
model input (the port swap) · producing a parameter · an operator with only inputs · a facet table
whose length disagrees with `decl_nodes`.

## 10. Changed files — commit-ready, not committed

**core** (`fix/…` branch to be created at commit time; currently on `main`, uncommitted):
`CORE.YAML` · `hymeko_core/src/ir/{ir,coherence,semantic_digest,incidence}.rs` ·
`hymeko_core/src/resolution/resolve.rs` · `hymeko_core/src/writers/cbor_writer/tests.rs` ·
`hymeko_core/examples/ir_depth_probe.rs` · `hymeko_core/benches/ir_bench.rs` ·
`hymeko_core/tests/unified_ir/{support/builder.rs,validation/cbor_roundtrip.rs,verification/{incidence,mod,semantic_digest}.rs}` ·
**new** `hymeko_core/tests/unified_ir/verification/model_contract.rs` ·
**new** `docs/plans/2026-09-06-executable-model-contract/` · **new** this report.

**nagare** (previous boundary, still uncommitted): `AGENTS.md` · `CLAUDE.md` · `Cargo.lock` ·
`nagare_vv/tests/contract_consumer_gate.rs` · `scripts/check.sh` · `scripts/hooks/{pre-commit,commit-msg,install.sh}` ·
**new** `reports/2026-09-06-slice1-reaudit-and-contract-migration.md` ·
**new** `docs/plans/2026-09-06-{slice1-mlp-execution-plan,ablation-preregistration}/`.

**Untouched, verified by hash:** the nine P2-C paths and `run_outputs/**`.

## 11. Nagare consumer impact

No Nagare production file was edited, and no second IR, operator vocabulary or port model was
created. When the core commit exists:

1. move `hymeko_core` **and** `hymeko_wgpu` together to that commit — no path dependency, no Cargo
   patch, no `rev=` on one declaration only;
2. `tests/merkle_digest_soundness.rs` constructs `RefAtomR` literally and will need
   `RefAtomR::plain` or the new field — a mechanical test-only change;
3. `canonical_mlp_witness` must then be re-run and its verdict vector updated from
   `CarriedSchemaNotCoreOwned` to `CanonicalOwned` for the nine moved rows. **That update is the
   proof, so it must be a change to the assertion, not to the fixture.**

## 12. Measured / inferred / blocked / not verified

**Measured:** every HEAD and tip; the worktree census both ways; the entry and final core suites;
all 30 new gates; the wire-compatibility assertions; the P2-C hashes.
**Inferred:** that row 9 (inference) is Nagare's rather than core's — an ownership judgement, argued
from the layering, not proven by a test.
**Blocked:** the Nagare witness re-run and the Slice-1 implementation, both on a core commit.
**Not verified:** the Nagare-side compile against the new core (impossible before the lock moves);
`scripts/check.sh` (Nagare's full gate, not run); the 28-commit `graph` lag, deliberately untouched.

## 13. Next executable step

Review this boundary and authorise the core commit on a feature branch. Everything after it is
sequenced: lock update → Nagare test fix → witness re-run and verdict update → Slice 1.
