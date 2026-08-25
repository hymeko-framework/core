<!--
Phase 3 deliverable: decision document only. No ComputeGraph is implemented,
and nothing in this file is wired into the build.
-->

# ComputeGraph — decision document

**Status:** proposal. Nothing here is implemented.
**Written:** 2026-08-24, at the end of WGPU Phase 3.
**Question it answers:** now that two prepared operations chain in one encoder
and one submit, is a general compute-graph layer justified — and if so, what
exactly should it be?

---

## 1. What Phase 3 actually established

Phase 3 proved the thing a graph would exist to automate, *without* a graph:

```rust
let mut enc = backend.encoder("chain");
spmm.encode(&mut enc);          // A × X -> Y   (Y stays on the device)
act.encode(&mut enc);           // Y -> f(Y)    (in-place, one binding)
backend.submit(enc);            // one submit
let out = backend.read_tensor(&y)?;   // one readback, explicitly asked for
```

Two facts follow from this, and they pull in opposite directions:

1. **Composition already works.** `encode(&mut enc)` is the composition
   operator. It needs no framework — a `CommandEncoder` is the sequencing
   structure, and WebGPU already guarantees that dispatches within one encoder
   observe each other's writes.
2. **Composition is manual, and the manual parts are exactly the parts that are
   easy to get wrong** — buffer reuse, in-place legality, and the order in which
   `prepare()` is called relative to buffer allocation.

A graph is worth building only if it removes class (2) without weakening (1).

---

## 2. What a graph would have to earn its place against

The honest baseline is not "no abstraction". It is the current builder chain,
which is already:

* **statically checked at prepare time** — shape mismatches, aliasing, dispatch
  limits, and `usize → u32` narrowing are all rejected before any GPU work is
  encoded;
* **explicit about cost** — `prepare()` is where pipelines are compiled and bind
  groups built; `encode()` is nearly free; `submit()` is the only synchronisation
  point. A reader can see where the time goes;
* **zero-dispatch on the CPU hot path** — the CPU layers do not know this crate
  exists.

Any graph that makes these worse is a regression regardless of how much
boilerplate it removes.

---

## 3. The three real problems a graph would solve

These are the ones observed while writing Phase 3, not speculative ones.

### 3.1 Intermediate buffer lifetime and reuse

Today the caller allocates every intermediate with
`resources().zeroed_tensor(shape)` and keeps it alive. For a two-op chain that
is one line. For a realistic HGNN forward pass —

```
    A × X → H₁ → f(H₁) → A × H₁ → H₂ → f(H₂) → ... → out
```

— it is one allocation per layer, all live simultaneously, all the same shape
whenever the feature dimension is constant. A graph knows the liveness
intervals and can alias `H₁` and `H₃` onto one buffer. **This is the single
strongest argument for a graph**, and it is a memory argument, not a speed one:
an 8-layer chain at 200k × 128 f32 is 8 × 102 MB = 819 MB of intermediates that
double-buffering would reduce to 205 MB.

### 3.2 In-place legality is a whole-chain property, not a local one

`ActivationBuilder::in_place()` is legal only when nothing downstream still
needs the pre-activation values. That is a *liveness* question. Locally, the
builder cannot answer it; it can only trust the caller. Today the caller is a
benchmark or a test and the answer is obvious. In a composed model it will not
be, and the failure mode is silent wrong numbers rather than a validation error
— WebGPU is perfectly happy to let you overwrite a buffer someone else was
going to read *in a later submit*.

A graph that tracks reads and writes turns this into a decision it can make
correctly and automatically, and into an error when the caller forces it.

### 3.3 Choosing dispatch parameters needs global information

The feature-dimension-aware workgroup rule (§ this phase, `spmm_workgroup_for`)
takes `feature_dim` — which the operation knows. But the *better* version of
that rule would also take "is this the first op in the chain, or is the input
already hot in L2" — which only a graph knows. This is real, but it is the
weakest of the three: the measured spread across workgroup shapes is small
compared to the SpMM/activation split itself.

