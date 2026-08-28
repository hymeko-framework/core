# Hypergraph synchronization — protocol and lifecycle design

**Date:** 2026-08-28 · **Status:** design.
Supersedes the first draft of this note, which treated dynamic and distributed
synchronization as hypothetical extensions.

> **Implementation status.** Phase 10A implemented §5.1 (the envelope) and §5.3
> (the section table) as `hymeko::ir_stream`, together with the resource limits
> and the digest rule — see
> `reports/2026-08-28-phase10a-ir-stream-wire-codec.md`. Claims **C6, C13 and
> C14** of §12 are now proved by tests; C5 is half proved. **Everything else in
> this document remains unimplemented**: the transaction lifecycle (§6), CPU
> snapshot publication (§7), orientation rebuilding (§8) and GPU materialization
> and retirement (§9) are all still design only.

---

## 0. The requirement

> The eventual source of truth is a **HyMeKo IR stream transported through an
> IPC/channel**, with the IR **changing dynamically while inference may already
> be running**.

That is a primary requirement, and it changes the shape of this document. The
system is not "an immutable snapshot with a possible future update path"; it is
a **replicated state machine** whose authority lives outside the process, whose
updates arrive asynchronously, and whose consumers must remain correct while a
GPU submission built from an older state is still in flight.

Three consequences follow immediately and constrain everything below.

1. **A consumer can never assume its state is current.** Every derived artifact —
   node-major index, expansion, uploaded buffer, prepared operation — must be
   attributable to a specific version, or staleness is undetectable.
2. **Applying an update is a transaction, not an assignment.** A topology change
   invalidates several derived structures at once; a consumer that observes them
   half-updated computes a wrong answer with no error.
3. **In-flight GPU work outlives the snapshot it was built from.** Freeing an old
   snapshot when the new one is published is a use-after-free on the device
   timeline, and WGPU will not diagnose it.

---

## 1. Audit of the existing `ipc` feature

### 1.1 What exists

`hymeko_core/Cargo.toml` declares `ipc = ["dep:iceoryx2"]` (iceoryx2 0.8.1),
gating exactly one module: `hymeko_core/src/tensor/shared_state.rs`, 91 lines.
It declares `HypergraphWeights`, `ExpansionKind`, `ExpansionHeader`,
`ExpansionOffsets`, `contiguous_offsets` and `calculate_required_bytes`.

The feature compiles. **The module is referenced nowhere** — `pub mod
shared_state;` in `tensor/mod.rs` is its only mention in the repository. It is
unused scaffolding, which is the best possible position to be in before a
redesign: nothing depends on its shape.

### 1.2 Measured facts

Layout and arithmetic were measured, not read off the source, by replicating the
declarations verbatim in a standalone program on this target (x86-64, Windows):

```
HypergraphWeights: 4194320 bytes (4.00 MiB), align 8
ExpansionHeader:   72 bytes, align 8
  field offsets: version 0, kind 4, nnz 8, etag 16, dim_k 48, dim_i 56, dim_j 64
usize on this target: 8 bytes
native byte order: little-endian
```

Unchecked offset arithmetic in `contiguous_offsets`, release semantics:

| `nnz` from the wire | k | i | j | values | total |
|---|---:|---:|---:|---:|---:|
| 2^61 | 72 | 72 | 72 | 72 | 9223372036854775880 |
| 2^61 − 1 | 72 | 64 | 56 | 48 | 9223372036854775852 |
| `u64::MAX` | 72 | 64 | 56 | 48 | **44** |

### 1.3 What is reusable

Three things are worth keeping as *ideas*, and one as a dependency.

* **The SoA payload layout.** `[header][k: i64×nnz][i: i64×nnz][j: i64×nnz][val:
  f32×nnz]` is the right shape: contiguous per-component arrays upload to GPU
  buffers directly, with no host-side deinterleaving. The new design keeps
  component-major sections. (The index width should narrow to `u32` — the GPU
  path narrows anyway, and `i64` doubles the transfer for a range WGSL cannot
  index.)
* **A versioned header preceding the payload.** `ExpansionHeader::VERSION = 1` is
  a real schema version, distinct from the data version. That distinction is
  correct and is preserved.
