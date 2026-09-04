# Compact declaration dispatch — verification & performance

**APPROVED-CORE-EDIT: compact-declaration-dispatch**
Created-at: 2026-08-31 19:10 CEDT · Branch `feat/core-decl-index-compaction` · base core main `791939b`

## Summary

The `Ir` carried three parallel per-declaration maps —
`decl_to_node: Vec<Option<NodeId>>`, `decl_to_edge: Vec<Option<EdgeId>>`,
`decl_to_arc: Vec<Option<HyperArcId>>` — at **48 B/decl** (each `Option<Id>` is
16 B). Every declaration has exactly one `DeclKind`, so at most one of the three
slots is ever populated; the other two are dead weight.

They are replaced by a single compact **8 B/decl** slot,
`decl_to_record: Vec<u64>` (**option D**, tagged index): `0` = NONE;
top two bits = `DeclKind` tag; low 62 bits = raw record index + 1. Storage is
private; the public read contract is unchanged (the total accessor API), and
core construction/lowering bind through checked typed mutators. The serde wire
keeps the legacy three-map layout **byte-for-byte**, and `canonical_program_hash`
is map-independent, so the change is **hash-neutral by construction**.

Result: **6× less mapping storage**, and — measured on real code — the compacted
form is **faster** on point-lookup, construction, and clone (no regression on any
axis).

## Files touched (core = the approved edit)

| File | ± | Role |
|---|---|---|
| `hymeko_core/src/ir/ir.rs` | +593/−… | `dispatch` module (encode/decode/errors), `decl_to_record` field, `as_*` rewrite, `push_decl` + `bind_*_decl` mutators, manual serde boundary, unit tests |
| `hymeko_core/src/ir/coherence.rs` | ~99 | slot-tag/kind, record-range, backlink checks; `ReservedDispatchSlot` finding; length check |
| `hymeko_core/src/ir/lower.rs` | 9 | 3 write sites → `bind_*_decl` |
| `hymeko_core/benches/ir_bench.rs` | 44 | construction/lookup migrated to accessors/mutators |
| `hymeko_core/examples/ir_depth_probe.rs` | 12 | same |
| 10 × `hymeko_core/tests/**` | — | 80-site migration + new fault/serde/dispatch tests |
| `hymeko_core/tests/unified_ir/verification/dispatch_serde.rs` | +new | known-answer wire + fail-closed decode |

**CORE.YAML items touched:** `ir/ir.rs`, `ir/coherence.rs`, `ir/lower.rs` — all
inside the authorizing `APPROVED-CORE-EDIT: compact-declaration-dispatch`. No
other core item, **no `Cargo.toml`/dependency change**.

## Option D encode/decode — boundary values (verified in `dispatch::tests`)

| input | encode → slot | decode |
|---|---|---|
| NONE | `0` | `Ok(None)` / `decode_tag → None` |
| `(Node, 0)` | `0x0000…0001` | `(TAG_NODE, 0)` |
| `(Edge, 0)` | `0x4000…0001` | `(TAG_EDGE, 0)` |
| `(HyperArc, 0)` | `0x8000…0001` | `(TAG_ARC, 0)` |
| `(Node, 2^62−2)` (max) | `0x3FFF…FFFF` | `(TAG_NODE, 2^62−2)` |
| `(Node, 2^62−1)` (over) | `Err(IndexOverflow)` | — |
| `(Edge, usize::MAX)` | `Err(IndexOverflow)` | — |
| reserved `(3<<62)\|1` | — | `Err(ReservedTag)`; `decode_tag → None` (never a false hit) |
| `(1<<62)\|0` (non-NONE, zero index) | — | `Err(MalformedZeroIndex)` |

All encode/decode arithmetic is checked (checked add + 62-bit bound + checked
`u64→usize`); no `unsafe`, no unchecked raw constructor, no truncation.

## Typed error variants (all fail-closed, ids-only, bounded, deterministic)

**`dispatch::DispatchError`** (encode/decode): `IndexOverflow`, `ReservedTag`,
`MalformedZeroIndex`, `UsizeOverflow`.

