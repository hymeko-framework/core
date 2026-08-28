# Phase 10A — IR-stream wire codec and hostile-input validator

**Date:** 2026-08-28
**Base commit:** `2d25ecc4e2b0b5d5ade928295430d71eda2086fa` (tree `85e7975f560a82d9d56723ab434f1702a1d28eab`)
**Authorisation:** `APPROVED-CORE-EDIT: ir-stream-wire-codec`
**Plan:** `docs/plans/2026-08-28-ir-stream-wire-codec/`
**Character:** implementation of the pure validation boundary. No transport, no
transaction engine, no materialisation.

---

## 1. Summary

The Phase 9 audit found that the only existing IPC scaffolding was unusable as a
portable wire format, and — most seriously — that its unchecked offset
arithmetic produced *descending* offsets and a total smaller than its own header
for a hostile length, which a naive length guard would accept before indexing out
of bounds.

Phase 10A implements the replacement: `hymeko::ir_stream`, a transport-independent
validator taking `&[u8]` to a `ValidatedMessage` through envelope decoding,
checked structural validation, digest validation and section-table validation.

* **39 new unit tests**, covering layout, endianness, round-trip, overflow,
  resource limits, corruption, forward compatibility and panic-freedom.
* **The audited hostile values are replayed as tests** and rejected by three
  independent guards.
* **6.5 GiB/s** validation throughput on large payloads — BLAKE3-dominated, as
  intended — and **41 ns** for a header-only message.
* **Rejection costs 6.5–8.8 ns against 37.4 µs for acceptance**, so a garbage
  flood costs the receiver ~5 700× less than a valid message. A validator
  without that property is a denial-of-service amplifier.
* The legacy module is **quarantined, not deleted**, and its unchecked
  arithmetic is gone.

No IR semantics, parser, resolution, module-store, HNN kernel or WGPU code was
touched. No dependency added or changed. No `unsafe`. No async runtime.

---

## 2. Baseline recorded before modification

Per the brief, the `ipc` configuration was built and tested *before* any edit:

```
cargo check -p hymeko_core --features ipc   -> Finished, clean
cargo test  -p hymeko_core --features ipc   -> 17 + 152 passed, 1 failed
```

The single failure is the known `test_load_meta_geometry` 57/58 fixture drift,
untouched throughout. `shared_state` had **zero** tests at baseline.

---

## 3. What was built

| module | code | doc | test | responsibility |
|---|---:|---:|---:|---|
| `ir_stream/mod.rs` | 12 | 96 | 0 | re-exports, design rules |
| `ir_stream/error.rs` | 154 | 44 | 0 | one variant per validation rule |
| `ir_stream/limits.rs` | 32 | 44 | 30 | resource ceilings |
| `ir_stream/version.rs` | 59 | 48 | 43 | four-component `SnapshotVersion` |
| `ir_stream/envelope.rs` | 186 | 117 | 185 | 144-byte header |
| `ir_stream/section.rs` | 328 | 164 | 273 | section table and bounds |
| `ir_stream/codec.rs` | 88 | 84 | 163 | `validate_message`, `encode_message` |

Public API, as sketched in the brief:

```rust
pub fn validate_message<'a>(bytes: &'a [u8], limits: &DecodeLimits)
    -> Result<ValidatedMessage<'a>, WireError>;
```

plus `DecodeLimits`, `ValidatedEnvelope`, `ValidatedMessage<'a>`,
`ValidatedSection<'a>`, `SnapshotVersion`, `MessageType`, `SectionKind`,
`ElementKind`, `SectionEntry`, `WireError`, and `encode_message` so producers
and tests build messages through the same rules the validator enforces.

---

## 4. The wire-format contract, rule by rule

Every requirement in the brief, and where it is enforced and tested.

