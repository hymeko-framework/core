# WGPU Phase 5 — tracked session and fan-out

**Date:** 2026-08-25
**Scope:** `APPROVED-WGPU-PHASE-5: tracked-session-fanout`
**Crates touched:** `hymeko_wgpu` only. CPU invariance verified by **hash diff**.

---

## 1. Files

New:

| file | lines |
|---|---:|
| `hymeko_wgpu/src/scope.rs` | 343 |
| `hymeko_wgpu/src/fanout_tests.rs` (was `tests/diamond.rs`) | 667 |
| `hymeko_wgpu/tests/session.rs` | 398 |
| `hymeko_wgpu/tests/shaders/merge_add.wgsl` | 29 (test-only) |

Modified: `src/arena.rs` (`TrackedSession` replaces `ArenaEncoder`; session-scoped
acquire; `ArenaTensor` loses `Deref`), `src/completion.rs` (the ordering argument,
§4), `src/lib.rs`, `src/operations/spmm.rs` and `src/operations/activation.rs`
(scope-generic builders, split `encode`), `benches/arena_bench.rs`,
`examples/arena_memory.rs`, `tests/arena.rs`.

**New dependencies: none.** Root `Cargo.lock` and `Cargo.toml` hashes are
byte-identical to the checkpoint (`ca754daa0cc11107`, `82ad3f1242f2cefb`).

---

## 2. Tracked session API

```rust
let arena = backend.tensor_arena(TensorArenaConfig::default());

let left  = arena.acquire(shape)?;
let mut session = arena.session(Some("inference"));

spmm.encode(&mut session)?;          // pooled operands -> takes the session
plain_op.encode(session.encoder())?; // caller-owned operands -> raw encoder
let mid = session.acquire(shape)?;   // may reuse a slab freed in THIS session

let (index, epoch) = session.submit();   // no wait, no readback
arena.poll_completion()?;                // non-blocking recycle
```

* `submit(self)` — consumes the session, so a double submit is a compile error.
* `submit_and_wait()` — the synchronous convenience.
* `operations_encoded()` / `is_empty()` — counters.
* Dropping without submitting frees pending releases immediately: unsubmitted
  commands can never run.

No hidden readback, no per-operation submit, no dynamic dispatch.

---

## 3. How the bypass was closed

Phase 4's gap: a pooled buffer could be encoded into a raw `backend.encoder()`,
released, and recycled while the unsubmitted command buffer still referenced it.

### 3.1 Why the cheaper options were rejected

| option | verdict |
|---|---|
| documentation + a test (Phase 4) | the weakest guarantee available — it holds until someone writes the natural thing |
| runtime check inside `encode` | **impossible.** By the time an operation holds `&GpuTensor<f32>`, provenance is gone; a pooled and a plain tensor are indistinguishable |
| keep the raw API, forbid arena resources on it | requires knowing provenance at the call site — the same information, so the same problem |
| **type-level provenance (option D)** | **chosen** |

### 3.2 The construction

Three pieces, all zero-sized:

1. `TensorOperand` gives each tensor type an `EncodeScope` — `GpuTensor` is
   `Untracked`, `ArenaTensor` is `Tracked`.
2. `JoinScope` combines operand scopes, `Tracked` absorbing: an operation
   touching *any* pooled buffer is `Tracked`. Mixing pooled and plain operands
   stays legal.
3. `encode` is implemented **twice** — `Untracked` takes
   `&mut wgpu::CommandEncoder`, `Tracked` takes `&mut TrackedSession`. There is
   no third impl.

The mistake is now a type error.

### 3.3 The cost, stated plainly

`ArenaTensor` **no longer implements `Deref<Target = GpuTensor>`**. It cannot:
`&*lease` yields a plain `&GpuTensor`, whose scope is `Untracked`, laundering the
provenance the whole scheme rests on.

Callers write `lease.tensor()` where a raw tensor is genuinely wanted — explicit
and greppable. This rippled through every arena call site in tests, benches and
the example. It also means a helper taking `&GpuTensor` and one taking
`&ArenaTensor` are different functions; the benchmark and the memory example each
grew one such twin, which is the visible price of the guarantee.