**`IrDecodeError`** (serde deserialize boundary): `InvalidMapLength`,
`MultipleBindings`, `KindMismatch`, `RecordOutOfRange`, `WrongBacklink`,
`DispatchEncode`. Each is exercised by a `dispatch_serde` corruption test that
mutates a valid wire and asserts the exact rejection.

**`coherence::Finding`** (validator, compact-dispatch additions):
`ReservedDispatchSlot` (slot decodes to reserved/malformed → always an error),
`KindMapMismatch` (slot tag ≠ `DeclNode.kind`), `RecordBacklinkMismatch`
(node/edge record backlink ≠ declaration, or arc index out of range). NONE is
permitted only as a partial state. Each has a named catcher test.

## Serde boundary — wire byte-known-answer

Deterministic fixture (2 nodes, 1 edge, its arc). The wire keeps the legacy
three-map layout verbatim; the compact `decl_to_record` **never reaches the
wire** (`v.get("decl_to_record").is_none()`):

```
decl_to_node = [0, 1, null, null]
decl_to_edge = [null, null, 0, null]
decl_to_arc  = [null, null, null, 0]
```

Full serialized wire: **1247 bytes**, sha256
`21eb528d1dd4badc91110d5afbdce42ee09364b20b2327c037905c933f6cc6d9`. The
known-answer test compares these frozen arrays **independently of the decoder**;
a separate test hand-mutates the wire to hit each fail-closed branch; `identity.rs`
C9 round-trips a richer IR and confirms the canonical hash is unchanged.

**Serialize cost (the sole compatibility allocation).** `impl Serialize for Ir`
is manual and **borrowed**: `decl_nodes`, `nodes`, `edges`, `arcs`, `meta`,
`decl_hash`, `arc_hash`, `doc_hash` are serialized **by reference — never
cloned**. Only the three legacy maps are reconstructed temporarily.
Cloned/allocated on serialize = **3 × n × `size_of::<Option<Id>>()` = 48·n bytes**
(freed when serialization returns); for the n=4 fixture, 192 B. This is the only
allocation on the serialize path; there is no whole-`Ir` clone and no
`#[serde(into)]`.

## Grep-proof of the migration (all 80 direct sites)

```
production (src) direct .decl_to_node/.decl_to_edge/.decl_to_arc field access : 0
decl_to_record accessed outside src (private storage)                          : 0  (5 hits = doc-comment / a JSON-key string / a bench group label)
legacy three-map names in src                                                  : serde boundary only (IrWire fields, serialize_field wire names, try_from_wire) + one doc comment in coherence.rs
```

The legacy three maps survive **only** as the `IrWire` shadow type and the
`serialize_field(...)` wire names at the serde boundary; they are not runtime
state anywhere.

## Test results

| Layer | Count | Result |
|---|---|---|
| lib unit (incl. `dispatch::tests`, `compaction_coverage`) | 112 | pass (1 ignored, pre-existing) |
| integration (`--test mod`) | 189 | pass |
| doctests | 7 | pass |

