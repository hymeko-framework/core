# Concrete Core admission correction

Prepared: 2026-09-05 17:18 CEST. Not applied.

The real Core image c935909, product-equivalent to 4a243ed, fails four of seven
public-API admission checks in admission_probe.rs. The retained baseline log
contains validated-message traces; this is not an abstract-model-only finding.

Protected file: hymeko_core/src/ir_stream/assembler.rs. The proposed patch:

1. Distinguishes an empty new assembler from a resumed version, including ZERO,
   through one private boolean; updates it on resume and successful commit.
2. Rejects an incomparable target before staging: every accepted advance must
   dominate the current version componentwise. Uses the existing typed
   IncoherentVersionBump outcome; no new enum variant or wire field.
3. Revalidates every equal-version transaction for an established version,
   returning ReplayOutsideWindow when its previous identity is unknown.
4. Preserves the fresh ZERO bootstrap and existing public signatures.

Acceptance: all seven public-API cases pass, the existing assembler suite,
Core release tests, formatting and workspace Clippy pass on Amaterasu; Lean
admission properties pass separately. Add the public-API cases to the existing
Core integration-test harness, retaining a single source of regression tests.
No canonical hash, decoder, payload grammar, dependency or fixture changes.

Risks: callers may have relied on previously accepted component regressions;
those now request resynchronization. This patch does not add durable checkpoint
storage, semantic delta application or installation acknowledgment. Version-only
resume remains conservative rather than fabricating a commit identity.

Rollback: revert only the scoped implementation/test commit on the task branch;
keep the baseline counterexamples, proof source and report as audit evidence.
The original user worktree and existing image remain unchanged.

Required token under Core CORE.YAML approval.mechanism:
`APPROVED-CORE-EDIT: distributed-admission-monotone-resume`

Reviewable implementation diff: proposed-admission-fix.patch.