* **A digest field.** `etag: [u8; 32]` is exactly the right size for BLAKE3, and
  the crate already depends on `blake3` for canonical IR hashing. **It is never
  populated** — `new()` writes `[0u8; 32]` and nothing else assigns it — so today
  it is a field that looks like integrity checking and provides none. The slot is
  reusable; the absence of a writer is a defect.
* **iceoryx2 as a transport.** Appropriate for the same-host zero-copy case and
  worth retaining as one transport binding. It is not a wire format.

### 1.4 What is unsuitable as a portable wire protocol

Assessed against the specific axes the requirement names.

**Fixed-size payloads.** `values: [f32; MAX_NNZ]` makes `HypergraphWeights`
4 MiB unconditionally. A single-weight update carries 4 194 320 bytes to deliver
4 useful ones. Worse, it is simultaneously *too small*: `MAX_NNZ = 1_048_576`,
while the `large` workload in `hymeko_hnn::workloads` has 1 642 260 incidences —
**the existing benchmark workload does not fit**, and nothing in the struct
detects that. `nnz` is a plain field; a consumer reading `values[..nnz]` with
`nnz > MAX_NNZ` reads past the array.

This is a fair consequence of `ZeroCopySend`, which requires a fixed-size POD
type: it is a reasonable *shared-memory slot* and an unreasonable *message*. The
new design separates the two — a message is variable-length and framed; a
transport may still place it in a fixed shared-memory region.

**`usize` fields.** `HypergraphWeights::nnz` and all six `ExpansionOffsets`
fields are `usize`: 8 bytes here, 4 on a 32-bit consumer or an embedded/FPGA
control endpoint. A `repr(C)` struct containing `usize` has **no stable layout
across targets**, so the same bytes decode differently on each side. Every wire
integer must be fixed-width.

**Checked arithmetic.** `contiguous_offsets` and `calculate_required_bytes`
perform `nnz * size_of::<i64>()` and chained additions with no checking, on an
`nnz` that arrives from the wire. §1.2 shows the result is not merely a wrong
number: at `nnz = u64::MAX` the offsets **descend** (72, 64, 56, 48) and
`total_bytes` is **44 — smaller than the 72-byte header**. A consumer performing
the obvious `buffer.len() >= total_bytes` guard *passes*, then indexes at
offsets 48–72 in a 44-byte buffer. This is the single most serious defect found,
and it is reachable from any corrupt or hostile producer.

**Endianness.** `repr(C)` is native-endian and nothing declares an order. Two
little-endian x86 hosts agree by accident. A big-endian endpoint, or an FPGA
control plane with its own convention, silently misreads every field. The wire
format must fix an order (little-endian) and convert explicitly.

**Alignment and padding.** `ExpansionHeader`'s 72 bytes happen to contain no
interior padding (§1.2). That is luck, not design: nothing asserts the size or
any offset, so reordering a field or inserting one would silently change the wire
image. Padding bytes are also uninitialised, so a naive `as_bytes` of a struct
with padding would hash and transmit indeterminate values.

**Schema evolution.** `ExpansionHeader::VERSION` exists but there is no rule for
what a consumer does with an unexpected value, no header-length field, and no
reserved space. A producer cannot add a field without breaking every consumer.
Relatedly, `kind: ExpansionKind` is a `repr(u32)` enum **in a wire struct**: only
1 and 2 are valid, and materialising any other bit pattern into that type is
undefined behaviour. Wire enums must be plain integers with a checked
conversion.

**Recovery.** There is none, and nothing to build it from: no sequence number, no
stream identity, no transaction identity, no acknowledgement, no way to express
"I have version X, send me what I am missing." A dropped or reordered message is
undetectable.

### 1.5 Conclusion of the audit

`shared_state.rs` is a same-host shared-memory slot for one full expansion. It is
**not** extended into the protocol below. The SoA layout, the schema/data version
split, the digest slot and iceoryx2-as-transport carry forward; the fixed-size
array, the `usize` fields, the unchecked offsets, the implicit endianness, the
wire enum and the absent framing do not.