> **Superseded by PHASE-5-CORRECTIVE.** `lease.tensor()` was itself the hole: it
> returned `&GpuTensor<f32>`, scope `Untracked`, so it laundered provenance
> through an explicit call instead of an implicit deref. It is now `pub(crate)`,
> `TensorOperand` is sealed, and readback/host writes go through
> `read_tensor`/`write_tensor`, which take any operand and return data rather
> than a reference. See
> `reports/2026-08-25-wgpu-phase5-corrective-provenance.md`.

Runtime cost: none. The markers are `PhantomData`, resolved at
monomorphisation — measured in §9.

---

## 4. Completion watermark invariant — corrected

Phase 4's comment claimed `fetch_max` made the watermark "conservative under any
out-of-order completion". **That was wrong**, and the correction matters because
the arena rests on it.

* **A single `wgpu::Queue` completes submissions in order.** Therefore completion
  is **downward closed**: if *N* completed, so did all *< N*.
* **That is why a scalar watermark works.** A downward-closed set over a total
  order is fully described by its maximum. This is a *precondition*, not a
  consequence of `fetch_max`. Without in-order completion a watermark is not
  conservative — it is **wrong**: watermark 9 asserts 5 is complete, which can be
  false on an independent timeline while the arena hands out 5's buffer.
* **What `fetch_max` actually buys:** independence from the order the *host*
  observes callbacks in. Nothing promises a callback registered earlier is
  delivered earlier; `store` would let a late-delivered earlier callback clobber a
  higher watermark. That is a host-side concern, not a GPU-ordering one.
* **Multiple queues are unsupported by construction.** `WgpuBackend` owns one
  queue. Two queues would be two timelines, completion would not be downward
  closed, and `CompletionTracker` would be **unsound**, not merely imprecise. A
  multi-queue design needs one watermark per queue and per-slab queue tagging.

Tests: `late_delivery_of_an_earlier_callback_does_not_lower_the_watermark`
(renamed from a version that implied GPU reordering) and
`watermark_implies_all_lower_epochs_complete`. Both are deterministic — no timing
dependence.

---

## 5. Drop and error handling

`tests/session.rs`, 9 tests:

| case | behaviour |
|---|---|
| session dropped without submit | pending releases freed **immediately** — unsubmitted commands can never run |
| empty session | submits cleanly, still takes an epoch |
| state after submit | arena no longer treats a session as open |
| in-flight submission, never waited | reclaimed by a later `reclaim_all` |
| several sessions in sequence | accounting stays exact; one buffer reused |
| two sessions open at once | same-session reuse **disabled** (submit order undetermined) |
| arena dropped with work in flight | no panic; device still usable |
| mixed pooled + plain operands in one session | both encode, one submit |
| intra-session reuse | ordered correctly |

### 5.1 Cases that are structurally unreachable

Recorded rather than faked with a test that cannot fail:

* **encode failing mid-session** — `encode` is infallible; all validation is in
  `prepare`. The `Result` is kept so a future operation needing per-encode checks
  is not a breaking change.
* **submit failing** — `wgpu::Queue::submit` returns no `Result`; device loss is
  asynchronous. Simulating it would test a mock.
* **submitting twice** — `submit(self)` moves the session; covered by a
  `compile_fail` doctest.
* **backend dropped before the arena** — `TensorArena<'b>` borrows the backend;
  rejected by the borrow checker.

### 5.2 Two of my own tests were wrong

* Two session tests asserted reuse counts after `poll_completion`, which is
  **non-blocking** — whether a just-submitted epoch has completed is a race, and
  a count assertion on a race is a flaky test. Both now use blocking
  `reclaim_all`.
* The diamond fixture used `SparseSpec::dense_block`, which is all-positive,
  making ReLU and LeakyReLU produce identical output. The branch test would have
  been vacuous, and its failure message blamed aliasing for a fixture defect.
  Replaced with a signed block.

---

## 6. Compile-fail / type-level guarantees

10 doctests: **5 `compile_fail`, each paired with a compiling twin**. The pairing
is the point — a lone `compile_fail` passes even when the snippet fails for an
unrelated reason, which makes it a test of nothing.

