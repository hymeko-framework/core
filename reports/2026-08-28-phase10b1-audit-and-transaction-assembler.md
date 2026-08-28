# Phase 10B1 — Wire-codec audit and transaction assembler

**Date:** 2026-08-28
**Base commit:** `9e04c9f57902b9942b7e0a0c3c4f881785c6ccd5` (tree `55c9ae5d48f63185a2e06514ac9f661cc7cbe3e9`)
**Authorisation:** `APPROVED-CORE-EDIT: ir-stream-transaction-assembler`
**Character:** Part A independent audit of Phase 10A, then Part B implementation.

---

## 1. Summary

**Part A verdict: A1 — defects found and corrected without protocol change.**

Five defects in the Phase 10A codec, none of them a protocol contradiction. The
most substantive was that `ValidatedEnvelope` had twelve public fields, so any
downstream crate could construct a value whose type name asserts it had been
validated. That is now impossible, proved by a compile-fail test that fails with
`E0451`.

The audit also found the codec had **no known-answer vector**: every test was a
round trip or used the decoder's own readers, so an encoder and decoder wrong in
the same way would have passed. A frozen 150-byte message now exists, decoded by
hand against the specification field by field, anchored to the **published
BLAKE3 test vector** for provenance.

Part B implements `TransactionAssembler`: a pure synchronous state machine
consuming only `ValidatedMessage`. No transport, clock, thread, `HyperGraphView`
or GPU type appears in it. 19 tests, including an exhaustive sweep over all
12³ = 1728 ordered message-type triples asserting it cannot panic.

Tests rose 39 → 68 in `ir_stream`, 377 → 407 workspace-wide. Clippy clean with
and without `--features ipc`. `Cargo.toml` was **not** modified — the assembler
benchmark extends the existing target.

---

## 2. Pre-edit verification

The brief required proving the module is genuinely compiled before editing —
the Phase 10A lesson, where a CRLF-defeated `perl` edit left `ir_stream`
uncompiled and `cargo check` passed vacuously.

```
ir_stream tests present in the built binary: 39   (Phase 10A recorded 39)
cargo fmt --check                              : pass
clippy default / --features ipc                : 0 / 0
lib tests                                      : 56 passed
```

The module was confirmed present by **test enumeration**, not by a green build.

---

## 3. Part A — audit against the eighteen invariants

| # | Invariant | Verdict |
|---:|---|---|
| 1 | Envelope size and every field offset match the 144-byte spec | **pass**, now also by frozen vector |
| 2 | Encoder and decoder agree without relying only on round trips | **was a gap** → fixed (§3.2) |
| 3 | At least one frozen known-answer byte vector exists | **was absent** → added (§3.2) |
| 4 | Digest coverage exact and unambiguous | **pass**, now pinned by three assertions |
| 5 | The digest field itself handled as specified | **pass** |
| 6 | Limits checked before hashing attacker-controlled lengths | **pass**, ordering test present |
| 7 | Exact input length established before slicing or hashing | **pass** |
| 8 | Every `u64 → usize` conversion checked | **pass** |
| 9 | Every `offset + length` and `count × size` checked | **pass** |
| 10 | Section offsets strictly increasing and non-overlapping | **was "non-decreasing"** → fixed (§3.3) |
| 11 | Unknown skippable and mandatory section rules correct | **pass** |
| 12 | Validated types not constructible outside validation | **VIOLATED** → fixed (§3.1) |
| 13 | No hostile byte slice can panic | **pass** — zero `unwrap`/`expect`/`panic!` in non-test code |
| 14 | Error formatting cannot echo unbounded attacker data | **pass** — every variant carries integers and `&'static str` only |
| 15 | Allocation bounded before any `Vec` reserve | **pass** |
| 16 | Encoder and decoder share layout arithmetic | **pass** — both use the same `OFF_*` constants |
| 17 | Legacy quarantine does not present the old structs as recommended | **pass** |
| 18 | The `Option`-returning legacy change documented accurately | **pass** |

### Targeted searches

* **`unwrap` / `expect` / `panic!` / `unreachable!` in non-test code: zero.**
* **Truncating casts:** all `as` casts reviewed. Every one is widening
  (`usize → u64`, `u16 → usize`) or bounded by a prior limit check
  (`u32 → usize` after `max_section_count`). One exception found — `ElementKind::size() as usize`,
  a `u64 → usize` narrowing — see D5.
