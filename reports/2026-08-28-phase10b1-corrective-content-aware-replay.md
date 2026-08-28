# Phase 10B1 corrective — content-aware replay

**Date:** 2026-08-28
**Base commit:** `c7ce54b90538d2af465d3829b85bc15320cf8213` (tree `637775f0ae6b6b0188d06e2638eabec68f170f7d`)
**Authorisation:** `APPROVED-CORE-EDIT: ir-stream-content-aware-replay`
**Character:** correctness and performance correction to the Phase 10B1
assembler, before snapshot materialization.

---

## 1. Summary

Phase 10B1 answered `Duplicate` for every message whose sequence number had been
seen, and for every transaction whose target version was already held. The audit
finding was **confirmed empirically before any fix**: ten discriminating probes
were run against the unmodified implementation, and every one of them returned
the same answer, including four genuine content conflicts and a transaction that
disagreed with the committed one at the same version.

The corrective replaces sequence-only reasoning with a **canonical identity** at
both levels — per message and per transaction — and reports five distinct
outcomes instead of one. Replay authentication is O(1) with memory constant in
stream age. No payload is retained for replay detection and **no payload is
hashed a second time**.

Large-payload assembler throughput is unchanged (~100 % retained, gate ≥ 90 %).
`hymeko_hnn` and `hymeko_wgpu` are structurally untouched, verified by
`git status`.

---

## 2. Pre-corrective proof

The brief required establishing the behaviour rather than trusting the review.
A probe applied ten cases to the **unmodified** assembler:

```
=== PRE-CORRECTIVE BEHAVIOUR ===
1  same seq, byte-identical               -> Duplicate
2  same seq, different message type       -> Duplicate
3  same seq, different txn_id             -> Duplicate
4  same seq, different version tuple      -> Duplicate
5  same seq, different payload digest     -> Duplicate
6  fresh seq, target==current, same txn    -> Duplicate
7  fresh seq, target==current, DIFFERENT txn -> Duplicate
8  superseded Begin                        -> Duplicate
8  superseded following Chunk              -> ResyncRequired(UnexpectedMessage{..})
8  superseded following Commit             -> ResyncRequired(SequenceGap{expected:2,got:3})
9  replay 200 behind                       -> Duplicate
10 seq == u64::MAX                         -> Accepted
10 continuation after MAX                  -> Duplicate
```

Three defect classes, all confirmed:

* **Conflicts reported as duplicates** (cases 2–5, 7). Four independent ways for
  a message to differ, and a transaction that disagreed with the one already
  committed at that version, were all indistinguishable from an identical
  retransmission.
* **Superseded transactions orphaned** (case 8). The `Begin` was dropped and
  forgotten, so its chunks arrived as `UnexpectedMessage` and its commit then
  produced a *spurious* `SequenceGap` — the rejection of the chunk had not
  advanced the sequence, so a compounding error followed the first.
* **Sequence exhaustion trap** (case 10). `saturating_add(1)` at `u64::MAX`
  yields `MAX`, and every sequence satisfies `seq <= MAX`, so **all subsequent
  traffic forever** was classified `Duplicate`.

Case 9 additionally showed a replay far outside any retained history being
asserted as a duplicate, which the implementation had no evidence for.

---

## 3. Corrected semantics

| outcome | meaning |
|---|---|
| `Accepted` | consumed and staged |
| `ExactDuplicate` | identical message at a retained sequence, **or** a transaction whose canonical identity equals the one committed at this version |
| `ConflictingReplay { seq }` | a retained sequence reused with different content |
| `Superseded` | belongs to a transaction the consumer has moved past; consumed, not staged |
| `ReplayOutsideWindow { seq }` | older than retained history; cannot be authenticated either way |
| `SequenceExhausted` | no successor to `u64::MAX` exists |
| `Aborted`, `Committed`, `ResyncRequired` | as before |

Post-corrective, the same ten probes:

```
1  same seq, byte-identical               -> ExactDuplicate
2  same seq, different message type       -> ConflictingReplay{seq:1}
3  same seq, different txn_id             -> ConflictingReplay{seq:1}
4  same seq, different version tuple      -> ConflictingReplay{seq:1}
5  same seq, different payload            -> ConflictingReplay{seq:1}
6  same txn replayed: Begin/Commit        -> Accepted / ExactDuplicate
7  DIFFERENT txn, same target: Begin/Commit -> Accepted / ResyncRequired(ConflictingCommit)
8  superseded Begin / Chunk / Commit      -> Superseded / Superseded / Superseded
9  replay 200 behind                      -> ReplayOutsideWindow{seq:1}
10 different message at MAX               -> ConflictingReplay{seq:18446744073709551615}
10 unauthenticatable at MAX               -> SequenceExhausted
10 identical message at MAX               -> ExactDuplicate
```

**No conflict is classified as an exact duplicate.**

---

## 4. Message identity

```rust
pub struct MessageIdentity {
    stream_id, seq, txn_id, msg_type,
    base_version, target_version,
    payload_digest: [u8; 32],
}
```

Fixed size, constructed in O(1) from already-validated envelope fields. The
payload digest is **copied** from the envelope where the validator computed and
verified it; the payload itself is never retained and never rehashed. Equality
means every property that could distinguish two messages is equal, which is what
makes "exact duplicate" a claim rather than a guess.

---

## 5. Replay window — O(1), bounded

A fixed-capacity ring indexed by `seq % capacity`, allocated once.

* **Lookup and insertion O(1)** — a modulo, an index and one comparison. No
  scan; production contains no `HashMap`, `HashSet`, `BTreeMap` or `VecDeque`
  (the only textual hit is a doc comment explaining why a map was rejected).
* **Memory** `capacity × size_of::<Option<MessageIdentity>>()`, fixed at
  construction, **constant with stream age** — asserted by
  `replay_memory_is_constant_with_stream_age` over 500 messages.
* **Collisions never falsely match.** Each slot stores the sequence it holds, so
  two sequences `capacity` apart are distinguished;
  `a_wrapped_slot_collision_never_produces_a_false_match` drives exactly that.
* **Capacity zero is explicit**: nothing is retained, so every replay reports
  `ReplayOutsideWindow` and none is claimed exact. A legitimate configuration
  for a consumer that prefers resynchronising to remembering.
* **Only accepted messages are recorded.** A resync-demanding or exhausted
  message never enters the window —
  `a_resync_demanding_message_does_not_poison_replay_history`.
* No payload bytes and no whole messages are retained.

The slot cast is exact by construction (the modulo result is strictly less than
`slots.len()`, itself a `usize`) and is `debug_assert`ed rather than trusted.

---

## 6. Transaction identity

An incremental BLAKE3 over a canonical, domain-separated encoding:

```
"hymeko.ir_stream.transaction.v1\0"
kind(u8) ‖ stream_id(u64 LE) ‖ txn_id(u64 LE) ‖ base(4×u64 LE) ‖ target(4×u64 LE)
per message, in arrival order: index(u64 LE) ‖ msg_type(u16 LE) ‖ payload_digest(32)
"count" ‖ message_count(u64 LE)
```

Every field is fixed width, so no concatenation ambiguity is possible. The index
commits chunk **order**; the trailing count commits chunk **count**, so a
truncated message list cannot masquerade as a shorter transaction.
`transaction_digests_distinguish_content_order_and_count` asserts that
reordering, dropping, adding and emptying all change the digest, and that
identical content does not.

**No second payload pass.** The digest composes the per-message payload digests
the validator already produced, so it still transitively commits to every payload
byte at O(messages) rather than O(bytes). Production contains zero
`blake3::hash` calls.

### 6.1 A design error found by the tests

The first implementation included the transport **sequence number** in the
per-message contribution. That broke the case the digest exists for: a producer
replaying a transaction after a resynchronisation necessarily uses new sequence
numbers, and case 6 — a legitimate exact replay — came out as
`ConflictingCommit`.

Sequence positions a message in a stream; it does not describe content. It was
removed, and identity now means "the same transaction", not "delivered at the
same stream position". Case 6 became `ExactDuplicate` and case 7 remained a
conflict, which is the pair of behaviours that had to hold simultaneously.

### 6.2 Same-version handling

A `Begin` whose target equals the current version enters an explicit
`RevalidatingReplay` state. It is staged under ordinary limits, and the identity
is compared **at commit**:

* equal digest → `ExactDuplicate`, nothing replaced;
* different digest → `ResyncRequired(ConflictingCommit { version, expected, got })`.

Nothing is published or replaced before equality is established, and a
transaction id alone is never treated as proof of content identity.

---

## 7. Superseded drain

A `Begin` whose target is strictly dominated by the current version enters
`DiscardingSuperseded`: subsequent chunks of that transaction are consumed
**without staging any payload**, its commit yields `Superseded`, and an abort
exits cleanly. Stream, transaction id, sequence and message-kind consistency are
still checked; a different transaction during a drain demands resync; and the
drain is bounded by `max_chunks` so it cannot be driven indefinitely for free.

This removes the compounding failure measured in §2 case 8: the chunks are no
longer orphans, and no spurious sequence gap follows.

---

## 8. Sequence exhaustion

`saturating_add` is gone. At `u64::MAX`:

* an identical message is still `ExactDuplicate`;
* a differing message at `MAX` is `ConflictingReplay`;
* anything unauthenticatable is `SequenceExhausted`, because no successor exists
  and the stream needs a new identity.

`the_sequence_boundary_is_handled_explicitly` covers `MAX-1`, `MAX` and three
attempted continuations. The `checked_add` on the non-replay path is retained as
a guard rather than removed as unreachable.

---

## 9. Tests

| suite | before | after |
|---|---:|---:|
| `ir_stream` total | 68 | **87** |
| of which assembler | 19 | **38** |
| `hymeko_core` lib | 85 | **103** |
| workspace (`--all-features`) | 407 | **428** |
| `hymeko_core --features ipc` | 242 | **260** |
| `hymeko_wgpu` | 168 | **168** (unchanged) |
| doctests (`--all-features`) | 2 | **22** |

The single failure throughout is the known `test_load_meta_geometry` 57/58
fixture drift — untouched and unsilenced.

**Model-based tests** (Part H) use a reference model written independently of the
production transition function: it never calls `dispatch`, `begin`, `chunk` or
`commit`, and tracks only the asserted properties. 64 deterministic randomized
streams of 200 mixed messages each, plus a zero-capacity variant, assert that the
version never regresses, one version never maps to two accepted transaction
digests, memory stays within bounds, and no sequence panics. Exhaustive triples
over a six-symbol alphabet cover the short-sequence space.

---

## 10. Performance

Measured like-for-like: construction and teardown are inside the timed region in
**both** the baseline and the corrective, because that is what the Phase 10B1
baseline measured.

| case | Phase 10B1 | corrective (2 runs) | retained |
|---|---:|---|---:|
| 1 chunk × 64 KiB | 17.575 µs | 19.525 / 19.812 µs | 89 % |
| 8 × 64 KiB | 156.47 µs | 164.75 / 149.10 µs | ~100 % |
| 64 × 64 KiB (4 MiB) | 2.7875 ms | 2.8288 / 2.7558 ms | **~100 %** |
| 8 × 512 KiB (4 MiB) | 1.9570 ms | 1.9423 / 1.9965 ms | **~100 %** |

**The large-payload gate (≥ 90 %) passes on both 4 MiB cases at ~100 %.**

The 1×64 KiB case retains 89 %, and the cause is measured rather than guessed:
assembler **construction** now allocates and zeroes the replay window.

| replay capacity | `TransactionAssembler::new` |
|---:|---:|
| 0 | 33.8 ns |
| 16 | 67.3 ns |
| 256 | 320 ns |
| 1024 (default) | **1.143 µs** |
| 65536 | 1.243 ms |

1.143 µs against a 17.6 µs baseline is 6.5 %, which accounts for the shortfall on
the smallest case. This is a **per-stream** cost paid once before any message,
not a per-message cost. A deployment sensitive to it can lower the capacity; the
curve above is why that knob is measured rather than asserted.

The rejection micro-benchmarks moved from 174 ns / 76.8 ns to 1.50 µs / 1.30 µs
for the same reason — they construct an assembler per iteration and do almost
nothing else, so they measure construction.

**Replay paths, all O(1):**

| path | time |
|---|---:|
| exact duplicate | 75.7 ns |
| conflicting duplicate | 68.9 ns |
| replay-window miss | 74.2 ns |
| superseded drain (256 KiB transaction) | 71.0 µs |