| guarantee | rejected as |
|---|---|
| pooled operation into a raw encoder | `E0599` no such method |
| prepared operation outliving its operands | `E0505` borrow conflict |
| double release | `E0382` use of moved value |
| aliasing a lease (`ArenaTensor: !Clone`) | `E0277` |
| double submit | `E0382` use of moved value |

No new dependency was added for these; `compile_fail` is a rustdoc feature.

---

## 7. Diamond / fan-out topology

```
input → SpMM → shared → { ReLU → left , LeakyReLU → right } → merge → output
```

Four logical tensors. `merge_add.wgsl` is **test-only**, deliberately not a
library operation — the brief forbids widening this phase into a kernel
framework, and a merge kernel with no measured consumer would be exactly that.

`tests/diamond.rs`, 5 tests, all passing:

* the arena reaches the liveness minimum and matches the CPU reference;
* the naive assignment needs four buffers;
* `shared` survives **both** branches (Identity on each branch must equal
  `shared` bit-for-bit — false if it were recycled between the reads);
* branch outputs never alias (ReLU and LeakyReLU must *differ*, which is why the
  signed fixture matters);
* after the merge, all three dead buffers are reusable by the next stage.

---

## 8. Computed vs actual peak buffers

| after | live set | count |
|---|---|---:|
| SpMM | `shared` | 1 |
| ReLU | `shared`, `left` | 2 |
| LeakyReLU | `shared`, `left`, `right` | **3** |
| merge | `left`, `right`, `output` | 3 |

**Computed minimum: 3. Achieved: 3.** Asserted, not assumed
(`buffers_created == 3`, `reuse_hits == 1`).

### 8.1 It was 4 first, and that was a real defect

The first run needed **4**. Cause: *any* release during an open session went to
`pending` and stayed unreusable until submit.

That is provably too conservative **within one session**:

1. a released lease can no longer be named by any new operation — operations
   borrow it, so the borrow checker forbids it;
2. therefore every dispatch touching that buffer was encoded **before** the
   release;
3. commands in one command buffer execute in order, with barriers between compute
   passes;
4. so the new tenant's dispatches strictly follow the old tenant's.

Across *different* sessions it is genuinely unsafe: two sessions can be open at
once and submitted in either order, so a slab released under one could be written
by the other before the first's reads execute.

Fix: `TrackedSession::acquire`, eligible only for slabs released under the **same
session id**, and disabled entirely when two sessions are open (owner ambiguous).
`test_two_open_sessions_disable_same_session_reuse` pins that.

**The fan-out test found this.** A linear chain never exposes it, because nothing
is released mid-session there.

### 8.2 Comparison

| variant | buffers | note |
|---|---:|---|
| A one per logical tensor | 4 | `test_naive_diamond_needs_four_buffers` |
| B manual lifetime | 3 | what a careful human computes |
| C arena + tracked session | **3** | reaches the minimum |

---

## 9. Performance overhead

### 9.1 Acceptance criteria

| criterion | target | measured | verdict |
|---|---|---|---|
| encode overhead per operation | ≈ Phase 3's 0.62 µs | **0.36 µs** tracked, 0.41 µs untracked | **met** |
| resident-chain regression from tracking | < 3 % | no regression (§9.3) | **met** |
| cache-hit acquire/release vs 80 µs submit floor | negligible | 226 ns = **0.28 %** | **met** |
| diamond allocations vs liveness minimum | ≤ minimum | 3 = 3 | **met** |

### 9.2 Component costs

| cell | median |
|---|---:|
| `acquire/hit` | 226.2 ns |
| `acquire/miss` | 775.3 ns |
| `acquire/poll` | 109.8 ns |
| `session/create_drop` | 325.4 ns |
| `session/create_submit` (empty) | 17.37 µs |
| `session/session_acquire` | 602.8 ns |
| `encode_only/untracked` (16 ops) | 6.55 µs → **0.41 µs/op** |
| `encode_only/tracked` (16 ops) | 5.84 µs → **0.36 µs/op** |

`encode_only` hoists `prepare` out of the measured loop, which is how Phase 3's
0.62 µs/op was defined. The earlier `encode/*` group measures prepare+encode
together and is **not** comparable to that figure — a distinction worth keeping,
since the two differ by ~40×.