| requirement | implementation | test |
|---|---|---|
| magic validated before anything else | first check after the length check | `magic_is_checked_before_anything_else` |
| supported schema version | rejected at `SCHEMA_VERSION` mismatch | `unsupported_schema_is_refused` |
| header length validated | bounded `[144, 4096]` | `header_length_bounds_are_enforced` |
| every integer explicitly little-endian | `rd_u16`/`rd_u32`/`rd_u64` copy into a fixed array, then `from_le_bytes` | `encoding_is_little_endian_on_every_host` |
| never cast the buffer to a struct | no `as *const T`, no struct read anywhere | — (absence; grep-checkable) |
| never `transmute` | none in the module | — |
| no reference into unaligned wire bytes | sections expose `&[u8]`; typed access **copies** | `typed_access_copies_and_respects_the_declared_element_kind` |
| no `usize`/`isize`/`bool`/`char`/enum on the wire | all wire integers fixed-width; discriminants are `u16` | `every_message_type_round_trips_through_its_wire_value` |
| discriminants validated before mapping | `try_from_wire` on `MessageType`, `SectionKind`, `ElementKind` | `unknown_message_types_and_flags_are_refused` |
| checked `u64` → `usize` | `usize::try_from` → `NotAddressable` | covered in section bounds |
| reject arithmetic overflow | `checked_mul`/`checked_add` throughout | `hostile_lengths_from_the_phase9_audit_are_rejected` |
| reject truncated / inconsistent lengths | exact-length rule | `a_short_buffer_is_rejected` |
| trailing-byte policy defined and tested | **rejected**; `len == header + payload` exactly | `trailing_bytes_are_rejected` |
| canonical BLAKE3 coverage | payload only, `payload_len` bytes | `any_payload_bit_flip_is_caught_by_the_digest` |
| compare received and computed digest | before any structure is parsed | `a_flipped_hash_field_is_caught` |
| only validated sections exposed | `ValidatedSection` constructible only inside the validator | type-level |

### 4.1 Two policies fixed here

**Trailing bytes are rejected.** The buffer length must equal
`header_bytes + payload_len` exactly. A framing that tolerated trailing bytes
would let two consumers disagree about where a message ends, which is the
ambiguity that makes resynchronisation unreliable.

**Digest covers the payload only**, as specified in
`docs/hypergraph-synchronization.md` §5.1. The envelope is protected by
structural validation — magic, schema, header length, known type, zero reserved,
exact total length. What the digest does *not* provide is **authenticity**: an
attacker able to rewrite bytes rewrites both payload and hash. That is a
transport concern (authenticated channels), and is deliberately not solved here
rather than solved badly.

### 4.2 Ordering of the pipeline

Deliberate, and load-bearing:

1. envelope structure — so a foreign protocol fails on the magic, not on a
   nonsense message type;
2. **limits — before the digest**, because hashing an attacker-declared
   multi-gigabyte buffer is itself a denial of service;
3. exact length;
4. digest — **before** the section table, so no attacker-chosen structure is
   interpreted until the bytes are known to be the ones the producer sent;
5. section table.

`limits_are_enforced_before_hashing` asserts step 2 by declaring a `u64::MAX`
payload and requiring `PayloadTooLarge` rather than a length or hash error.

---

## 5. Section table

Implemented: checked table length and entry count; strictly increasing offsets;
non-overlap; bodies after the table area; end offsets within the payload; checked
`offset + length`; singleton uniqueness; skippable unknown sections; rejection of
unknown *mandatory* sections; explicit alignment; deterministic iteration.

**Alignment policy.** Bodies must start on a **4-byte** boundary, matching the
widest element type carried (`u32`/`f32`). Gaps of up to three bytes are
therefore permitted purely as alignment padding, and are covered by the digest
like any other byte.

**Criticality boundary.** Kinds below `CRITICAL_SECTION_MAX = 1000` must be
understood; at or above it they become `SectionKind::Unknown(raw)` and are
skippable but preserved, so a consumer can forward what it cannot interpret.
This is the schema-evolution seam: a section whose absence would change meaning
is numbered below the boundary, and an old consumer refuses rather than
computing on partial data.

**Determinism.** Sections iterate in table order, which validation proves equals
increasing byte order.

### 5.1 The audited defect, replayed

`hostile_lengths_from_the_phase9_audit_are_rejected` feeds the measured values
(`2^61`, `2^61 − 1`, `u64::MAX`) as `element_count`, `byte_offset` and
`byte_len`. Three independent guards each reject them alone:

1. checked `element_count × element_size` and `byte_offset + byte_len`;
2. strictly increasing, non-overlapping offsets — this alone rejects the
   descending offsets the old code produced;
3. `element_count × element_size == byte_len` exactly.

---

## 6. Resource limits

| limit | default | bounds |
|---|---:|---|
| `max_message_bytes` | 256 MiB | whole message |
| `max_payload_bytes` | 256 MiB | payload |
| `max_section_count` | 1024 | **the only allocation the validator performs** |
| `max_section_bytes` | 128 MiB | one section body |
| `max_elements_per_section` | 64 M | what a consumer would later allocate |

Sized against measured workloads rather than picked round: the `large` workload
has 1 642 260 incidences, so a full snapshot is ~13 MiB of indices and weights —
an order of magnitude of headroom without permitting an unbounded request.

`a_tiny_message_cannot_request_an_enormous_allocation` sends an 8-byte payload
declaring `u32::MAX` sections and requires `TooManySections` — the count check
precedes `Vec::with_capacity`, so the allocation is never attempted.