A conflicting replay is **not** more expensive than an exact one, so detecting
divergence is not a denial-of-service lever.

**Capacity scaling confirms O(1) lookup** — a 4096× capacity increase produces no
trend:

| capacity | 16 | 256 | 4096 | 65536 |
|---|---:|---:|---:|---:|
| exact-duplicate lookup | 80.8 ns | 79.4 ns | 75.3 ns | 74.8 ns |

### 10.1 A measurement error found and corrected

The first "after" run hoisted construction out with `iter_batched` and reported
the 4 MiB cases as ~1.9× **faster** than baseline. That was not a win:
`iter_batched` excludes construction *and teardown*, and teardown frees up to
4 MiB of staged section buffers, which the baseline timed. The benchmark was
reverted to the baseline's exact shape, and construction is reported separately
instead. An unexplained 2× speedup from a change that adds work is a measurement
artefact, not a result.

---

## 11. Complexity and memory bounds

| property | bound |
|---|---|
| replay lookup | O(1) — modulo, index, compare |
| replay insertion | O(1) |
| replay memory | `capacity × size_of::<Option<MessageIdentity>>()`, fixed at construction |
| memory growth with stream age | **none** |
| transaction digest | O(messages), not O(payload bytes) |
| payload hashing passes | **1**, performed by the validator |
| payload copies | **1**, the documented staging copy |
| staged bytes | `≤ max_staged_bytes`, checked before each allocation |
| production `unwrap`/`expect`/`panic!` | **0** |

---

## 12. Files changed

| file | change |
|---|---|
| `ir_stream/assembler.rs` | identity, replay window, transaction digest, drain state, exhaustion; taxonomy probe; 19 new tests; model module |
| `ir_stream/mod.rs` | exports `AssemblerState`, `MessageIdentity` |
| `benches/ir_stream_bench.rs` | replay-path, capacity-scaling and construction groups |

`Cargo.toml` unchanged. No dependency added. No `unsafe`. No IR interpretation,
no `HyperGraphView`, no snapshot publication, no transport, no async runtime, no
event-store framework.

---

## 13. Mistakes found

1. **Sequence numbers in the transaction digest** (§6.1) — broke legitimate
   replay after resync. Found by case 6, not by review.
2. **`SequenceExhausted` was initially unreachable.** Every `seq <= u64::MAX`
   takes the replay path, so the `checked_add` branch could never fire. The
   exhaustion condition had to move into the replay verdict.
3. **A benchmark that measured the wrong thing** (§10.1).
4. Two `perl` delimiter collisions (`~2x` and `#[ignore`) corrupted source
   mid-edit; both were caught by the next `cargo build` and repaired. The same
   class of tooling error as the Phase 10A CRLF incident, and the same lesson:
   verify the edit landed, do not assume it did.

---

## 14. Limitations

* Replay authentication is bounded by capacity: a retransmission older than the
  window is reported honestly as unauthenticatable, not resolved. That is a
  deliberate trade for constant memory.
* `resuming_at` cannot know the digest of the version it resumes at, so a
  transaction re-establishing exactly that version reports
  `ReplayOutsideWindow` rather than `ExactDuplicate`. Persisting the commit
  digest alongside the version would fix it and belongs to the caller's durable
  state.
* Still single-stream. Multi-stream merge is unimplemented.
* The reference model checks invariants; it does not predict outcomes, so it
  cannot catch a systematically wrong-but-consistent classification.
* Benchmarks ran at survey resolution on one host.
* No producer side: coalescing, delta log and resync emission remain unbuilt.

---

## 15. What Phase 10B2 may now rely on

* A committed transaction carries a **stable content digest**, so a materializer
  can key caches and derived artifacts on it.
* A version maps to **at most one** accepted transaction identity; a second
  claimant is refused, not silently accepted.
* An exact replay is distinguishable from a conflict, so a materializer can skip
  work safely without risking skipping a genuine change.
* Superseded transactions are consumed cleanly, so a materializer never sees
  their content and never sees orphan messages after them.
* Memory is constant in stream age, so a long-lived consumer needs no trimming
  policy.