Tracking adds one `usize` increment per encoded operation. It does not show up:
the tracked figure is *lower* than untracked, which is noise around zero, not a
speedup.

### 9.3 Four cells exceeded 3 % — investigated, not dismissed

The brief requires investigating any delta over 3 % rather than calling it
ambient noise. Four cells did, so each was re-run three times:

| cell (arena vs baseline) | first run | r1 | r2 | r3 |
|---|---:|---:|---:|---:|
| `fanout/medium/fd32` | +12.0 % | **−12.0 %** | +2.8 % | −0.8 % |
| `chain/large/fd8` | +3.2 % | **−18.0 %** | −3.3 % | +0.4 % |
| `encode/medium/fd128` | +18.4 % | −2.9 % | −5.8 % | +12.9 % |

**Identical code moved in opposite directions on every cell.** A real regression
moves consistently; this does not. The means are ≈ +0.5 %, −5 % and +5.7 %
respectively.

For `encode/medium/fd128` the baseline's *own* spread across runs is 248.65 →
332.17 µs — **33 %**. That cell cannot resolve a 3 % question in either
direction, and saying so is more honest than picking whichever run supports the
conclusion. The structural bound applies instead: the arena adds two acquires
(2 × 226 ns) plus one session create (325 ns) ≈ 0.78 µs to a ~270 µs encode =
**0.3 %**.

### 9.4 Linear chain, arena vs hand-written ping-pong

| case | fd | ping-pong | arena | delta |
|---|---:|---:|---:|---:|
| small | 8 | 582.4 µs | 606.5 µs | +4.1 % |
| small | 32 | 543.0 µs | 523.4 µs | −3.6 % |
| medium | 32 | 1.259 ms | 1.252 ms | −0.6 % |
| medium | 128 | 3.961 ms | 3.970 ms | +0.2 % |
| large | 32 | 8.629 ms | 8.685 ms | +0.6 % |
| large | 128 | 47.85 ms | 47.86 ms | +0.0 % |

The two cells outside ±1 % are the smallest ones and have opposite signs.

### 9.5 Fan-out

| case | fd | manual | arena | delta |
|---|---:|---:|---:|---:|
| medium | 32 | 266.9 µs | 298.9 µs | +12.0 % → see §9.3 |
| medium | 128 | 744.7 µs | 741.6 µs | −0.4 % |
| large | 32 | 1.342 ms | 1.350 ms | +0.6 % |
| large | 128 | 6.400 ms | 6.419 ms | +0.3 % |

The fan-out benchmark measures SpMM plus two branches; the merge stage is
excluded because its kernel is test-only, and duplicating it into the crate to
add one dispatch would widen the kernel surface for a number that measures the
merge rather than the arena.

### 9.6 Conditions

Ambient CPU load 12 % before the run. Every decisive comparison is within-run and
in-process; the four over-threshold cells were re-run three times each; no
conclusion rests on a single measurement.

---

## 10. ComputeGraph re-evaluation

Full argument appended to `docs/compute-graph-decision.md`.

| condition | Phase 4 | Phase 5 |
|---|---|---|
| branching makes manual lifetimes impractical | **untested** | **refuted** — the diamond reaches its minimum with one lexical scope of manual information |
| a backend without `CommandEncoder` semantics | not met | not met |
| a measured win from reordering | not met | not met — the branches are independent but already in one command buffer, so there is no freedom left |

### 10.1 How much manual lifetime information the caller supplies

Exactly one thing: the scope in which `shared` lives. One `{ }`.

### 10.2 Is the branch release point error-prone? Asymmetrically.

| mistake | consequence | detected by |
|---|---|---|
| release **too early** | would corrupt results | **compile error** — prepared operations borrow their operands |
| release **too late** | 4 allocations instead of 3 | nothing — silent, memory only |

The dangerous direction is impossible. The remaining one is a performance
mistake bounded by "no worse than not using the arena". That asymmetry is the
strongest argument against a graph so far.