Because the module has no consumers, replacing it costs nothing. It lives in
`hymeko_core`, which is `lockdown: full` — the eventual change is a core edit and
needs its own `APPROVED-CORE-EDIT` token. This document does not make it.

---

## 2. The four meanings, re-ranked

| # | Meaning | Status under the new requirement |
|---|---|---|
| S-E | Execution dependency (`gather → scale → scatter` on one queue) | Still solved. §3 |
| S-R | Representation coherence (edge-major ↔ node-major) | **No longer free.** Was guaranteed by immutability; dynamic topology removes that guarantee. §8 |
| S-H | Host/device coherence (CPU snapshot ↔ GPU upload) | **Primary.** §7–§9 |
| S-D | Distributed coherence (partition halos) | Still deferred. §11 |

The first draft concluded S-R was guaranteed by construction. That was true of a
static system and is **false** under this requirement: the two orientations are
derived from one view, so a topology update invalidates both, and a consumer that
rebuilds one before the other has a window where they disagree. Publishing them
atomically (§7) is what restores the guarantee.

---

## 3. What still holds

Three properties of existing code survive the change and are load-bearing below.

* **Ordering carries execution dependency.** WGPU inserts barriers between
  compute passes in one encoder; one queue completes in submission order. A
  resident chain needs one submit and no host wait. Per-incidence locks or
  atomics remain unjustified — gather and scatter have disjoint output ownership
  by construction, and that does not change when the topology does.
* **`SubmissionEpoch` and the completion watermark already solve retirement.**
  Phase 5 built a monotone watermark over `Queue::on_submitted_work_done`, and
  Phase 4's `TensorArena` already refuses to recycle a buffer until its
  submission completes. Snapshot retirement (§9) is the same problem and reuses
  the same mechanism rather than inventing a second one.
* **Provenance types already prevent the *memory-safety* half.** `TensorOperand`
  sealing and operand lifetimes stop a prepared operation outliving its buffers.
  What they do not stop is a prepared operation binding buffers that are
  *valid but stale* — that is what versioning adds.

---

## 4. Version model

Four independent monotonic counters, plus a wire schema version that is a
property of the *message*, not the data.

```rust
/// Data versions. Every derived artifact records the tuple it came from.
struct SnapshotVersion {
    topology: u64,   // incidence structure: nodes, edges, which node is in which edge
    weights:  u64,   // incidence weights; structure unchanged
    features: u64,   // node/edge feature tensors
    params:   u64,   // model parameters (layer weight matrices)
}
```

**Why four and not one.** They have different invalidation blast radii and
different transport costs, and collapsing them would force a full rebuild for a
weight tweak. Measured justification from the Phase 9 baseline: rebuilding a
`HyperGraphView` at large scale costs 32.5 ms from a design and 91.1 ms from IR,
`build_explicit_a` costs 724 ms, and clique expansion 456 ms. A weight-only
change that triggered those would cost ~1.3 s to deliver new numbers into an
unchanged structure.

**Ordering rules.**

* Each counter is strictly increasing within a stream. Equal means identical.
* `topology` is the **dominant** counter. A topology bump invalidates the meaning
  of the other three, because they are indexed by structure. A transaction that
  bumps `topology` **must** carry new `weights` and `features` values in the same
  transaction (`params` may legitimately be unchanged — model parameters are
  indexed by feature dimension, not by topology).
* The other three are mutually independent.

Comparison is component-wise; there is no total order across the tuple, and
consumers must not invent one.

---

## 5. Wire protocol

### 5.1 Framing

Every message is `[header][payload]`. All integers **little-endian**, converted
explicitly, never by `repr(C)` transmute. No `usize` anywhere. No enums as
fields — discriminants travel as integers and are converted with a checked
`TryFrom` that rejects unknown values.

**Header — 144 bytes, fixed, statically asserted for both size and every field
offset.**