* **Unchecked add/mul:** none reachable from wire values. `base = TABLE_PREFIX_BYTES + index * ENTRY_BYTES`
  is `usize` arithmetic but `index < count ≤ max_section_count`, and
  `table_bytes ≤ payload_len` was already proved, so `base + ENTRY_BYTES` is
  in-bounds by construction.
* **Accidental quadratic validation:** `singletons_seen.contains()` inside the
  section loop *looks* O(n²). It is not: only singleton kinds are pushed and a
  duplicate errors immediately, so the vector never exceeds the six singleton
  kinds. `contains` is O(6). No change made; recorded so a future reader does not
  "fix" a non-problem.
* **Hash-after-interpretation ordering:** correct — digest precedes
  `validate_sections`.
* **Public constructors for validated types:** `ValidatedMessage` and
  `ValidatedSection` were already sealed. `ValidatedEnvelope` was not (D1).

### 3.1 D1 — `ValidatedEnvelope` was forgeable *(invariant 12)*

Twelve public fields meant a downstream crate could write
`ValidatedEnvelope { .. }` with arbitrary contents. Nothing in this crate trusted
one in a memory-unsafe way, but a type whose name asserts a property must not be
constructible without establishing it, or the next function to take one on trust
inherits a lie.

**Fix.** Split by role. `EnvelopeDraft` carries only what a producer legitimately
chooses — message type, flags, stream/seq/txn identity, versions. `schema_version`
and `header_bytes` are constants of this build, and `payload_len` and
`payload_hash` are derived from the payload, so **none of them appear in the
draft** and a producer cannot set them wrong. `ValidatedEnvelope` now has private
fields and accessors, obtainable only from `decode`.

Proved, not assumed: the compile-fail doctest was temporarily un-marked and the
compiler produced

```
error[E0451]: fields `schema_version`, `header_bytes`, `msg_type`, `flags`,
`stream_id`, `seq`, `txn_id`, `base_version`, `target_version`, `payload_len`
and `payload_hash` of struct `ValidatedEnvelope` are private
```

— the intended reason, all eleven fields, not an incidental error.

### 3.2 D2 — no known-answer vector *(invariants 2, 3)*

Every Phase 10A test was `encode → decode → compare`, or read fields back with
`rd_u16`/`rd_u32` — the decoder's own helpers. Both families pass if encoder and
decoder are wrong in the *same* way.

**Fix.** A frozen 150-byte message (144-byte header + `b"HyMeKo"`), decoded by
hand against the specification table before being frozen:

| off | bytes | meaning |
|---:|---|---|
| 0 | `53 4b 4d 48` | magic, LE `0x484D4B53` |
| 4 | `01 00` | schema 1 |
| 6 | `90 00` | header_bytes 144 |
| 8 | `06 00` | msg_type 6 = `Commit` |
| 10 | `01 00` | flags = LAST_CHUNK |
| 16 | `ef cd ab 89 67 45 23 01` | stream_id `0x0123456789ABCDEF`, LSB first |
| 104 | `06 00 …` | payload_len 6 |
| 112 | `d73afba8…107b` | BLAKE3(`"HyMeKo"`) |
| 144 | `48 79 4d 65 4b 6f` | ASCII `HyMeKo` |

**Provenance of the digest.** No BLAKE3 command-line tool exists on this host
(`b3sum`, `blake3`, `rhash` absent; `openssl` offers only BLAKE2), so an external
comparison was not possible. Independence is established differently:
`blake3_matches_the_published_empty_vector` asserts the **official BLAKE3 test
vector for the empty input**, `af1349b9…f3262`, which comes from the algorithm's
authors and not from this crate. It passes, so the dependency is genuine BLAKE3
and the frozen digests are trustworthy fixtures.

Six tests now pin this: decode, raw offsets, digest coverage (payload only — and
explicitly *not* the whole message or the header), encoder reproduction, and an
exhaustive single-bit-flip sweep.

### 3.3 D3 — offsets were non-decreasing, not strictly increasing *(invariant 10)*

A zero-length section makes `previous_end == byte_offset`, so a second section at
the *same* offset passes the non-overlap check. The sequence was therefore only
non-decreasing.

**Fix.** Zero-length sections are rejected (`WireError::EmptySection`). A section
carrying no bytes conveys nothing — a producer with nothing to send omits it —
and rejecting it restores the strict rule unconditionally rather than by
argument.

This is a *tightening within* the specification, not a change to it:
`docs/hypergraph-synchronization.md` §5.3 already says offsets must strictly
increase. Nothing that was legal by the spec became illegal.

