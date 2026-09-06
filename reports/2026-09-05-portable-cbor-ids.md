# Portable CBOR ID sentinel

Created: 2026-09-05 03:25 CEST. Branch: `fix/portable-cbor-ids`, based on `0a34a97`.

The user authorized the prepared Core fix on a separate fix branch while another task uses
the original Core checkout. Changes are isolated in `core-portable-id`; the original
`core` checkout and its branch were not modified. The existing `hymeko_core` lockdown
applies; this task's authorization covers the ID serde implementation and its tests/docs.

## Failure and change

A 74,181-byte fixture produced by native Windows Core `encode_compiled` was rejected by
the actual browser WASM decoder: `integer 18446744073709551615, expected usize`.
`Id::NONE` was serialized as the host's `usize::MAX`. A 64-bit sentinel cannot be decoded
as a wasm32 `usize`.

The existing `Id<T>` serde implementation now writes a `u64`, reserving `u64::MAX` for
NONE on every target. Decode maps that value to the local sentinel and rejects ordinary
indices that overflow or collide with the target's reserved index. In-memory layout,
public constructors/accessors, canonical IR, hashing, and ordinary ID wire bytes remain
unchanged. No dependency was added. The conversion adds no allocation.

Files: `hymeko_core/src/common/ids_impl.rs` (serde and direct byte tests),
`hymeko_core/src/common/ids.rs` (wire contract documentation), this report, and
`SYMBOLS.md` (generated worktree heading required by the repository pre-commit check).
Discovery found the canonical serde seam in `ids_impl.rs`; consumer-side CBOR rewriting
was rejected because the Core owns IR encoding and validation.

## Migration

Existing 64-bit payloads retain their bytes. New wasm32 payloads use the same NONE value.
Legacy wasm32 payloads containing `u32::MAX` as NONE must be regenerated from source:
the decoder does not guess that an otherwise numeric index means a legacy sentinel.
The approved plan and initial patch were prepared before implementation under the local
`%LOCALAPPDATA%/HyMeKo/core-portable-id-review.txt` and `core-portable-id.patch` artifacts.

## Executed verification

- `cargo test -p hymeko_core --lib common::ids_impl::portable_wire_tests --offline`:
  both native byte tests passed.
- `cargo test -p hymeko_core --offline`: unit, integration and documentation suites passed;
  integration suite: 216 passed, 6 intentionally ignored, 56.73 s.
- `cargo clippy -p hymeko_core --lib --offline -- -D warnings`: passed.
- Six direct cases executed as actual wasm32 in Node's WebAssembly runtime against the
  consumer's compiled Core: NONE decode/encode, ordinary ID decode/encode, rejection of
  `u32::MAX`, `u32::MAX + 1`, `u64::MAX - 1`, and a negative integer. All passed.
- Simulator `model_ingress` and `runtime_binding`: 5 integration tests passed.
- Actual Chrome + Zenoh + Rust/WASM: source replacement removed the camera binding at
  revision 2; the unchanged native CBOR fixture restored it at revision 3. Canonical hash
  remained `64b12abf9956fd893044bff81c6f9444bf4003e0e1ce9ce67a9f713dc810e913`.

The browser test previously failed with the exact same native fixture before this change.
No performance improvement is claimed; no benchmark campaign was run. The integration is
local, using sibling path dependencies. Publication and merge are outside this task.