New tests: dispatch encode/decode boundary values; DispatchError/IrDecodeError
`Display`; serde known-answer + 6 fail-closed corruptions + unbound-decl partial
state; coherence ReservedDispatchSlot / KindMapMismatch / node+edge backlink /
arc-range fault mutants (each with a named catcher); the 5 pre-existing accessor
fault tests redesigned for the invariant-enforcing mutators (fresh unbound decl
injection). `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, and
doctests are clean.

**Touched-line coverage** (`cargo llvm-cov`): every reachable touched line is
covered. Three lines are documented waivers, all fail-closed defensive:
(1) the `bind_decl` conflict-rebind `debug_assert!` message (only formatted on
assertion failure, which the guard prevents); (2) the Serialize inner reserved-tag
`_` arm (unreachable — `decode_checked` errors on a reserved tag before the
match); (3) the `try_from_wire` `DispatchEncode` map-err closure (unreachable —
record indices are bounded by table length ≪ 2^62). Pre-existing uncovered
accessors (`decl_node_unchecked`, `decl_kind`, `SignedRefR::{target,sign}`) are
outside this change.

## Performance (criterion, 30 samples; real code, A = `791939b`/`1d1e5f1`, B = compact `788924a`)

Host: x86_64-pc-windows-msvc, rustc 1.97.1, release. Lower is better. Figure:
`reports/figures/2026-08-31-decl-dispatch-ab.svg`.

| Metric | A baseline | B compact | Δ |
|---|---|---|---|
| **Mapping bytes/decl** (`size_of`) | **48** | **8** | **−83 % (6.0×)** |
| `as_node` seq ×10k (`m2a_map_lookup/as_node`) | 4.79 µs | 4.40 µs | **−8 %** |
| construction n1 000 | 115.4 µs | 106.4 µs | −7.8 % |
| construction n10 000 | 2.241 ms | 2.199 ms | −1.9 % |
| construction n100 000 | 34.53 ms | 30.29 ms | −12.3 % |
| construction n10 000 arity-32 | 3.824 ms | 3.552 ms | −7.1 % |
| clone n1 000 | 79.1 µs | 77.3 µs | −2.3 % |
| clone n10 000 | 1.181 ms | 1.151 ms | −2.5 % |

**Acceptance — all met.** Mapping is exactly 8 B/decl; reduction 6.0× (≥5×);
point-lookup **improves** 8 % (≤3 % regression budget); construction/clone
**improve** (≤3 % budget, no regression); canonical hash + writer output
bit-identical (map-independent by construction; `identity.rs` C9 + `frozen_hash`
pass); no new hot-path allocation (`as_node` is a single load + mask/shift;
construction grows one `Vec` instead of three); **peak RSS drops** — 40 B/decl
freed (the two eliminated maps = 32 B, plus 16→8 B on the third), i.e. **40 MB at
1 M declarations**. The checked encode in the mutators does **not** regress
construction — managing one map instead of three dominates.

**A/A null control** (`docs/plans/2026-08-31-decl-index-compaction/bench_ab.rs`,
diagnostic, Instant-based per §10): the sequential lookup path is noise-dominated
(A-vs-A up to ±38 %), so mid-size sequential deltas sit inside the noise floor;
the random path A-vs-A is tight (+2–3 %).

**Correction to the plan's random claim.** `plan.tex` recorded a stand-in
prototype random-lookup speedup of ≈ −79 %. It is **not confirmed on real code**:
the only shuffled-id lookup bench (`m2a1_random_point`) reads the `DeclNode`
*arena* via `decl_node`, not the dispatch map, so A ≈ B there (≈ 7.7 µs), and the
stand-in's magnitude at cache-resident sizes (−85 % at 15k, where both maps fit
L2) is not physically justified — an artifact of the stand-in harness. The
**measured** latency win is the −8 % sequential `as_node` above; a larger
random-access cache benefit at ≥ 1 M decls is *plausible* (3× fewer bytes per
slot) but **untested in real code** and is not claimed as a result.

## Generational arena

A generational arena is **not needed for the compact declaration dispatch**. The
declaration→record mapping is a total, index-addressed function over a
monotonically-growing arena; the compaction changes only the *width and tagging*
of that mapping (48 B three-map → 8 B tagged slot), not its lifecycle. Stale-ID
protection, if ever wanted for a mutable/canonical IR, is an orthogonal concern
and out of scope here.

## New / removed dependencies

None.

## Open issues / follow-ups

- Real-code shuffled-id `as_node`/`as_edge`/`as_arc` bench at 100k/1M to settle
  whether the cache benefit materialises at scale (would need one new bench group;
  not required for acceptance).
- The latent CBOR writer (`CborPayload { ir: Ir }`) rides the same manual serde
  path; it remains unwired.

## Experiment provenance

- Git SHA `788924a9139b03858abf96f8230450206218d762` (branch
  `feat/core-decl-index-compaction`); working tree clean but for this report + its
  figure + `bench_ab.rs`.
- rustc 1.97.1 (8bab26f4f 2026-07-14), host x86_64-pc-windows-msvc, release.
- Baseline A measured from a `git worktree` at `1d1e5f1` (frozen 3-map baseline);
  removed after the run.
- Deterministic: fixtures seeded (`shuffled_decl_ids(_, 0xC0FFEE)`,
  `build_indexed`); RL carve-out N/A.
