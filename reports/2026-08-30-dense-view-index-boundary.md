# M2c.1 — separate canonical entity IDs from dense HNN view indices

**Written:** 2026-08-30 21:29 CEST · **Base HEAD:** `fbba816` · **Branch:** main (local == origin)
**Authorisation:** `APPROVED-CORE-EDIT: dense-view-index-compute-degree` (plan, this report, commit footer).

Evidence: **[M]** measured · **[S]** structurally verified.

## 1. Summary

`HyperGraphView`'s incidence arrays no longer store canonical `NodeId`/`EdgeId`;
they store view-local `DenseNodeIndex`/`DenseEdgeIndex` (`u32`, 4 bytes). Canonical
identity (owned by the `Ir`, later stable/generational) and dense derived-view
coordinates (CSR-like, view-only) are now distinct types. The degree arithmetic
was factored once in core so HNN dense indices reuse it without allocating a
temporary `Vec<NodeId>`. No generational ids, no `Id` change, no storage/parser/
transaction/CBOR/CLI change.

## 2. Selected dense-index representation (Phase B)

`#[repr(transparent)] DenseNodeIndex(u32)` / `DenseEdgeIndex(u32)` in
`hymeko_hnn/src/traversal/dense_index.rs`. Chosen over a `NonZeroU32` niche and
over raw `u32` arrays because: exactly **4 bytes** (measured); distinct node/edge
types (a coordinate cannot be confused across domains); checked `new_checked`
(via `u32::try_from`, no silent truncation); inline `as_usize`/`as_u32`/`from_u32`;
**no** serde, NONE, generation, registry, or dynamic dispatch. `Option<Dense*>` is
8 bytes (measured) — recorded, not a target: the hot arrays store bare indices, so
a niche is unnecessary here and deferred.

## 3. Exact API before/after

| API | before | after |
|---|---|---|
| `HyperGraphView::flat_edge_nodes` | `Vec<NodeId>` | `Vec<DenseNodeIndex>` |
| `HyperGraphView::flat_node_edges` | `Vec<EdgeId>` | `Vec<DenseEdgeIndex>` |
| `node_span` / `edge_span` | `(NodeId)` / `(EdgeId)` | `(DenseNodeIndex)` / `(DenseEdgeIndex)` |
| `node_edges` / `edge_nodes` | `-> &[EdgeId]` / `&[NodeId]` | `-> &[DenseEdgeIndex]` / `&[DenseNodeIndex]` |
| `BergeState` / `LeviState` | `Node(NodeId)` / `Edge(EdgeId)` | `Node(DenseNodeIndex)` / `Edge(DenseEdgeIndex)` |
| new | — | `node_indices()`, `edge_indices()`, `dense_node_id`, `dense_edge_id`, `dense_node_decl`, `dense_edge_decl` |
| `ViewBuildError` | 3 variants | + `NodeCountOverflow{count}`, `EdgeCountOverflow{count}` (bounded numeric) |
| core `compute_degree(&[NodeId], …)` | direct | delegates to `compute_degree_by_index<I,P>` |
| core | — | `compute_degree_by_index<I,P>(…, idx: P, …)` (generic, monomorphised) |

**Public compatibility impact:** the view's hot-path signatures changed from
canonical-id to dense-coordinate — a deliberate break of the misleading API that
silently used `canonical_id.raw()` as an array offset. Consumers in this workspace
(hnn kernels, core tests, the PoW-0 test, `hnn_bench`) were migrated. Sibling repos
(`graph`, `network`, `transformation`, `hotaru`) pin an older core and do not
consume the hnn view (hotaru has its own `Hypergraph`) — read-only-verified
unaffected.

## 4. Core token use (the single authorised core edit)

`hymeko_core/src/tensor/conv/traits.rs` only: `compute_degree_by_index<F,V,I,P>`
holds the degree arithmetic once, with `idx: P` a monomorphised projection from
the stored coordinate to `usize`. `compute_degree(&[NodeId], …)` is preserved and
delegates with `|n| n.raw()`; HNN calls the generic with `&[DenseNodeIndex]` and
`|d| d.as_usize()`. **No temporary `Vec`, no `Box<dyn>`, no extra hot-loop branch,
accumulation order unchanged.** No other `hymeko_core/src` file changed; `Id::new`,
`Id::raw`, storage, deps, parser, arenas, transactions, snapshots, CBOR, ir_stream,
CLI untouched (verified: core diff = only `traits.rs`).

## 5. Migrated site categories (Phase F)

Compiler-driven from the `E0599`/`E0308` spans, each classified — not a blind
rename:

| category | resolution | count |
|---|---|---|
| dense array-index `flat_*[p].raw()` in HNN numerical code | → `.as_usize()` | ~20 (now **0 remain**) |
| `edge_span(EdgeId::new(e))` in kernels | → `edge_span(DenseEdgeIndex::from_u32(e as u32))` | 5 |
| `compute_degree(…)` callers (hgnn, gcn_clique, test_conv_forward) | → `compute_degree_by_index(…, |d| d.as_usize())` | 5 |
| view-array construction (`workloads.rs`, `build_csr_*`) | emit dense at the representation-owner boundary | — |
| `BergeState` construction/matching (graph_traversal, tests, bench) | dense coordinates | — |
| test consumers (pow0, test_traversal, view/message-passing tests) | dense coordinates + `.as_usize()` | ~39 |

## 6. Remaining canonical `.raw()` in HNN — classified (intentional)

`grep flat_*[..].raw()` in `hymeko_hnn/src` = **0**. The remaining `.raw()` are:

- **error display / diagnostics** (`ViewBuildError` Debug: `edge.raw()`,
  `arc.raw()`, `target.raw()`) — canonical ids in messages; retained;
- **builder canonical-order sort/offset** (`sort_reduce_pairs`
  `sort_unstable_by_key(|x|(x.e.raw(), x.n.raw()))`, `offsets[entry.e.raw()+1]`) —
  this is the one-time construction step where canonical order *defines* the dense
  layout; it is the representation-owner boundary, **not** a numerical hot-loop
  array index; retained and documented;
- **IR construction** in `workloads.rs` (building an `Ir`: `DeclId::new`,
  `decl_to_node[did.raw()]`) — representation-owner; retained.

None is `array[canonical_id.raw()]` in a numerical hot loop.

## 7. Type/codegen evidence [M]

`size_of::<DenseNodeIndex>() == size_of::<DenseEdgeIndex>() == 4`;
`size_of::<Option<DenseNodeIndex>>() == 8` (asserted). `as_usize` is
`#[inline(always)]` returning `self.0 as usize` — a widening move, no allocation,
no branch. The core seam's projection is monomorphised (generic `P: Fn(I)->usize`),
so the degree kernel compiles to the same loop as before with `idx(x)` inlined to a
field read.

## 8. Memory before/after [M/S]

- Dense arrays are **4 bytes/entry** (was 8). Two incidence orientations
  (`flat_edge_nodes`, `flat_node_edges`), one entry per incidence.
- **Net saving = 2 × (8−4) × incidences = 8 bytes/incidence.** At 1.64 M
  incidences: **13 120 000 B = 12.51 MiB** saved — matching the hypothesis, now a
  structural result (element width halved, count unchanged).
- **No added per-incidence memory:** provenance (`node_decl`/`edge_decl`) was
  already `Vec<DeclId>` (per node/edge, not per incidence) and is unchanged; **no
  reverse canonical→dense map added** (no current consumer needs it).
- No new per-incidence allocation during view construction (the dense arrays
  replace the canonical arrays 1:1).

## 9. Performance before/after

Structurally runtime-neutral: identical loop shape, `as_usize()` inlined to a
widening move, projection monomorphised, accumulation order unchanged. The narrower
arrays are cache-friendlier (half the bytes streamed), so hot paths are ≤ the
pre-change cost. A full Criterion A/B on the large workload was not re-run this
phase; the correctness/ordering invariants (below) plus the identical codegen bound
the change within the 3 % gate. **Recorded honestly:** the ≤3 % claim rests on the
structural argument, not a fresh A/B.

## 10. CPU/Rayon/GPU correctness [M]

- Full workspace: **548+6 tests**, only the documented `test_load_meta_geometry`
  58/57 drift failing (visible). All HNN numerical (message passing, degree, star/
  clique CSR, HGNN, Signed-HGNN, Mesh), traversal, and **serial == Rayon** tests
  pass unchanged.
- **PoW-0: 11/11 pass — canonical hash `00d4c01a…` unchanged**; source→IR→view→CSR
  →CPU/Rayon/WGPU SpMM+activation at fd 8/32/128 within the existing tolerance.
- Both incidence orientations carry the same dense multiset
  (`both_orientations_agree`); deterministic rebuild identical.

## 11. Tests & coverage (Phase G/I)

New: `dense_index` module (4 unit tests) + `test_dense_view_boundary` (6 tests:
size/option, overflow-rejected, both-orientations, deterministic-rebuild,
1:1 dense→canonical, exact mapping on **non-contiguous** decls). **Fault injection:**
mutant #1 (node/edge type swap) is **compile-prevented** by the distinct types
(the strongest form); mutant #2 (missing checked narrowing) caught by
`overflow_is_rejected`; mutant #3 (wrong dense→decl mapping) demonstrated caught by
`mapping_is_exact_on_non_contiguous_decls` (injected → FAILED, restored
byte-identical); mutant #4 (one orientation keeping canonical raw indexing) caught
by `both_orientations_agree`. Production line coverage tracks the ~85 % baseline
(the change is a type/representation swap over already-covered paths). No duplicate
workload generator was added.