### 3.4 D4 — a misleading error *(minor)*

Reading an `f32` section as `u32` returned `SectionLengthMismatch { index: 0, … }`
— the wrong rule, a fabricated index, and element *sizes* in fields documented as
byte counts. Replaced with `SectionTypeMismatch { expected, actual }`.

### 3.5 D5 — a truncating cast *(minor, unreachable)*

`ElementKind::size() as usize` narrows a `u64`. Unreachable in practice (the
values are 1 and 4) but it is exactly the cast class this codec forbids. `size()`
is now defined in terms of a new `size_usize()` and widens, rather than the
reverse.

### 3.6 One false alarm, self-inflicted

My first version of the bit-flip test asserted that only bytes 16..112 are
"identity fields whose flip stays valid". It failed at byte 8 — `msg_type` 6
(`Commit`) flips to 7 (`Abort`), a *different but entirely valid* message. The
code was right and the test's classification was wrong.

Rewritten to the property that actually matters: **no flip may produce a message
that both validates and decodes identically to the original.** Silent absorption
is the failure mode; a different-but-valid message is not. That version passes
over all 150 bytes × 8 bits.

### 3.7 One rule discovered while writing Part B

A section-bearing message with a **zero-byte payload** is rejected
(`SectionTableOutOfBounds { need: 8, payload_len: 0 }`). This is correct: the
payload *is* the section table, so "no sections" is an eight-byte table with
`count = 0`, not an absent one. Surfaced when an assembler test sent an empty
`SnapshotBegin`. The code was right; the test was wrong. The rule is now pinned
by `a_section_bearing_payload_must_carry_a_table_even_when_empty` so it cannot be
loosened by accident.

---

## 4. Part B — the transaction assembler

```rust
pub fn apply(&mut self, msg: &ValidatedMessage<'_>) -> ApplyOutcome
```

`ApplyOutcome` is `Accepted | Duplicate | Aborted | Committed(CommittedTransaction) | ResyncRequired(ResyncReason)`.

**Purity.** The module imports four things: `ValidatedMessage`, `MessageType`,
`SectionKind`/`ElementKind`, `SnapshotVersion`. No transport, clock, thread,
`HyperGraphView`, GPU type or async runtime appears in the API or its
dependencies. Zero `unwrap`/`expect`/`panic!`.

**Staging.** Sections are copied into owned buffers on arrival and live state is
untouched until `Commit`. Two independent reasons: a `ValidatedMessage` borrows
the transport's buffer, which will be reused; and an abandoned transaction must
leave no trace, so a consumer never observes a half-applied topology change.

**Idempotence, two mechanisms.** A `seq` at or below the highest seen is dropped
(per-message). Separately, a transaction whose `target_version` is already
dominated by the current version is a no-op *even with a fresh seq* — which
catches a producer replaying after a resync, when sequence numbers have moved on.
Both are tested; neither subsumes the other.

**Divergence detection.** Sequence gap, stream-identity change, stale delta base,
unexpected message for the state, accumulated staging limits, and one semantic
rule: a transaction raising `topology` without also raising `weights` and
`features` is `IncoherentVersionBump`, because those are indexed by structure
(§4 of the design note).

**Consumer-to-producer messages** (`Ready`, `Nack`, `ResyncRequest`, `Credit`)
arriving on a consumer's input are refused rather than ignored — treating them as
benign would hide a mis-wired stream.

**Recovery.** `reset_for_resync()` discards staging and forgets stream position
but **keeps the version**: that is the consumer's durable state and what a
producer needs in order to bring it current. A restart re-establishes identity and
sequence from the first message, because a restart cannot know whether the
producer also restarted.

`CommittedTransaction` is sealed for the same reason as `ValidatedEnvelope`, and
exposes `changes_topology()` — the distinction a consumer acts on, since a
topology change invalidates both orientations while the others permit partial
updates.

### 4.1 Tests

19 assembler tests: happy paths for snapshot and weight-only delta; abort leaves
no trace; the version is never visible before commit; sequence and version
idempotence; sequence gap, stale base, stream change, orphan chunk, orphan
commit, double begin, consumer-to-producer refusal, incoherent version bump;
staging bounded by bytes and by chunk count; resync semantics; and
**snapshot-versus-delta-chain convergence** — two routes from `ZERO` to one
target must leave identical version *and* identical staged content (claim C3 of
the design note, at the version level).

`no_message_sequence_can_panic_the_assembler` drives **all 12³ = 1728 ordered
triples** of message types through a fresh assembler.