> **Correction (PHASE-5-CORRECTIVE).** As originally written, this claim was
> **not supported**. Removing `Deref` closed the *implicit* laundering path, but
> `ArenaTensor::tensor() -> &GpuTensor<f32>` remained public, and a plain
> `&GpuTensor` has scope `Untracked` — so `.output(pooled.tensor())` produced an
> untracked operation on a pooled buffer that a raw encoder accepted. An
> independent API audit found this; a probe test confirmed the path **compiled**
> before the fix.
>
> The accurate statement for the pre-corrective code is: *the implicit `Deref`
> bypass was removed, but an explicit public raw-tensor escape hatch remained.*
> The claim above holds only from the corrective onward, where it is backed by
> `scope::NoRawTensorAccessor` and `scope::TensorOperandIsSealed`. See
> `reports/2026-08-25-wgpu-phase5-corrective-provenance.md`.

### 10.3 Verdict

**Still no ComputeGraph** — and the case is weaker than in Phase 4, not stronger,
because the one untested condition has now been tested and did not hold.

The minimal residual problem a graph would solve, stated precisely: **computing
release points automatically**, so a value held too long cannot silently cost an
allocation. That is a memory-optimality benefit with a bounded failure mode, on
topologies whose manual cost is currently one lexical scope.

New falsification condition added: evidence that callers *routinely* release too
late in practice — which requires a profile of a real HGNN forward pass showing
allocations above the computed minimum, not an argument.

---

## 11. CPU / core / HNN invariance — by hash

```
$ diff cpu_manifest.sha256 cpu_final.sha256
IDENTICAL: 140/140 hymeko_core + hymeko_hnn files
```

Root manifests, checkpoint vs now:

| file | checkpoint | now |
|---|---|---|
| `Cargo.lock` | `ca754daa0cc11107` | `ca754daa0cc11107` |
| `Cargo.toml` | `82ad3f1242f2cefb` | `82ad3f1242f2cefb` |

Dependency tree unchanged. `vendor/` untouched. No timestamp was used.

### 11.1 Gates

| gate | result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo check --workspace --offline` | clean |
| `cargo build -p hymeko_wgpu --release --offline` | clean |
| `cargo test --workspace --release` | **279 passed, 1 failed** (known drift) |
| `hymeko_wgpu` alone | 110 passed, 0 failed |
| doctests | 10 passed (5 compile-fail + 5 twins) |

The single failure is the pre-existing 57/58 `test_load_meta_geometry` fixture
drift. Untouched, not silenced.

Phase 2 SpMM (13), Phase 3 activation/chain (19) and Phase 4 arena (20)
regression suites all pass unchanged in count.

---

## 12. Recommendation for the next algorithmic kernel

Ranked by what Phase 3–5 measurements support.

1. **A fused SpMM+activation used throughout a chain.** Still the largest
   measured lever: Phase 3 measured fusion recovering 5–21 % by removing one
   dispatch of a pair, and an 8-layer chain has 8 such pairs. Memory is now
   solved and lifetime safety is now type-checked, so dispatch count is what
   remains.
2. **A real HGNN forward pass, as a measurement not a feature.** Every number in
   Phases 3–5 comes from synthetic chains and one hand-built diamond. The open
   ComputeGraph question is now explicitly empirical — do callers release too
   late on real topologies? — and only a real model answers it.
3. **Do not build a merge/elementwise-binary kernel yet.** The one written for
   this phase is test-only and there is no measured consumer. Promote it when a
   real topology needs it, not because the test has one.
4. **Do not pursue suballocation or an allocation fast path.** Unchanged from
   Phase 4: 2–3 allocations per topology, 226 ns hit.
5. **The SpMM workgroup rule remains tuned on one adapter** — still the largest
   single-cell uncertainty (16.5 % swing between shapes at large/fd128).

### 12.1 Open issues

1. Same-session reuse is disabled when two sessions are open. Correct but
   conservative; a per-session pending partition could keep it enabled.
2. Fan-out measured at one topology (diamond) and one size class.
3. One adapter only (RTX 5080 Laptop / Vulkan). DX12 compiled, unmeasured.
4. `encode/*` benchmark cells have ~33 % run-to-run spread on this host and
   cannot resolve small differences; `encode_only/*` is the cell to trust.
5. Amaterasu still blocked; no access attempts this phase.