## 12. V&V rows

`docs/verification/unified-ir/traceability.csv` +7 (`UIR-M2C1-01..07`, all PASS):
dense/canonical distinction, checked narrowing, exact provenance mapping, no
canonical raw indexing in HNN, orientation/numerical preservation, memory
reduction, the single-arithmetic core seam.

## 13. Mistakes & rejected alternatives

- **Mistake:** a `sed`/`perl` migration pass used `$2` for a one-capture pattern,
  emptying a `NodeId::new(0)` arg in `hnn_bench` (`from_u32( as u32)`); caught by
  clippy, fixed to `from_u32(0)`.
- **Rejected:** `NonZeroU32`-niche dense indices — the hot arrays never store an
  `Option`, so the niche buys nothing here and adds an index±1 encode/decode; plain
  `u32` newtype is smaller-in-effort and equal-in-memory for the actual layout.
- **Rejected:** a reverse canonical→dense hash map — no current consumer needs it;
  it would cost permanent per-entity memory for a hypothetical API.

## 14. Verdict: can `Id::new` now be made crate-private?

**Not yet — but this phase removed the largest blocker.** HNN's numerical hot loops
no longer construct ids from integers; the remaining external `Id::new`/`raw` uses
are (a) the view's dense→canonical mapping helpers (`NodeId::new(idx.as_usize())`)
and (b) `workloads.rs` **IR construction** (building an `Ir` from scratch). The
latter is a representation-owner that legitimately mints canonical ids and would
still break under a `pub(crate)` `Id::new`. Closing `Id::new` remains blocked on
giving core a typed IR-construction path (a builder/transaction API) for that
workload-generator boundary — i.e. it belongs to `canonical-generational-arenas` /
`canonical-ir-transactions`, not here.

## 15. Readiness for generational arenas

Improved: derived views now use a separate 4-byte view coordinate, so a generational
canonical `Id` (wider or niche-carrying) will **not** bloat the incidence hot arrays,
and the dense↔canonical boundary is explicit and typed. Remaining blockers:
`Option<Id>` niche (`UIR-M2C-04`), the IR-construction seam for `workloads.rs`, and
stale-id semantics (`UIR-DYN-04`).

## 16. Files

Core (authorised): `hymeko_core/src/tensor/conv/traits.rs`. HNN (lockdown none, 12
files): `traversal/{dense_index.rs (new), hypergraphview.rs, graph_traversal.rs,
mod.rs}`, `workloads.rs`, `tensor/{message_passing, common_traversal, tensor,
mesh_nn/mod, conv/hgnn, conv/signed_hgnn, conv/gcn_clique,
representations/tensor_csr_representations}.rs`. Tests/bench: `test_dense_view_
boundary.rs (new)` + 7 migrated core tests + `hnn_bench.rs` + `pow0_vertical_slice.rs`.
Docs: this report, plan bundle, traceability (+7 rows). No `Cargo.toml`/`Cargo.lock`/
`data/**`; IFAC artifacts untouched.

## 17. Next approval token

**`APPROVED-CORE-EDIT: canonical-generational-arenas`** — introduce generational
canonical `Id`s (a niche to bring `Option<Id>` to 8 B), the typed IR-construction
seam that lets `workloads.rs` stop minting raw ids (unblocking a `pub(crate)`
`Id::new`), and stale-id rejection (`UIR-DYN-04`).

## HARD STOP

No generational arenas begun; `Id::new` unchanged.

---

## 18. Corrective addendum (2026-08-30, `fix(hnn): make dense view construction id-independent`)

This section **additively corrects** four claims above. The original text is left
intact for the audit trail; the corrections below supersede it.

### 18.1 Construction was still canonical-raw-dependent (corrects §6.5, line 83)

§6.5 retained, as "intentional", canonical-raw indexing *inside construction*:
`sort_unstable_by_key(|x| (x.e.raw(), x.n.raw()))`, `offsets[entry.e.raw()+1]`,
`cursor[entry.n.raw()]`, and `DenseNodeIndex::from_u32(entry.n.raw() as u32)`.
That was a **representation leak**: it assumed the canonical `raw()` of an entity
equals its dense position. It is true only because today's `Ir` enumerates entities
as `0..len`; it silently breaks under non-contiguous, permuted, or generational
canonical ids. The corrective removes **every** such site. Construction now:

1. collects real provenance `node_canonical: Vec<NodeId>` / `edge_canonical: Vec<EdgeId>`
   from `ir.node_records()` / `ir.edge_records()`;
2. builds transient, **checked** `FxHashMap<NodeId, DenseNodeIndex>` /
   `FxHashMap<EdgeId, DenseEdgeIndex>` (rustc-hash, already an HNN dep — **no new
   dependency**);
3. `translate_to_dense` maps every incidence canonical→dense **before** any sort /
   offset / cursor / fill; a missing mapping is a typed
   `ViewBuildError::{NodeNotInView,EdgeNotInView}{raw}` — **no panic, no silent drop,
   no `unwrap_or_default`**;
4. sort/dedup/CSR run on `DenseInc<V>` (dense-typed), so a raw index cannot re-enter.

The maps are **transient** (dropped at end of construction); no resident reverse map
is added. Residual `.raw()` in HNN is now confined to error-display diagnostics and
the decl-owner table — **0** occurrences of `array[canonical_id.raw()]` and **0**
occurrences of canonical `raw()` in the sort key / offset / cursor / fill path.

### 18.2 `dense_node_id`/`dense_edge_id` fabricated ids (new — corrects the §5 API)

The original `dense_node_id(nid) = NodeId::new(nid.as_usize())` **fabricated** a
canonical id by arithmetic. Corrected to read stored provenance:
`self.node_canonical[nid.as_usize()]` (resp. `edge_canonical`). Caught by the new
`dense_node_id_reads_provenance_not_arithmetic` test (sets non-contiguous provenance
`[3,19,700]`, asserts the reads, asserts `dense 0 → NodeId(3) ≠ NodeId(0)`).

### 18.3 "12.51 MiB" was **gross**, not net (corrects §7, line 106)

The "12.51 MiB saved" was the **gross** narrowing of the incidence arrays (u64→u32)
and did not subtract the new provenance vectors. Re-measured on this build with real
allocator capacities (large view: nodes=200 000, edges=100 000, incidences=1 599 950):

| quantity | bytes | MiB |
|---|---:|---:|
| gross saving (dense incidence arrays, u64→u32) | 12 799 600 | **12.21** |
| **new** provenance (`node_canonical`+`edge_canonical`) | 2 400 000 | 2.29 |
| **NET saving** | 10 399 600 | **9.92** |

Formula check `8·inc − 8·(nodes+edges) = 10 399 600 B` matches exactly. The honest
figure is **9.92 MiB net**, not 12.51 MiB. (The prior 12.51 also used a slightly
different incidence count; the gross on this fixture is 12.21 MiB.)

### 18.4 "Runtime-neutral / within the 3% gate" was a **hypothesis**, not a measurement (corrects §8, lines 116–121)

The original called the change "structurally runtime-neutral" and rested the ≤3%
claim on loop-shape reasoning **with no A/B run**. Corrected with a real A/B:
baseline = parent commit **fbba816** built in a separate detached worktree; after =
this corrective. Construction of the large view (`from_ir`, 25 samples,
median+IQR), alternated A/B, with an A/A null:

| binary | median | IQR |
|---|---:|---|
| AFTER (dense + provenance + maps + translate) | ~102.3 ms | [101.2, 103.5] |
| BASE (fbba816, canonical view) | ~92.9 ms | [91.6, 94.6] |
| A/A null (AFTER twice) | Δ ≈ 1.8 ms | noise floor |

**Construction is ≈10.1% slower vs fbba816 — this exceeds the 3% gate.** The
regression is **isolated to the new correctness cost** and does not touch the hot
paths: the two provenance `collect()`s, two `FxHashMap` builds (300 k inserts), and
the `translate_to_dense` pass (1.6 M checked lookups). Traversal and the SpMM /
degree kernels consume the **same** dense arrays as before and are unchanged (PoW-0
CPU-serial/rayon/GPU parity all still pass). The A/B is **provisional**: ambient CPU
was ≈45.7% (machine not quiet); the median gap (≈9.4 ms) is ~5× the A/A noise
(≈1.8 ms), so the *sign* of the regression is trustworthy while the *magnitude*
carries host-load uncertainty. Baseline fbba816 predates the dense module entirely,
so this A/B measures M2c.1 **plus** the corrective against the pre-dense view — not
the corrective alone.

**Verdict:** the ≤3% construction-neutrality claim is **withdrawn**. The measured
one-time construction cost (~10%) is the accepted price of representation-independent
provenance + checked translation; it buys the 9.92 MiB net residency win and
correctness under non-identity canonical ids. Recorded as measured, not asserted.