| off | field | type | meaning |
|---:|---|---|---|
| 0 | `magic` | `u32` | `0x484D_4B53` (`"HYMKS"` truncated). Cheap desync detector. |
| 4 | `schema_version` | `u16` | Wire format version. §5.5 |
| 6 | `header_bytes` | `u16` | This header's length. §5.5 |
| 8 | `msg_type` | `u16` | Plain integer; checked conversion. §5.2 |
| 10 | `flags` | `u16` | Bit 0 `LAST_CHUNK`. Others reserved, must be 0. |
| 12 | `reserved` | `u32` | Must be 0. |
| 16 | `stream_id` | `u64` | Producer/stream identity. §5.4 |
| 24 | `seq` | `u64` | Per-stream, strictly `+1`, never reused. §6.3 |
| 32 | `txn_id` | `u64` | Transaction this message belongs to. §6 |
| 40 | `base_version` | `4 × u64` | What the producer believes the consumer holds. |
| 72 | `target_version` | `4 × u64` | What applying this transaction yields. |
| 104 | `payload_len` | `u64` | Payload bytes following the header. |
| 112 | `payload_hash` | `32 × u8` | BLAKE3 of exactly those bytes. |

Every field is naturally aligned and there is no padding — asserted, not assumed.
`payload_len` is bounded by a negotiated maximum (§5.4) and validated before any
allocation.

**All offset and length arithmetic is checked.** Concretely, the failure in §1.2
is prevented by three independent guards, any one of which catches it:
`payload_len` against the negotiated maximum; every section offset computed with
`checked_mul`/`checked_add`; and a **monotonicity check** on section offsets —
they must strictly increase and the last must equal `payload_len`. The wrapped
case produced descending offsets, so the monotonicity check alone rejects it.

### 5.2 Message types

Producer → consumer:

| type | meaning |
|---|---|
| `Hello` | Stream identity, schema version, negotiated limits, producer's current version. |
| `SnapshotBegin` | Opens a full-snapshot transaction. Payload: section table. |
| `SnapshotChunk` | One or more payload sections. |
| `DeltaBegin` | Opens a delta transaction. Carries `base_version`. |
| `DeltaChunk` | Delta sections. |
| `Commit` | Transaction complete; `target_version` is now authoritative. |
| `Abort` | Transaction abandoned; discard everything staged under `txn_id`. |
| `Heartbeat` | Liveness plus current version, so an idle consumer can detect divergence. |

Consumer → producer:

| type | meaning |
|---|---|
| `Ready` | "I have `target_version` and it is live." §10 |
| `Nack` | "I rejected this transaction", with a reason and the version I still hold. |
| `ResyncRequest` | "I have version V; bring me current." §6.5 |
| `Credit` | Backpressure window update. §6.7 |

### 5.3 Payload sections

A payload is a **section table** followed by section bodies. Each entry:
`section_kind: u16`, `element_kind: u16`, `element_count: u64`, `byte_offset: u64`,
`byte_len: u64`. Offsets are relative to the payload start, must strictly
increase, and the final `byte_offset + byte_len` must equal `payload_len`.

Full-snapshot sections:

| kind | contents |
|---|---|
| `EdgeOffsets` | `u32 × (num_edges + 1)` |
| `EdgeNodes` | `u32 × nnz` (edge-major incidence) |
| `IncidenceWeights` | `f32 × nnz`, edge-major order |
| `NodeFeatures` | `f32 × (num_nodes × feature_dim)` |
| `ModelParams` | `f32 ×` layer-defined |
| `SignPattern` | `i8 × nnz`, optional |

Node-major orientation is **not transmitted**. It is derived, and shipping it
would double the payload to send information the consumer can reconstruct — and
would create a second thing that can disagree with the first. §8.

Delta sections:

| kind | contents |
|---|---|
| `WeightPatchSparse` | `u32 × count` edge-major indices, then `f32 × count` values |
| `WeightPatchRange` | `offset: u64`, `f32 × count` |
| `FeatureRowPatch` | `u32 × count` row ids, then `f32 × (count × feature_dim)` |
| `ParamPatchRange` | layer id, `offset`, `f32 × count` |
| `TopologyEdgeAdd` / `TopologyEdgeRemove` | edge ids and their node lists |
| `TopologyIncidenceAdd` / `TopologyIncidenceRemove` | `(edge, node)` pairs |

**Weight patch indices are always edge-major**, because edge-major is the
transmitted orientation and therefore the only index space both sides share
without ambiguity. The consumer maps them into node-major through the
permutation it built when it derived that orientation (§8).

