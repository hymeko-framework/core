# MCU canonical-IR architecture (plan, not implementation)

2026-09-01. `APPROVED-CORE-EDIT: platform-performance-survey`. This is a **design plan**; no MCU runtime,
no new protocol, and no new IR model are implemented in this phase.

## Principle
**One semantic Ir. Several validated storage/transport profiles. No MCU-specific second model.** The
`.hymeko` parser does not initially run on the MCU:

```
host:  source -> parser -> canonical Ir -> validated sectioned snapshot
MCU:   validate/open snapshot -> direct sparse queries (no parser, no heap)
```

The canonical semantics that must survive onto the MCU unchanged: declaration/incidence identity, the
signed sparse operator B (`ArcRec.refs` rows / reverse `(HyperArcId, ref_slot)` index), `+`/`-`/`~`
signs, weights, duplicate incidence, slot order, and canonical IDs. **No dense graph or clique
expansion; the star stays a sparse incidence operator.**

## 9. Profiles

### MCU-RO (read-only)
- Read-only sectioned snapshot in flash or mmap-like memory; **zero heap allocation during queries**.
- Fixed-width wire IDs (u32/u64, §11); canonical declaration/incidence semantics; direct sparse
  traversal (node -> incident arcs -> refs -> node); caller-owned scratch (the `TraversalWorkspace` shape).
- Target: Cortex-M / ESP32 / STM32-class. Budget: `tiny`/`medium` (§13).

### MCU-DYNAMIC
- Bounded mutation/delta into a **caller-provided fixed-capacity arena**; Begin/Chunk/Commit/Abort
  (the existing `TransactionAssembler` lifecycle) with a staging buffer; **full validation before
  commit**; atomic active-snapshot swap; power-loss recovery via a journal or dual-bank (A/B) plan;
  a transaction-updated reverse-incidence index; rollback leaves no partial state.
- Target: `medium` (512 KiB-2 MiB RAM).

### MCU-HOSTED
- Larger MCU/RTOS/Linux-class edge device; optional allocator; larger snapshots; multiple query
  workspaces; transport/telemetry integration.
- Target: `large` (4-32 MiB RAM).

## 10. C wire view (REUSE the existing ir_stream format; no new protocol)

The `src/ir_stream/` envelope+section format already satisfies every §10 requirement -- it is the MCU
wire view; the C side is a decoder for it, not a new protocol.

Existing format (audited): 144-byte **Envelope** (`MAGIC 0x484D4B53 "HYMKS"`, `schema_version:u16=1`,
byte-wise **little-endian**, `msg_type`, `stream_id/seq/txn_id:u64`, `base/target:SnapshotVersion=4xu64`,
`payload_len:u64`, `payload_hash:[u8;32]` BLAKE3). **Section table**
`[u32 count][u32 reserved][SectionEntry(32B) x count][bodies]`; `SectionEntry{kind:u16,
element_kind:u16, reserved:u32, element_count:u64, byte_offset:u64, byte_len:u64}`; section kinds
`EdgeOffsets/EdgeNodes/IncidenceWeights/NodeFeatures/ModelParams/SignPattern` + delta patch kinds +
skippable `Unknown(u16)`. It already provides: `uint{8,16,32,64}_t` fields, little-endian, **no usize**,
no enum/bool/pointer on the wire, offset+length addressing, schema version, header length, payload
length, digest/integrity, deterministic section order, and skippable unknown sections.

C requirements (already met by ir_stream; the C header adds the compile-time guards):
- checked add/multiply on every `offset + len` before access; alignment-independent reads (memcpy or
  byte assembly, never a cast-through-pointer); `static_assert` on struct size/offset in the C header.

Sketched C API (architectural only; finalise after the implementation PoW):

```
typedef struct hymeko_ir_view    hymeko_ir_view_t;
typedef struct hymeko_ir_scratch hymeko_ir_scratch_t;

hymeko_status_t hymeko_ir_open(const uint8_t *bytes, size_t len, hymeko_ir_view_t *out);
hymeko_status_t hymeko_ir_decl(const hymeko_ir_view_t *ir, uint32_t decl_id, hymeko_decl_ref_t *out);
hymeko_status_t hymeko_ir_incident_begin(const hymeko_ir_view_t *ir, uint32_t node_id,
                                         hymeko_incidence_iter_t *out);
hymeko_status_t hymeko_ir_reachable(const hymeko_ir_view_t *ir, uint32_t start,
                                    hymeko_ir_scratch_t *scratch, hymeko_id_buffer_t *out);
```

C++: a thin RAII/span wrapper over the SAME C view -- no separate C++ data model, exception-free
profile, optional `std::span`/iterator façade, no owned graph copy.

## 11. ID and numeric contract on the MCU

Hazard found (audit): the `Ir` **serde** path serialises `Id<T>` as bare `usize` and `DeclKind`/
`SignedRefR` as serde-tagged enums -- usize width, `NONE = usize::MAX`, and tagged enums are **not**
32-bit-portable. The MCU path therefore uses the **ir_stream fixed-width section encoding**, never the
serde/JSON path.