---

## 4. What a graph must **not** become

Ranked by how likely each is to happen if this is built carelessly.

| anti-pattern | why it is fatal here |
|---|---|
| **Runtime-dispatched node trait** (`Box<dyn Node>`, `enum Op` matched per dispatch) | The CPU path must stay dispatch-free. A graph is allowed to exist *beside* the CPU backend, never *under* it. |
| **A graph that owns the device** | `WgpuBackend` owns device/queue/cache. A graph that also owns them creates two authorities over pipeline caching and the second framework the architecture brief forbids. |
| **Deferred execution by default** | The current model — `encode` when I say, `submit` when I say — is what makes the benchmark cells separable at all. A graph that decides *when* to submit destroys the measurement methodology that produced every number in `wgpu-backend.md`. |
| **A node per activation kind** | `ActivationKind` is already a value, not a type. Re-expressing it as a node hierarchy is abstraction without a use case. |
| **Automatic kernel fusion** | Fusion is a code-generation problem. Phase 3 deliberately excluded runtime shader-string generation; a fusing graph reintroduces it through the back door. Fused kernels should stay hand-written and explicitly selected. |

---

## 5. Recommendation

**Do not build a general ComputeGraph in Phase 4.** Build the narrow thing that
solves §3.1 and §3.2, and only that:

> A **buffer arena with liveness tracking**, not a graph.

Sketch of the shape it should take — deliberately not a trait hierarchy:

```rust
// Owns intermediates; hands out borrows that record their own liveness.
// Does NOT own the device, does NOT execute anything, does NOT know what an
// "operation" is.
pub struct TensorArena<'b> { /* backend borrow, free-lists keyed by byte size */ }

impl<'b> TensorArena<'b> {
    pub fn acquire(&mut self, shape: TensorShape) -> ArenaTensor<'_>;
    // Dropping an ArenaTensor returns the buffer to the free list.
}
```

Why this and not a graph:

* it solves the memory problem (§3.1) directly, and the measurement above says
  that problem is worth ~600 MB on an 8-layer chain;
* liveness becomes Rust's borrow checker plus `Drop`, which is machinery that
  already exists and is already correct, rather than a scheduler I would have to
  write and verify;
* it composes with the existing `encode`/`submit` model instead of replacing it
  — `PreparedSpmm` and `PreparedActivation` need no change;
* it adds no dynamic dispatch anywhere;
* if it turns out to be insufficient, it is ~200 lines to delete. A graph is not.

§3.3 is explicitly deferred: the workgroup rule should stay a pure function of
information the operation already has until a measurement says otherwise.

## 6. What would change this recommendation

Stated in advance so the decision is falsifiable rather than a preference:

1. **A chain long enough that manual encoding is the actual bottleneck.** Threshold: a
   forward pass where the encode-site boilerplate exceeds the operation
   configuration in line count — realistically 6+ heterogeneous ops with
   branching topology, not a linear stack.
2. **A second backend that cannot use `CommandEncoder` semantics.** If CUDA
   arrives and its natural sequencing unit is not an encoder, then a
   backend-neutral sequencing description becomes necessary, and that
   description is a graph. Until then it would have exactly one implementation.
3. **A measured win from reordering.** If a benchmark shows that executing ops
   in an order other than the one written is materially faster, something must
   own the reordering. No such measurement exists today.

None of the three hold as of the end of Phase 3.

---

# Phase 4 re-evaluation (2026-08-25)

Phase 4 built the `TensorArena` this document recommended instead of a graph.
The three falsifiable conditions from §6 are now re-tested against evidence
rather than expectation.

## Condition 1 — "a chain long enough that manual encoding is the bottleneck"

**Not met, and partly untested.**