### 5.4 Stream identity and writer policy

**Single writer per `stream_id`.** A stream is a totally ordered sequence, and
`seq` plus monotone versions presuppose one authority. Two writers on one stream
would need consensus to agree on the next `seq`, which is a distributed-log
problem this system has no reason to take on.

Multiple producers are expressed as **multiple streams with disjoint ownership**
— e.g. one stream owning topology and one owning model parameters — merged by the
consumer, which tracks a version tuple per stream. Overlapping ownership across
streams is **forbidden**, not arbitrated: two streams that can both write
`weights` have no defined resolution order. Deferred and out of scope.

`Hello` negotiates `max_payload_len`, `max_sections`, and the highest schema
version both sides support. A consumer that cannot meet the producer's minimum
replies `Nack` and does not start.

### 5.5 Schema evolution

Two mechanisms, both required.

* **`header_bytes`** lets a newer producer append header fields. An older
  consumer reads the fields it knows and skips to `header_bytes`. Existing fields
  never move or change meaning; `reserved` must be zero so a future field can be
  distinguished from old noise.
* **Unknown section kinds are skippable** — every section carries its own
  `byte_len`, so a consumer steps over what it does not recognise. A section that
  *must* be understood is marked by placing its kind below a
  `CRITICAL_SECTION_MAX` boundary; encountering an unknown critical kind is a
  `Nack`, not a skip.

`schema_version` gates the envelope grammar itself. A consumer that does not
support the producer's `schema_version` refuses at `Hello`, before any state is
touched.

---

## 6. Transaction lifecycle

### 6.1 Commit and abort

A transaction is `Begin → Chunk* → Commit | Abort`, all sharing one `txn_id`.

The consumer **stages** into a builder and never mutates live state until
`Commit`. On `Commit` it validates the complete transaction — every section
present, every hash matched, structural invariants satisfied (§6.2) — and only
then publishes (§7). On `Abort`, or on any validation failure, it discards the
staging area; live state is untouched and the consumer keeps serving the previous
version.

This is what makes a topology change safe under concurrent inference: there is no
observable moment at which the incidence arrays and the derived orientation
disagree, because neither is visible until both are complete.

### 6.2 Validation performed at commit

Cheap, host-side, and paid once per transaction — the same argument as the CSR
checks Phase 9 added to `upload_csr`, and for the same reason: WGPU clamps
out-of-bounds storage access, so an invalid structure produces a wrong number
rather than an error.

* `payload_hash` matches BLAKE3 of the received payload bytes.
* Section offsets strictly increase and cover the payload exactly.
* `EdgeOffsets[0] == 0`, monotonically non-decreasing, final entry `== nnz`.
* Every `EdgeNodes` entry `< num_nodes`.
* Every array length agrees with the header's declared counts.
* Every arithmetic step is checked; overflow is a `Nack`, never a wrap.

### 6.3 Idempotence, duplicates and gaps

**Idempotence comes from absolute target versions.** A transaction states the
version it *produces*, not a relative change. Applying a transaction whose
`target_version` is component-wise `<=` the consumer's current version is a
**no-op**: acknowledge and discard. Retransmission is therefore always safe, and
the consumer needs no dedup table beyond its current version tuple.

Duplicate detection uses `(stream_id, seq)`: a `seq` already seen is dropped
silently. A `seq` **greater than** `last_seq + 1` is a gap — the consumer does not
attempt to apply it, aborts any open transaction, and issues `ResyncRequest`.

**Stale base** is the other direction: a `DeltaBegin` whose `base_version` does
not equal the consumer's current version cannot be applied, because a delta is
meaningful only against the state it was computed from. The consumer replies
`Nack { reason: StaleBase, have: current_version }` and the producer decides
between replaying from the log or sending a full snapshot (§6.5).

Both checks are pure comparisons of values already in the header.

### 6.4 Crash and restart recovery

The consumer's durable state is its version tuple; everything else is
reconstructible. On restart it sends `ResyncRequest { have: last_committed }` —
or `have: ZERO` if it has nothing, which is the same code path.