| canonical semantics | wire encoding (MCU) | MCU-supported subset | rejection behaviour |
|---|---|---|---|
| DeclId/NodeId/EdgeId/HyperArcId | u32 (default) or u64 (large profile), LE | up to 2^32-1 decls on 32-bit; NONE = all-ones sentinel | id >= declared count -> typed `OutOfRange` |
| SymId / interned strings | u32 offset into a string section | bounded table | offset+len overflow -> `Malformed` |
| sign +/-/~ | i8 (+1/-1/0) in a `SignPattern` section | full | value not in {-1,0,1} -> `Malformed` |
| weight (f64) | f32 or f64 section, explicit; **never implicit f64->f32 downcast** | profile MAY restrict to f32 | unsupported ValueR shape -> typed error, never silent |
| duplicate incidence / slot order | preserved (each incidence a distinct coordinate) | full | reorder/dedup is a corruption -> reject |
| ValueR List/Ref/Str | packed value section | profile MAY restrict (e.g. numeric-only) | unsupported shape -> typed error |
| star operator | sparse incidence B (rows = refs, reverse index = Bᵀ) | full; **no dense/clique expansion** | a dense/clique input is out of contract |

Rule: an MCU profile MAY *restrict* the supported subset, but MUST NOT *silently change meaning*. Every
restriction is a typed rejection, not a reinterpretation. Overflow is checked; endianness is fixed LE;
alignment is handled by byte reads.

## 12. CBOR vs sectioned snapshot -- decision (evidence-backed; falsifiable)

The repository's only CBOR artefact (`writers/cbor_writer.rs::CborPayload`) is **dormant and would be a
full-state copy**: it is never constructed or serialised, no CBOR codec is even a dependency (ciborium
is a transitive dev-dep of criterion only), and it clones the entire Ir + Index + every interned string.
It is not canonical and not incremental.

The sectioned ir_stream snapshot, by contrast, is the wired, deterministic, MCU-suitable format
(offset+length, no usize, per-payload digest, skippable sections, delta patch kinds).

| axis | A. deterministic CBOR into fixed arena | B. sectioned packed snapshot (zero-copy RO) | C. CBOR metadata + packed numeric sections |
|---|---|---|---|
| flash bytes | larger (tag+length per item) | smallest (packed) | middle |
| RAM peak | high (decode into arena, full copy) | ~0 (view over flash) | middle |
| decode cycles | per-item tag parse | offset table + memcpy | mixed |
| zero-copy | no | **yes** | partial |
| schema evolution | good | **skippable Unknown sections** already provide it | good |
| C decoder complexity | a CBOR parser | a bounds-checked offset reader | both |
| streaming / delta | weak | **Begin/Chunk/Commit + patch kinds** | mixed |
| corruption rejection | per-item | digest + checked offsets | both |
| code size | larger (CBOR lib) | small | larger |

**Decision:** use the **single sectioned snapshot** as the MCU resident zero-copy / read-mostly format.
It is the same schema and the same canonical Ir -- not two models. A CBOR path is **not** adopted: it
would duplicate the schema for a lossy full-copy interchange the project does not currently use. The
dormant `CborPayload` is flagged for removal or explicit repurposing (host-only debug interchange), not
promotion to a wire format. **Falsifier:** if a real interoperability requirement for RFC-8949 CBOR
appears (a third-party consumer that cannot read the sectioned format), option C (a CBOR envelope
wrapping the packed sections) is the least-duplicative addition -- but no such requirement exists today.

## 13. MCU measurement plan and budgets

Target classes: `tiny` (128-256 KiB RAM / 512 KiB-2 MiB flash, static small model), `medium`
(512 KiB-2 MiB / 2-16 MiB, dynamic sub-model), `large` (4-32 MiB / 16 MiB+, full edge runtime).

Metrics to capture (when the probe exists): snapshot bytes; decoder code size; peak stack; peak RAM;
validation cycles/byte; lookup cycles; incidence-iteration cycles; reachability cycles/incidence; delta
apply; commit latency; power-loss recovery; energy/query where measurable.

Budget projection from the host measurements (this survey): the sparse index is ~12-15 B/incidence
(canonical, no dense expansion); a 10^4-incidence sub-model is ~150 KiB of packed sections + node/decl
tables -> fits a `medium` MCU; a 10^3-incidence model fits `tiny`. Host resident reachability is
~3-13 ns/incidence on a desktop core; an MCU at ~100-300 MHz will be ~10-40x slower per incidence but
still sub-ms for 10^3-10^4 incidences (no allocation, streaming the section arrays). The Bᵀy-style
arc-scatter penalty (measured ~3.4-4.9x over Bx on the host, §incidence) is the term to watch on
cache-poor MCUs; a `SignPattern`+weight co-located section would mitigate it (a documented future
option, not implemented).

**First implementation PoW (plan only here):** a host-compiled C11 layout/accessor probe that reads the
**same frozen KAT bytes** on the Rust and C sides (the ir_stream envelope+section KATs), under
ASan/UBSan, against a malformed-input corpus, with **no dynamic allocation** -- then a Cortex-M / ESP32
cross-compile. No production C is committed in this phase.