---

## 7. Legacy `shared_state` — compatibility decision

**Option A: quarantine in place.** Chosen as the narrowest defensible migration.

The brief warned explicitly not to treat "unused in this repository" as "unused".
`hymeko_core` builds as `cdylib` **and** `rlib`, so downstream callers cannot be
enumerated from here. The module path and the `ipc` gate are therefore kept and
**no import breaks**. What changed:

* every public item carries `#[deprecated]` naming the replacement;
* the module header states plainly that it is **not a wire protocol** and lists
  all six measured defects;
* `contiguous_offsets` and `calculate_required_bytes` are now **checked**,
  returning `Option`;
* a compile-time assertion records that `MAX_NNZ` is smaller than this
  repository's own `large` workload, so the defect cannot quietly stop being
  true.

**The one deliberate breaking change** is those two return types. Preserving the
unchecked behaviour was explicitly not authorised, and it is precisely the defect
that produced descending offsets. `the_phase9_hostile_values_now_report_instead_of_wrapping`
asserts they now return `None`; `offsets_are_monotone_whenever_they_are_produced`
asserts the property the old implementation violated — whenever `Some` is
returned, the offsets are monotone and the total is at least the header.

A module-scoped `#![allow(deprecated)]` was required because the `ZeroCopySend`
derive expands to code naming the type it is applied to. It is scoped to the
quarantined module with a comment stating why (§6.3), and covers nothing else.

**`ipc` feature.** Retained, still gating `shared_state`, and still pulling
`iceoryx2` when enabled. `ir_stream` is deliberately **unconditional**: it needs
only BLAKE3, which is already a dependency, so the hostile-input validator is
always compiled and always tested rather than only in a configuration someone
must remember to enable.

---

## 8. Tests

| gate | result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass, 0 issues |
| `cargo clippy -p hymeko_core --features ipc --all-targets -- -D warnings` | pass, 0 issues |
| workspace tests (`--no-fail-fast`) | **377 passed**, 1 pre-existing failure |
| `hymeko_core --features ipc` | **212 passed**, 1 pre-existing failure |
| doctests | 1 passed (the `ir_stream` example) |
| codec benchmark smoke | 12 cases, 0 errors |

Workspace tests rose 337 → 377. The single failure throughout is the known
`test_load_meta_geometry` 57/58 fixture drift — untouched and unsilenced.

**39 new `ir_stream` tests**, by category: layout and offsets (2), endianness
(2), round-trip (3), envelope structure (6), overflow and hostile lengths (4),
resource limits (3), corruption (2 — including an exhaustive single-bit-flip
sweep over a 64-byte payload), section bounds (8), compatibility (2),
panic-freedom (3), version semantics (4). Plus **3 new tests** on the quarantined
module, which had none.

---

## 9. Performance

Budget from the plan: ≥1 GB/s on payload; ≤5 µs fixed cost for a header-only
message.

| case | time | throughput |
|---|---:|---:|
| header only (0 payload) | **41.2 ns** | — |
| 1 KiB payload | 773 ns | 1.46 GiB/s |
| 16 KiB | 2.53 µs | 6.09 GiB/s |
| 256 KiB | 37.5 µs | 6.51 GiB/s |
| 4 MiB | 603 µs | 6.48 GiB/s |

**Both budgets met with wide margin** — 6.5 GiB/s ≈ 7.0 GB/s against a 1 GB/s
floor, and 41 ns against a 5 µs ceiling. The plateau at 6.5 GiB/s from 16 KiB
upward is BLAKE3's own rate, which is exactly the intended shape: the validator
adds no per-byte work of its own.

Section-table overhead, at a fixed 256 KiB payload:

| sections | time |
|---:|---:|
| 1 | 37.19 µs |
| 8 | 37.33 µs |
| 64 | 38.09 µs |
| 512 | 41.35 µs |

511 additional entries cost 4.16 µs — **≈8 ns per entry**, negligible against
the digest.

**Rejection is far cheaper than acceptance**, which is the anti-amplification
property:

| rejection | time | vs. acceptance |
|---|---:|---:|
| bad magic | **6.51 ns** | 5 750× cheaper |
| oversized payload | **8.84 ns** | 4 230× cheaper |
| corrupt payload | 37.4 µs | same as acceptance |

The first two short-circuit before the digest. The third necessarily costs the
same as acceptance — detecting corruption *requires* hashing — and is recorded
rather than glossed over: an attacker who sends well-framed messages with wrong
digests can still force full-payload hashing, which is a rate-limiting concern
for the transport layer, not something this layer can avoid.

---

## 10. What was deliberately not built