The producer keeps a **bounded delta log**. If `have` is within the log it
replays deltas; otherwise it sends a full snapshot. Both converge to the same
state, which is the property worth testing (§12).

An interrupted transaction needs no recovery: staging is not durable, so a crash
mid-transaction leaves the last committed version intact by construction. That is
the reason for staging, not merely a benefit of it.

Producer restart is symmetric but changes `stream_id`: a consumer seeing a new
`stream_id` discards its per-stream sequence state and resynchronizes from
scratch. Reusing a `stream_id` across producer restarts is **forbidden**, because
`seq` would restart and duplicate detection would silently drop live messages.

### 6.5 Resynchronization

`ResyncRequest` is the single entry point for every divergence — gap, stale base,
hash failure, restart, or a consumer that has simply fallen too far behind. It
carries the consumer's version tuple and nothing else, so the producer needs no
per-consumer bookkeeping beyond the log.

### 6.6 Backpressure

Credit-based. The consumer advertises a window in `Credit`; the producer must not
have more than that many unacknowledged transactions outstanding. A consumer that
stops issuing credit stalls the producer rather than accumulating an unbounded
queue.

The interesting case is a producer that cannot stall — a live IR editor. It then
**coalesces** rather than queues (§6.7), which is why credit exhaustion is a
normal operating mode rather than an error.

### 6.7 Update coalescing

When the consumer is behind by several transactions, the producer collapses them
before sending, using the version model:

* **Weight-only runs coalesce losslessly.** Union the patched index sets, take the
  latest value for each index, emit one `WeightPatchSparse` with the newest
  `target_version`. Correct because weights are last-writer-wins per index and
  the structure is unchanged throughout.
* **Feature and param runs coalesce the same way**, per row and per range.
* **Any topology change in the run collapses the whole run to a full snapshot.**
  Indices are not comparable across a topology change, so a weight patch from
  before it cannot be merged with one after it. This is a deliberate refusal to
  be clever: the alternative is index remapping across an arbitrary number of
  intermediate structures.

Coalescing is a producer-side optimisation and changes no consumer semantics,
which makes it independently testable (§12).

---

## 7. CPU snapshot publication

The consumer holds:

```rust
struct Snapshot {
    version: SnapshotVersion,
    view: HyperGraphView<...>,   // immutable once built
    node_major: NodeMajorIndex,  // derived, built with the view
    derived: DerivedCache,       // degrees, clique_diag, normalised adjacency
}
live: ArcSwap<Snapshot>          // or Mutex<Arc<Snapshot>>; the swap is the point
```

Publication is **a single atomic pointer swap of a fully-built immutable
snapshot.** A reader takes `Arc::clone` once at the start of an inference step
and uses that snapshot for the whole step, so a mid-step publication cannot make
a step observe two versions. This preserves exactly the property the static
system had for free: `HyperGraphView` remains immutable after construction, and
"updating" means *replacing* rather than mutating.

Building the new snapshot is off the serving path. Under a topology change that
costs the construction figures measured in Phase 9 (32.5 ms from design, 91.1 ms
from IR, plus 724 ms if explicit clique adjacency is needed at large scale), all
of it concurrent with inference on the old snapshot.

---

## 8. Both orientations, and what invalidates them

The consumer derives node-major from the transmitted edge-major, and retains the
**permutation** `perm: edge_major_index → node_major_index` produced while
building it. That permutation is what lets an edge-major weight patch reach both
orientations without transmitting the second one.

| change | edge-major | node-major | permutation | derived cache | GPU buffers |
|---|---|---|---|---|---|
| `weights` | values only | values only, via `perm` | **valid** | degrees / `clique_diag` / normalised adjacency **invalid** | partial write |
| `features` | untouched | untouched | valid | valid | partial write |
| `params` | untouched | untouched | valid | valid | partial write |
| `topology` | **rebuild** | **rebuild** | **rebuild** | **all invalid** | **reallocate** |

**Only a topology change requires rebuilding both orientations.** That is the
whole return on splitting the version tuple.