Met on the part that was measured: an 8-layer chain runs through the arena with
two `acquire` calls and one loop. The lifetime reasoning that a hand-written
ping-pong requires — *which buffer is free at layer i* — moved into the pool, and
the memory result is identical to hand-written ping-pong (195.3 MiB, 2 buffers).
That is the outcome this document predicted.

Encoding is not the bottleneck: `encode/*` measures ~345 µs for a whole 8-layer
chain at medium/fd8, against 831 µs for the chain's execution, and the arena adds
~1 % to it.

**Untested:** the condition named *branching topology*, and the chain measured is
strictly linear. A fan-out graph where one intermediate feeds two later layers is
where explicit leases would get harder — the lease must outlive both consumers,
and nothing in the current API helps a caller work out how long that is. No such
workload exists in this repository yet, so the condition is neither met nor
refuted. It should not be recorded as "passed".

## Condition 2 — "a second backend that cannot use `CommandEncoder` semantics"

**Not met.** No CUDA or FPGA backend arrived. Unchanged.

Phase 4 did add something a future backend must honour: `SubmissionEpoch` and the
completion watermark. Those are expressed in terms of "a submission completes",
not in terms of wgpu, so they should port. The `ArenaEncoder` guard is
encoder-shaped and would need rethinking for a backend without encoders — but
that is the same observation as the condition itself.

## Condition 3 — "a measured win from reordering"

**Not met.** Nothing was reordered, and no measurement suggests reordering would
help. The chain is a dependency chain: layer *i+1* reads layer *i*'s output.
There is no freedom to exploit.

## What Phase 4 added to the argument

One new consideration, in the *for* column, recorded because it is a real gap
rather than a hypothetical:

> **The arena cannot see an encoder it did not create.** Buffer reuse safety
> depends on knowing whether commands referencing a released buffer have been
> submitted. `TensorArena::encoder` tracks that; `WgpuBackend::encoder` does not.
> A caller who mixes the two can release a lease whose commands are still
> unsubmitted, and the pool would consider the buffer free.

A compute graph that owned execution would close this by construction, and that
is a genuine argument for one. It is not a *sufficient* argument, because there
is a much cheaper fix that does not require a scheduler: make the tracked encoder
the only ergonomic path, which is what `TensorArena::encoder` already is, and
document the untracked one as the escape hatch. `test_release_before_submit_is_not_recycled_early`
pins the tracked behaviour so a regression fails a test.

If a future phase finds callers routinely mixing the two — or a topology where
the tracked encoder is impractical — that observation moves to the *for* column
with evidence, and this section should be revisited.

## Verdict

**Still no ComputeGraph.** Two of the three conditions remain unmet, the third is
untested for the case that matters (branching), and the problem the graph was
originally proposed to solve — intermediate memory — was solved by ~600 lines of
buffer pool that reaches the hand-written optimum exactly, at ≤1 % runtime cost.

The next thing that would change this is a **branching or fan-out topology**, not
a longer linear chain. A linear chain of any length is handled by two leases.

---

# Phase 5 re-evaluation (2026-08-25) — after a real fan-out

Phase 4 closed with one condition explicitly **untested**: branching topology.
Phase 5 tested it. This section replaces speculation with a measurement.

## What was built and run

A diamond — `SpMM → shared → {ReLU, LeakyReLU} → merge → output` — with four
logical tensors and a computed liveness peak of **three** simultaneous buffers.

## Condition 1 — branching makes manual lifetime management impractical

**Refuted for this topology.** The arena reaches the liveness minimum of 3, and
the caller supplies exactly **one** piece of lifetime information: the lexical
scope in which `shared` lives.

```rust
let left  = arena.acquire(shape)?;      // branch outputs, live to the merge
let right = arena.acquire(shape)?;
let mut session = arena.session(Some("diamond"));
{
    let shared = arena.acquire(shape)?; //  <-- the ONE manual decision
    // ... SpMM into shared, both branches out of it ...
}                                       //  <-- shared dies here
let output = session.acquire(shape)?;   // reuses shared's allocation
```