Not authorised and not implemented: transaction application, `TransactionStaging`,
snapshot publication, dynamic `HyperGraphView` mutation, iceoryx2
publish/subscribe, async runtime, a general serialization framework, GPU code,
HNN kernel changes.

`encode_message` is deliberately minimal: it fills in `payload_len` and the
digest and concatenates. It does **not** build section tables — a producer
composes its own payload — which keeps it to the one job the format constrains
and stops it growing into the serialization framework the brief forbids.

---

## 11. Mistakes and findings

1. **A silent no-op edit.** The insertion of `pub mod ir_stream;` into
   `hymeko_core/src/lib.rs` did not apply: the file has **CRLF** line endings, so
   a `^pub mod ir;$` pattern failed to match with `$` sitting before `\r`.
   `cargo check` then passed *vacuously* — the module was simply never compiled.
   Caught only because the test list showed no `ir_stream` tests. The lesson is
   that a clean build after adding a module proves nothing unless the module's
   tests appear; the fix was verified by test enumeration, not by a green check.
   `hymeko_wgpu/src/lib.rs` is LF and Phase 9's equivalent edits were confirmed
   unaffected.
2. **A constant assertion in a test.** The check that `MAX_NNZ` is smaller than
   the `large` workload was written as a `#[test]` on two constants — clippy
   correctly flagged it. It is now a `const _: () = assert!(...)`, which is
   stronger: it fails at compile time rather than at test time.
3. **`plan.pdf` was not produced.** CLAUDE.md §2 requires four plan artifacts;
   `pdflatex`, `lualatex` and `latexmk` are all absent on this host.
   `plan.tex`, `plan.tikz` and `plan.mmd` exist and the gap is recorded in the
   plan itself (§9) and here. §11 would normally halt on this; the overnight
   brief authorises autonomous execution, and a missing document renderer is a
   tooling gap rather than a scope or safety question, so the work proceeded with
   the gap declared rather than silently skipped.
4. **`hymeko_core/Cargo.toml` was edited** to register the benchmark target,
   because `autobenches = false` makes registration mandatory and the benchmark
   was explicitly authorised. The edit is additive and follows the exact
   precedent of the `csr-spmv-spmm-into` token, including the citing comment. **No
   dependency was added, removed or changed**; the diff is seven lines, all
   comment and `[[bench]]`.

---

## 12. Compatibility summary

| surface | change | breaks a downstream caller? |
|---|---|---|
| `tensor::shared_state` module path | unchanged | no |
| `ipc` feature | unchanged | no |
| `MAX_NNZ`, `HypergraphWeights`, `ExpansionKind`, `ExpansionHeader`, `ExpansionOffsets` | deprecated, otherwise unchanged | warning only |
| `ExpansionHeader::contiguous_offsets` | returns `Option` | **yes, deliberately** |
| `calculate_required_bytes` | returns `Option` | **yes, deliberately** |
| `hymeko::ir_stream` | new | no |
| dependencies | none | no |

---

## 13. Limitations

* The codec is the *only* layer implemented. Sequence-gap detection, stale-base
  rejection, transaction staging, resync, coalescing and snapshot lifetime are
  specified in `docs/hypergraph-synchronization.md` and remain unbuilt; C1–C5 and
  C7–C12 of that document are therefore still unproved.
* Digest coverage excludes the envelope (§4.1), and the format provides
  integrity, not authenticity.
* Benchmarks ran at survey resolution (`--measurement-time 2 --sample-size 10`)
  on one host. Adequate to establish the plateau and the rejection ratio; not a
  regression gate.
* Panic-freedom is asserted over a constructed hostile corpus, not proved. A
  fuzzer over `validate_message` is the obvious next step and needs no new
  design.
* Only little-endian hosts were available. The byte-order tests assert explicit
  LE encoding, which is what makes the format host-independent, but no big-endian
  execution was performed.

---

## 14. Which C-claims are now testable, and which hold

From `docs/hypergraph-synchronization.md` §12:

| claim | status |
|---|---|
| C6 — wrapped/hostile lengths rejected, not wrapped | **proved** (`hostile_lengths_from_the_phase9_audit_are_rejected`) |
| C13 — header layout stable | **proved** (`field_offsets_are_where_the_specification_says`) |
| C14 — byte order explicit | **proved** (`encoding_is_little_endian_on_every_host`) |
| C5 — a corrupt payload never mutates live state | **half proved**: corruption is always detected; "never mutates live state" needs the staging layer |
| C2 — idempotence predicate | predicate implemented and tested; the no-op *behaviour* needs the transaction layer |
| C1, C3, C4, C7–C12 | unchanged — require layers not built here |