---

## 5. Gates

| gate | result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 issues |
| `cargo clippy -p hymeko_core --features ipc --all-targets -- -D warnings` | 0 issues |
| lib tests | **85 passed**, 0 failed (baseline 56) |
| workspace (`--no-fail-fast`) | **407 passed**, 1 pre-existing failure (baseline 377) |
| `hymeko_core --features ipc` | 242 passed, 1 pre-existing failure |
| doctests | 2 passed (example + the `compile_fail` seal) |
| benchmark | 18 cases, 0 errors |

`ir_stream` tests: **39 → 68**. The single failure throughout is the known
`test_load_meta_geometry` 57/58 fixture drift, untouched and unsilenced.

---

## 6. Performance

Assembler, at survey resolution:

| case | time | staged throughput |
|---|---:|---:|
| 1 chunk × 64 KiB | 14.7 µs | 4.18 GiB/s |
| 8 × 64 KiB | 154 µs | 3.19 GiB/s |
| 64 × 64 KiB | 2.57 ms | 1.52 GiB/s |
| 8 × 512 KiB | 1.76 ms | 2.22 GiB/s |

Rejection stays cheap, the same anti-amplification property the codec has:
**sequence gap 161 ns, orphan commit 64.1 ns**, against tens of microseconds for
an accepted transaction.

Throughput falls with chunk count because each chunk pays a full message
validation — dominated by BLAKE3 over that chunk — before the assembler sees it.
That is the validator's cost, not the assembler's, and it is the expected shape.

**Codec numbers are 15–30 % below the Phase 10A run**, and the reason is visible
in the data rather than assumed: the two *non-hashing* rejection paths moved by
−0.34 % and −0.67 %, while every digest-dominated case moved 15–30 %. Unchanged
code plus moved hashing throughput is ambient variation, not regression. Having
both kinds of case in one benchmark is what makes that diagnosable.

---

## 7. Files changed

| file | change |
|---|---|
| `ir_stream/assembler.rs` | **new**, 700 lines — the state machine and 19 tests |
| `ir_stream/envelope.rs` | `EnvelopeDraft` added; `ValidatedEnvelope` sealed with accessors |
| `ir_stream/section.rs` | zero-length rejection, `size_usize`, 4 new tests |
| `ir_stream/error.rs` | `EmptySection`, `SectionTypeMismatch` |
| `ir_stream/codec.rs` | `encode_message` takes a draft; frozen-vector module (6 tests) |
| `ir_stream/mod.rs` | exports, `compile_fail` seal doctest, design-rule note |
| `benches/ir_stream_bench.rs` | 2 assembler benchmark groups |

`Cargo.toml` **unchanged** — the assembler benchmark extends the existing target,
so no new registration was needed. No dependency added. No `unsafe`. No
`hymeko_core` module outside `ir_stream` touched; `hymeko_hnn` and `hymeko_wgpu`
untouched.

---

## 8. Limitations

* The assembler is single-stream by construction. Multi-stream merge with
  disjoint ownership (§5.4 of the design note) is not implemented.
* No producer side: coalescing, the delta log and `ResyncRequest` emission are
  the producer's, and are unbuilt.
* `CommittedTransaction` is not applied to anything. Building a view, publishing
  a snapshot and materialising GPU buffers are later phases, deliberately.
* Panic-freedom is asserted over an exhaustive triple sweep and a hostile byte
  corpus, not proved. A fuzzer over `validate_message` and `apply` is the obvious
  next step and needs no new design.
* Benchmarks ran at survey resolution on one host, and that host was measurably
  busier than during Phase 10A. Adequate for shape and ratios; not a gate.
* Still only little-endian hosts. The byte-order tests assert explicit LE
  encoding, but no big-endian execution was performed.

---

## 9. Claim status

From `docs/hypergraph-synchronization.md` §12:

| claim | before | now |
|---|---|---|
| C1 — replay of any prefix is idempotent | untested | **proved at the transaction level** |
| C2 — a dominated target is a no-op | predicate only | **proved** |
| C3 — snapshot and delta chain converge | untested | **proved for version and staged content** |
| C4 — no sequence gap is ever applied | untested | **proved** |
| C5 — corrupt payload never mutates live state | half | **proved** — staging plus digest |
| C6, C13, C14 | proved | proved, now also by frozen vector |
| C7 — crash leaves last committed version intact | untested | **partly** — `resuming_at` semantics tested; durability is the caller's |
| C8–C12 | unchanged | require producer, view or GPU layers |