Note the asymmetry that is easy to get wrong: a weight change does **not**
invalidate structure but **does** invalidate every weight-folding derived
quantity. Degrees, `clique_diag` and normalised adjacency are functions of the
weights, so "weights are cheap" is true for transport and false for derived
state. Any layer holding a precomputed normalised adjacency — `GcnCliqueLayer`
does — must be rebuilt on a weight bump.

---

## 9. GPU snapshot materialization and lifetime

### 9.1 Which changes permit partial buffer updates

| change | GPU action | why |
|---|---|---|
| `weights` | `write_buffer` into the CSR values buffer at patched ranges | Buffer sizes and `row_ptr`/`col_ind` are unchanged. A sparse patch becomes one write per contiguous run. |
| `features` | `write_buffer` into the feature tensor | Same shape; only contents change. |
| `params` | `write_buffer` into parameter buffers | Independent of topology. |
| `topology` | allocate new buffers, upload, publish, retire old | `nnz` and `num_edges` change, so buffer *sizes* change. |

A partial update must be bounded before it is chosen: patching a large fraction
of the values costs more in per-write overhead than one full upload. The
threshold is a **measured** question, and B2 (§12) is the experiment. Nothing
here assumes a value.

Existing prepared operations survive weight, feature and param updates — bind
groups bind *buffers*, not contents, which Phase 2 established and Phase 9's
`write_tensor` test still asserts. They do **not** survive a topology change:
new buffers mean new bind groups, hence new prepared operations.

### 9.2 Materialization is asynchronous

A GPU snapshot is built off the serving path: allocate, upload, submit, and
record the resulting `SubmissionEpoch`. It becomes eligible for use only once
that epoch has completed — publishing it earlier would let a dispatch read a
buffer whose upload has not landed.

The consumer therefore has two publication points: the CPU snapshot (§7,
immediate once built) and the GPU snapshot (only after its upload completes).
`Ready` (§10) reports the second, because that is the one a GPU consumer can act
on.

### 9.3 Lifetime relative to in-flight submissions

**An old GPU snapshot must not be freed when a new one is published.** Dispatches
already submitted against it are still in flight, and WGPU will not diagnose a
buffer freed underneath them.

The rule reuses machinery that already exists rather than inventing a second one:

> A retired snapshot is released only once the completion watermark has reached
> the highest `SubmissionEpoch` in which it was used.

That is precisely Phase 4's arena reclamation rule and Phase 5's monotone
watermark, applied to snapshots instead of buffers. Each snapshot records
`last_use_epoch`, updated whenever a submission encodes against it; the retire
queue is drained on the same non-blocking poll the arena already performs.

The single-queue precondition matters here for the same reason it did in Phase 5:
a scalar watermark is valid because completion on one queue is downward-closed.
A second queue would invalidate both this and the arena, which is a reason to
treat adding one as a design change rather than a configuration change.

---

## 10. Consumer readiness and rejection

`Ready { stream_id, txn_id, version, epoch }` — the consumer has committed
`version`, and for a GPU consumer its upload has completed. Sent once per
committed transaction.

`Nack { stream_id, txn_id, reason, have_version }` with an explicit reason:

| reason | meaning | producer's move |
|---|---|---|
| `SchemaUnsupported` | `schema_version` outside the supported range | Downgrade or stop |
| `SequenceGap` | `seq > last_seq + 1` | Replay or full snapshot |
| `StaleBase` | delta's `base_version` ≠ current | Replay from `have_version`, or snapshot |
| `HashMismatch` | payload digest disagrees | Retransmit `txn_id` |
| `MalformedPayload` | section table or structural validation failed | Defect; do not retry blindly |
| `Unsupported` | unknown *critical* section kind | Downgrade |
| `ResourceExhausted` | payload exceeds negotiated limits, or allocation refused | Renegotiate |

`have_version` is always present, so every rejection doubles as an implicit
resync point and the producer never has to guess what the consumer holds.

---

## 11. Distributed coherence — still deferred

Unchanged from the first draft, and the reasoning is unaffected by the IPC
requirement: partitioning cuts **hyperedges**, so halo cost scales with cut
*cardinality*, not cut count, and one high-cardinality edge spanning every
partition is a global reduction. That term is entirely unmeasured.

