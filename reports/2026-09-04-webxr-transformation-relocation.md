# WebXR relocation from core to transformation

Date: 2026-09-04 CEST

Branch: `refactor/relocate-webxr-to-transformation`

Authorizations:

- `APPROVED-CORE-EDIT: relocate-webxr-to-transformation`
- `APPROVED-CORE-EDIT: repair-core-yaml-syntax`

## Result

The production WebXR validator, borrowed spatial query, WebXR fixtures, tests,
and active semantic-model document have moved to the transformation repository
as the dedicated `hymeko_webxr` crate. The destination was committed first as
transformation commit `b435d86`, using the latest original core working tree so
that its uncommitted camera/fiducial/query additions were preserved.

Core no longer exports `hymeko::webxr` and no longer contains production WebXR
source, WebXR integration tests, active WebXR fixtures, or the active WebXR
semantic-model document. Historical plans and reports remain as immutable
design/evidence records and are explicitly superseded by this relocation.

The active platform strategy now assigns WebXR vocabulary, validation, and
query ownership to `transformation/hymeko_webxr`; core owns only the canonical
IR and generic access surfaces consumed by it. No core IR layout changed.

Removing the three tracked WebXR fixtures intentionally changes the core corpus
identity from 105 to 102 files. The new frozen digest is
`f9ab7da8b5d823fa9d55c28e78fd63bc37660a97f67852ad587a145f951cb3b5`.

## CORE.YAML repair

The five `approval.required_for` strings are now quoted YAML scalars. This
repairs parsing where backticks and the wrapped plain scalar made the mapping
invalid/ambiguous; the five policy statements and their order are unchanged.

## Verification

- YAML parse via PyYAML: pass, five `approval.required_for` entries
- `cargo fmt --all -- --check`: pass
- frozen corpus digest test: pass
- `cargo test --workspace --all-features --no-fail-fast`: 511 passed,
  0 failed, 3 ignored
- `cargo clippy -p hymeko_core --lib --all-features -- -D warnings`: pass

The migration plan and diagrams are under
`docs/plans/2026-09-04-webxr-transformation-relocation/`.
