# Hypergraph synchronization — feasibility analysis

**Date:** 2026-08-28 · **Status:** design note. **Nothing here is implemented.**

The word "synchronization" is used in this project for four different problems
that have almost nothing in common. Conflating them is how a system acquires
per-incidence locks it does not need, or ships a distributed protocol before
anyone has measured the cut size. This note separates them, states what the
current system already guarantees, and defines the experiments that would
justify building anything.

---

## 1. The four meanings

| # | Meaning | Question it answers | Current status |
|---|---|---|---|
| S-E | **Execution dependency** | Does `scatter` see what `gather` wrote? | Already guaranteed. §3 |
| S-R | **Representation coherence** | Do the edge-major and node-major layouts describe the same topology? | Guaranteed by construction. §4 |
| S-H | **Host/device coherence** | Is the GPU's copy the same snapshot the CPU is reasoning about? | **Unguarded.** §5 |
| S-D | **Distributed coherence** | Do partition boundaries agree across workers? | Not applicable yet. §6 |

They differ in what can go wrong, at what rate, and at what cost. S-E is a
correctness property of one command stream; S-D is a protocol between machines.
Treating them as one topic is the error this note exists to prevent.

---

## 2. What the current system actually is

Three facts constrain everything below, and all three are properties of code
that exists today rather than intentions.

**`HyperGraphView` is an immutable snapshot.** It is built once — from a
`HypergraphDesign` or from `Ir` — and thereafter only read. There is no method
that mutates topology, and the message-passing kernels take `&HyperGraphView`.
Nothing can observe a partially-updated view because nothing can update one.

**Gather and scatter have disjoint output ownership.** Edge-major gather writes
one slot per edge; node-major scatter writes one slot per node. Within either,
each output element is written by exactly one iteration. This is the same
property that let the CPU backend parallelise with Rayon without atomics, and
the same one that lets the GPU SpMM kernel avoid float atomics: one invocation
owns one output element exclusively.

**A single WGPU queue is ordered.** Commands submitted to one queue complete in
submission order, which is precisely the precondition Phase 5 relied on when it
justified a scalar completion watermark. Ordering carries the dependency; the
host does not need to observe it.

---

## 3. S-E — execution dependency: nothing to add

A `gather → scale → scatter` chain on one device is already correct without any
new mechanism:

* WGPU inserts a memory barrier between compute passes in the same command
  encoder. Phase 3 measured a resident two-operation chain relying on exactly
  this, and its output matched the CPU oracle.
* The chain therefore needs **one** submit and **no** host wait between stages.
  Phase 8 measured what a host wait costs when it is not needed: a readback
  round trip is 2.8–7.3 GB/s of wall time that a resident chain never pays.

**Per-incidence locks or atomics are not justified for static single-device
execution.** There is no shared mutable output to protect: the disjointness above
is structural. Adding atomics would cost throughput to defend an invariant the
data layout already provides.

The one caveat worth writing down: this holds for **one queue**. Multiple queues
would break both the ordering assumption here and the watermark assumption in
`completion.rs`. That is a reason not to add a second queue casually, not a
reason to add locks.

---

## 4. S-R — representation coherence: guaranteed by construction

Edge-major and node-major indices are derived from one `HyperGraphView` in the
same construction step. They cannot disagree, because there is no path that
updates one without the other — the snapshot is immutable, so "one is stale" is
not a reachable state.

This stops being free the moment topology becomes mutable. That is the subject
of §7.

---

## 5. S-H — host/device coherence: the real gap

This is where the current system has no guard at all.

`ResourceFactory::upload_csr` produces a `GpuCsrMatrix` that carries **shape and
nothing else**. It does not record which `HyperGraphView` it came from, or when.
A caller can:

* mutate the host-side weights, re-run the CPU path, and compare against a GPU
  result computed from the *previous* upload;
* rebuild a view with different topology and dispatch a `PreparedSpmm` whose
  bind group still references the old buffers;
* upload twice and lose track of which upload a given `PreparedSpmm` binds.

None of these is detectable today. They do not corrupt memory — Phase 4's
operand lifetimes and Phase 5's provenance types prevent *that* — but they
produce a numerically wrong answer with no error.

### Proposed model (not implemented)

```rust
/// Two independent version counters, because the two kinds of change have
/// very different costs.
struct HypergraphVersion {
    topology: u64,
    weights: u64,
}
```

**Which updates invalidate both orientations.** Anything that changes incidence
structure — adding or removing a node, an edge, or an incidence — invalidates
edge-major *and* node-major indices, both expansions, any cached degree vector,
and every uploaded buffer derived from them. Topology bumps invalidate
everything.