That is one `{ }`. Everything else — which allocation is reused, when it is safe,
what epoch must complete — is the arena's.

## How error-prone is the release point, precisely

This is the question that decides whether liveness automation is needed, and the
answer is asymmetric:

| mistake | consequence | detected by |
|---|---|---|
| release **too early** | would corrupt results | **compile error** — prepared operations borrow their operands, so `shared` cannot be dropped while any operation using it is alive |
| release **too late** | 4 allocations instead of 3 | **nothing** — silent, and costs memory only |

The dangerous direction is impossible. The remaining direction is a *performance*
mistake, not a correctness one, and it degrades toward the naive allocation the
arena was replacing — not toward wrong numbers.

> **Correction (PHASE-5-CORRECTIVE).** This claim was premature when written:
> `ArenaTensor::tensor()` was still public and returned `&GpuTensor<f32>`, whose
> scope is `Untracked`, so a pooled buffer could still reach a raw encoder
> through one explicit method call. Found by an independent API audit and
> confirmed by a probe that compiled. The statement holds from the corrective
> onward, where the accessor is `pub(crate)` and `TensorOperand` is sealed.

That asymmetry is the strongest argument yet against a graph: the failure mode a
liveness analyser would prevent is one the type system already prevents, and the
one it would optimise is bounded by "no better than not using the arena".

## Condition 2 — a backend without `CommandEncoder` semantics

**Still not met.** No CUDA or FPGA backend exists.

Phase 5 did add something a future backend must model: `TrackedSession` couples
"a command buffer" with "a submission the pool observes". A backend whose
sequencing unit is not an encoder would need its own session type, but the
`SubmissionEpoch` watermark is expressed in terms of submissions completing, not
in terms of wgpu, so that part ports.

## Condition 3 — a measured win from reordering

**Still not met**, and the diamond makes it slightly stronger. The two branches
are genuinely independent and *could* be reordered — but they are already encoded
into one command buffer with no barrier between them beyond what WebGPU inserts,
so there is no scheduling freedom left for a graph to exploit. Reordering them
changes nothing measurable.

## Measured cost of the tracked-session machinery

| quantity | measured |
|---|---:|
| pure `encode` per operation, untracked | 0.41 µs |
| pure `encode` per operation, tracked | **0.36 µs** |
| session create + drop | 325 ns |
| cache-hit `acquire` + release | 226 ns |
| same-session `acquire` | 603 ns |
| fan-out, arena vs manual (4 runs) | +12 %, −12 %, +2.8 %, −0.8 % → no regression |

Tracking costs one `usize` increment per encoded operation. It does not show up.

## Verdict

**Still no ComputeGraph**, and the case is now weaker than it was in Phase 4, not
stronger: the one untested condition has been tested and did not hold.

### What a graph would minimally solve, stated precisely

Having run the fan-out, the residual problem is exactly one thing:

> **Computing release points automatically.** The caller decides when a value
> dies. Deciding it too late is silent and costs allocations. A liveness analysis
> over a declared topology would compute the optimal points instead.

That is the whole remaining benefit. It is a *memory-optimality* benefit with a
silent-but-bounded failure mode, on topologies whose manual cost is currently one
lexical scope. It does not justify a scheduler, a node hierarchy, or deferred
execution.

### What would change this

Unchanged from Phase 3/4, minus the condition now discharged, plus one sharpened
by the fan-out:

1. A topology where the number of simultaneously-live values exceeds what a
   reader can track by eye — realistically **8+ overlapping live ranges with
   irregular structure**, not a diamond and not a chain.
2. A backend whose sequencing unit is not a command encoder.
3. A measured win from reordering.
4. **New:** evidence that callers routinely release too late in practice. That is
   a claim about human error rates on real models, and it needs real models —
   the correct evidence is a profile of an actual HGNN forward pass showing
   arena allocations above the computed liveness minimum, not an argument.