The protocol above is single-producer/multi-consumer and does not attempt to be a
partition protocol. It does not preclude one — a partition worker is a consumer —
but no claim is made that distribution helps until B6/B7 (§12) produce numbers.

---

## 12. Testable claims

The point of this document is that its claims can be falsified in a later IPC
phase. Each is stated with the test that would refute it.

| # | Claim | Test that would falsify it |
|---|---|---|
| C1 | Replaying any prefix of a transaction stream twice yields the same state as replaying it once | Property test: random valid stream, apply with random duplication, compare canonical hash |
| C2 | A transaction whose `target_version <= current` is a no-op | Apply an old transaction after a newer one; assert state and version unchanged |
| C3 | Full-snapshot resync and delta-replay converge to identical state | Fork a stream at version V, deliver one path by each route, compare canonical hashes |
| C4 | No sequence gap is ever applied | Inject dropped/reordered messages; assert `Nack{SequenceGap}` and unchanged state |
| C5 | A corrupt payload never mutates live state | Flip bits in payload and header; assert `Nack` and previous version still served |
| C6 | Wrapped/hostile length fields are rejected, not wrapped | Replay the §1.2 values (`nnz = 2^61`, `u64::MAX`) through the section validator |
| C7 | A crash at any point leaves the last committed version intact | Fault injection at each lifecycle point; restart; assert version and state |
| C8 | Coalescing is semantically transparent | Apply a run transaction-by-transaction and coalesced; compare final state |
| C9 | Inference during a topology swap observes exactly one version | Concurrent readers across a publication; assert every step's inputs share a version tuple |
| C10 | An old GPU snapshot is never freed while a submission using it is in flight | Retire under load with validation layers; assert no use-after-free and epoch ordering |
| C11 | A weight-only update never rebuilds either orientation | Instrument rebuild counters; assert zero across a weight-only run |
| C12 | Weight patches reach both orientations identically | Apply patch, compare edge-major and node-major results against a CPU oracle |
| C13 | Header layout is stable | Static assertions on size and every field offset; golden byte-image test |
| C14 | Byte order is explicit | Decode a fixed golden image; must succeed identically regardless of host endianness |

B-series experiments from the first draft remain, with two now unblocked:
**B4** (resident `gather → scale → scatter`, no host wait) and **B5** (the same
with a forced wait) are runnable today and give the baseline every §9 number is
measured against. **B2** (weight-only vs full upload) and **B3** (publication
latency) become measurable as soon as versioned uploads exist. **B1** (rebuild vs
delta), **B6** (halo size vs partition count) and **B7** (overlap ratio) remain
blocked on a delta path and a partitioner respectively.

---

## 13. Decision

**S0 — static single-device execution needs no additional synchronization.**
Retained for the static case (§3), and now explicitly scoped: it is a statement
about execution dependency, not about a changing IR.

**S1 — versioned host/device snapshot coherence.** **Promoted from "next
candidate" to required.** Under a dynamically changing IR it is not an
optimisation but the mechanism that makes staleness detectable at all. §4, §7 and
§9 specify it.

**S2 — dynamic topology synchronization.** **Promoted from deferred to required,
and specified here** (§5, §6, §8). It was deferred on the grounds that nothing
mutates a `HyperGraphView`; the requirement removes that premise. Still not
implemented in Phase 9 — the deliverable is this protocol plus C1–C14.

**S3 — distributed synchronization.** **Unchanged: deferred**, pending B6/B7.
The IPC requirement does not imply distribution; it makes the *source of truth*
external, which the single-producer protocol above already serves.

**Implementation order, when a later phase takes this up.** (1) The envelope,
section table and validator, with C6/C13/C14 — the audit's most serious defect is
an unchecked length, and it is testable before any transport exists. (2) The
version tuple and CPU snapshot publication, with C9/C11. (3) Full snapshots over
one transport, with C1–C5, C7. (4) Deltas and coalescing, with C8/C12. (5) GPU
materialization and retirement, with C10 — last, because it depends on all of the
above and on the arena machinery it reuses.

Nothing in this document is built in Phase 9. The `shared_state.rs` replacement
it implies is a `hymeko_core` edit and requires an `APPROVED-CORE-EDIT` token
that has not been requested.