**Whether weight-only changes can avoid a topology rebuild.** Yes, and this is
the main reason for two counters. Incidence *values* live in a separate array
from the offsets and column indices. Changing a weight leaves `edge_offsets` and
`flat_edge_nodes` byte-identical, so a weight bump requires re-uploading only
the values buffer — for the large workload that is one of three CSR arrays.
Derived quantities that *fold* weights (degree vectors, normalised adjacency,
`clique_diag`) must still be recomputed, so a weight bump is cheap for transport
and not free for derived state. Both facts belong in the version, which is why
the counter is not a single number.

**What metadata a GPU upload should carry.** The `HypergraphVersion` it was
produced from, plus the source view's identity. A prepared operation would
capture the version it was built against, and dispatch would compare. That is a
`u128` comparison per dispatch against a kernel measured in hundreds of
microseconds — unmeasurable overhead for a class of silent wrong answers.

**How stale-resource use would be detected.** Compare at prepare time and at
encode time; mismatch is a `ComputeError`, not a warning. It must not be a debug
assertion: the failure it catches is a wrong number, and wrong numbers are worst
in release.

**Where snapshot publication becomes atomic.** At the point the new buffers are
complete and the version counter is bumped — an `Arc` swap of an immutable
snapshot record holding (buffers, version). Readers hold the old `Arc` until
they drop it, so an in-flight dispatch keeps a consistent view of the data it
started with. This is the same discipline `TensorArena` already uses for buffer
lifetime, applied to snapshot lifetime.

---

## 6. S-D — distributed coherence: not yet a question

**What a multi-GPU partition must exchange.** Partitioning a hypergraph cuts
*hyperedges*, not just node pairs. An edge whose nodes span two partitions
requires the partial aggregate from each side before its edge value is complete,
so the halo is not a ring of boundary nodes — it is the set of cut hyperedges,
and its cost scales with cut *cardinality*, not cut count. A single
high-cardinality edge spanning every partition is a global reduction.

That is the fact that makes this unmeasurable today: the exchange volume depends
on a partitioner nobody has run on this data.

**Whether synchronization cost can be amortized across inference steps.** Only
if topology is static across steps, which for inference it is. Then the halo
*pattern* is computed once and only values move per step, which is the
favourable case. It does not help training with changing topology.

**What FPGA DMA/stream boundaries would require.** A stream-oriented endpoint
wants fixed-size, statically-scheduled transfers. The variable-cardinality halo
above is the opposite of that, so an FPGA path would likely need a
cardinality-bucketed layout decided at partition time. That is a design
consequence of the hypergraph structure, not of any transport choice, and it is
worth knowing before an FPGA path is scoped.

---

## 7. Discriminating benchmarks, defined but not run

None of S1–S3 should be built before these produce numbers.

| # | Experiment | What it decides |
|---|---|---|
| B1 | Full view rebuild vs applying a topology delta | Whether incremental topology is worth its complexity |
| B2 | Weight-only upload vs full CSR re-upload | Whether the two-counter split pays for itself |
| B3 | CPU→GPU snapshot publication latency at each scale | The per-step floor for any versioned scheme |
| B4 | `gather → scale → scatter` in one submit, no host wait | The resident baseline every other number is measured against |
| B5 | The same chain with a forced host wait between kernels | The cost of *not* keeping the chain resident |
| B6 | Partition halo size vs partition count, on real topologies | Whether the cut is small enough for distribution to help at all |
| B7 | Computation/communication overlap ratio | Whether B6's exchange can be hidden |

B4 and B5 are runnable today with existing code. B1–B3 need the versioned
snapshot. B6–B7 need a partitioner.

**No claim that distributed or dynamic synchronization is worthwhile will be
made until update frequency, cut size and the compute/communication ratio are
measured.** The hypergraph structure makes the pessimistic case genuinely bad —
one large cut hyperedge is a global reduction — so the burden of evidence sits
with the proposal, not against it.

---

## 8. Decision

**S0 — static single-device execution needs no additional synchronization.**
Adopted. §3 and §4 are structural guarantees of existing code, not aspirations.

**S1 — versioned host/device snapshot coherence is justified next.** Adopted as
the next candidate, not implemented in Phase 9. It is the only one of the four
with a demonstrated gap (§5), the failure it prevents is a silent wrong answer,
and the runtime cost is an integer comparison against a kernel three orders of
magnitude more expensive. B2 and B3 should run before the design is fixed.

**S2 — dynamic topology synchronization needs update-workload evidence.**
Deferred. Nothing in the repository mutates a `HyperGraphView` after
construction, so there is no update workload to measure yet. B1 defines what
would have to be measured first.

**S3 — distributed synchronization needs a partition/halo prototype first.**
Deferred, and deliberately last. Hyperedge cut cardinality (§6) is the term that
decides it and is entirely unmeasured. B6 and B7 come before any protocol.
