# Unified IR M2c — readiness audit

**Date:** 2026-08-29 · **DESIGN AND AUDIT ONLY. No production source changed, no layout finalised.**

M2c is the stage that seals the ID representation: removing `pub .0`, `From<usize>` and `From<Id<T>> for usize` so that a later representation change fails loudly instead of silently indexing the wrong element. This audit establishes what that would break, and compares the candidate stable-ID layouts on evidence rather than preference.

---

## 1. Remaining representation-coupled sites

Measured across every repository, `.rs` files referencing an ID type, excluding `target/` and the backup mirror.

| Repository | `.0` access | `From`/`Into` usize | `raw()` | literal IDs |
|---|---:|---:|---:|---:|
| `hymeko_core` | 403 | 14 | 30 | 51 |
| `transformation` | 250 | 13 | 24 | 26 |
| `graph` | 137 | 37 | 1 | 0 |
| `hymeko_hnn` | 26 | 0 | 0 | 3 |
| `vscode-plugin` | 14 | 1 | 16 | 0 |
| `network` | 4 | 12 | 1 | 0 |
| `hymeko_python` | 4 | 0 | 10 | 0 |
| `hotaru` | 3 | 0 | 3 | 0 |
| `tensor` | 0 | 0 | 0 | 0 |
| **total** | **841** | **77** | **85** | **80** |

The `.0` column is the migration's real size and it is **larger than the M2b dense-index inventory** (196), because it counts every field access, not only array indexing — formatting, comparison, hashing and pass-through included. Most are mechanical; the hazardous subset is the one that indexes.

## 2. Classification

### 2.1 Representation-owner internals — `.0` is legitimate and stays

| Site | Why |
|---|---|
| `ir/ir.rs` | *is* the storage; accessors are built on it |
| `ir/access.rs` | the encapsulation boundary — indexing lives here by design |
| `ir/lower.rs` | construction, before ids are meaningful |
| `ir/canonical_hash.rs`, `ir/hash_pass.rs` | representation-owner algorithms; M0.6 measured the accessor cost as acceptable but these are inside the boundary |
| `ir/coherence.rs` | must read raw storage to *detect* incoherence |
| `common/ids_impl.rs` | defines the representation |

### 2.2 Published contracts — these are what M2c actually breaks

| Contract | Surface | Risk |
|---|---|---|
| **Python `name_to_id()`** | `BTreeMap<String, usize>` | **Highest.** Python receives raw ids and can hold them across calls. Nothing documents an invalidation rule. |
| Python DTO fields | `id`, `hyperarc_id`, `edge_id`, `target_id`, `position` — all `usize` | 38 pyo3 attribute sites |
| JSON snapshot DTO | `target_id`, `id`, `from`, `to` in `hymeko_formats::snapshot` | Consumed by the browser demo and by Python. **Versionless** — no `schema_version` field |
| `vscode-plugin` | 16 `raw()` + 14 `.0` | reads ids across a process boundary |

### 2.3 The contract that is already safe

> **The IR-stream wire protocol carries no entity IDs at all.** A grep for `DeclId|NodeId|EdgeId|HyperArcId` across `core/hymeko_core/src/ir_stream/` returns **zero** matches. Its sections are CSR offsets, column indices, weights, features and params — `u32`/`f32` arrays whose indices are *positions within the message*, not IR identities.

This materially reduces M2c's blast radius: the wire format needs no versioning for an ID change, and the Phase 10A/10B1 envelope is unaffected. It was worth checking rather than assuming, because Phase 0 had listed serialization as a high-risk category.

## 3. Compile-fail tests required before M2c

The point of sealing is to convert silent wrong answers into compile errors. That must itself be tested, or the seal is unverified. The repository already uses `compile_fail` doctests (`ir_stream/mod.rs`), so the pattern exists.

| Test | Must fail to compile after M2c |
|---|---|
| `let n: usize = decl_id.0;` | field is no longer `pub` |
| `let d = DeclId::from(7usize);` | `From<usize>` removed |
| `let u: usize = decl_id.into();` | `From<Id<T>> for usize` removed |
| `let e = EdgeId::from(node_id.0);` | the cross-domain confusion D4 permits today |
| `ir.decl_nodes[d.0]` from outside the crate | fields sealed |

The fourth is the one that matters: it is currently **legal** and is how a migration would silently index the wrong table.

## 4. Candidate stable-ID layouts

Measured on this toolchain (`rustc 1.97.1`), not assumed:

```
Option<usize>                      = 16   ← today
NonZeroU64                         =  8
Option<NonZeroU64>                 =  8
struct { idx: u32, gen: NonZeroU32 } =  8
Option<struct{idx,gen}>            =  8   ← niche from the NonZeroU32 field
Option<(u32, u32)>                 = 12   ← no niche
```

Two facts follow, and one is a surprise:

1. **A generational key is half the size of today's `Option<Id>`.** `Option<Id<T>>` is 16 bytes now because `usize::MAX` is a *valid* `usize` and leaves no niche. A generational key with a `NonZeroU32` generation is 8 bytes *including* the `Option`. Adding safety would **reduce** memory, not cost it.
2. **Bit-packing is unnecessary.** `struct { idx: u32, gen: NonZeroU32 }` already gets the niche from its field, so index and generation are plain field reads — no shift, no mask. A packed `NonZeroU64` is the same size and strictly more work to decode. **The obvious "pack it into a u64" instinct is the wrong one here.**

### Decision matrix

| Criterion | Generational slot arena | Tombstone arena | SlotMap-style key | Indirection table over dense storage |
|---|---|---|---|---|
| Lookup latency | 1 bounds check + 1 generation compare | 1 bounds check | as generational | **2 loads** (indirection + record) |
| Iteration locality | dense, skip vacant | dense, skip tombstones | dense | **poor** — indices scatter |
| Stale-ID detection | **yes**, generation mismatch | **no** — slot still readable | yes | yes, via table |
| Deletion | slot reused safely | slot burned forever | safe | safe |
| Memory under churn | bounded | **unbounded** | bounded | bounded + table |
| Serialization | id is (idx, gen); needs both | id is idx | as generational | id is stable, storage free to move |
| FFI/Python | 8 bytes, opaque | 8 bytes | 8 bytes | 8 bytes |
| Snapshot sharing | structure shareable by `Arc` | shareable | shareable | **table must be shared too** |
| Transaction rollback | restore generation counter | tombstones must be un-set | as generational | table restore |
| Distributed replay | **local-only** — needs the `bindings` map (§F.2) | local-only | local-only | **best** — table can hold globally stable ids |
| Memory / entity | 8 B key | 8 B key, wasted slots | 8 B key | 8 B key + 8 B table entry |
| `Option` niche | **yes** (NonZeroU32 gen) | no (all values valid) | yes | depends |

**Provisional recommendation, not a decision: generational slot arena**, `struct { idx: u32, gen: NonZeroU32 }`.

It is the only candidate that is simultaneously 8 bytes, niche-carrying, stale-ID-detecting and bounded under churn. Tombstones fail on two counts (unbounded memory, and a stale id reads the *deleted entity's last state* rather than faulting — strictly worse than a generation mismatch). The indirection table wins only on distributed replay, and §G.4 already solves that with the `bindings` map without paying two loads on every lookup.

**What would falsify it:** if `u32` proves too narrow for the index space (cap ≈4.29 × 10⁹ declarations), or if profiling shows the generation compare is not free on the hot path. Neither is currently measured; both are M2d work.

## 5. Compatibility and versioning plan

| Surface | Plan |
|---|---|
| **JSON DTO** | Add `schema_version` **now**, while the schema has not changed. Free today, expensive during a breaking change. Then: emit both `target_id` (legacy) and a versioned id for one release. |
| **Python** | Ids become opaque. `name_to_id()` keeps its signature but its values become the packed key; document that ids are valid only within one snapshot, and add an explicit invalidation rule — which does not exist today. |
| **Wire** | **No change required** (§2.3). |
| **On-disk** (`ir_writer`, PGIP) | Format version field; reader supports N and N−1. |
| **Rust API** | The M2a accessor layer already insulates every migrated consumer. The remaining exposure is the 841 `.0` sites, which M2c converts to compile errors. |

## 6. Rollback and coordinated release

1. **Seal first, change representation second.** M2c (sealing) and the representation change are separate commits. Sealing is revertable by restoring two `From` impls and a `pub`.
2. **Sibling repositories are already decoupled** — they build against pinned core commits (see `2026-08-29-unified-ir-m2b-cross-repo.md`), so a core change cannot break them until their lock is bumped deliberately. This is an accident of the current setup, and for M2c it is an advantage: the blast radius is opt-in per repository.
3. **Order:** core seals → core representation change → per-repository lock bump with its own gates → per-repository consumer migration.

## 7. M2c core-edit surface

Exactly:

- `common/ids.rs` — remove `pub` from the field; delete the false `#[repr(C)]` comment (D3, still present).
- `common/ids_impl.rs` — remove `From<usize>` and `From<Id<T>> for usize`, or seal them behind explicit `raw()`/`from_raw()`.
- New `compile_fail` doctests (§3).
- Every in-crate `.0` site that is *not* representation-owner internals — to be enumerated at M2c time, not now, because M2b's accessor layer will have absorbed some of them.

**Not in M2c:** the representation change itself, arenas, transactions, mutation.

## 8. M2d performance baseline and acceptance

The baseline exists and is on record — no new measurement is needed to set the budget:

| Quantity | Current | Source |
|---|---:|---|
| `decl_node` scalar lookup | ~0.35 ns | M2a.1, zero-cost vs guarded control |
| iteration, 10 000 decls | 5.47 µs | M2a.1, identical to raw `Vec` |
| `Option<Id>` size | 16 B | measured above |
| `decl_to_*` maps @ 20 001 decls | 960 048 B | Phase 0 §6.3 |
| `canonical_program_hash` @ 15 000 decls | 10.33 ms | M0.6, post-iterative |
| `HyperGraphView::from_ir` medium | 11.1 ms | M2b alternating A/B |

**Proposed M2d acceptance:**

- Scalar lookup: ≤ 3 % regression against the M2a.1 figure, with the generation compare included.
- Iteration: ≤ 3 %.
- `Option<Id>` size: **must fall to 8 B** — if it does not, the chosen layout lost its niche and the design is wrong.
- `decl_to_*` maps: **must fall to ~240 KB** at 20 001 declarations (4× from the niche).
- Canonical hash and `from_ir`: ≤ 3 %.
- Peak RSS: no increase.

The two "must fall" rows are the ones that make the change worth doing. If they do not hold, M2d has bought stale-ID safety at a memory cost rather than alongside a memory win, and that trade should be re-argued rather than assumed.

## 9. Open questions this audit did not settle

1. **Index width.** `u32` caps declarations at ≈4.29 × 10⁹. Unmeasured against any stated scale requirement.
2. **Generation width and wraparound.** A `NonZeroU32` generation wraps after ~4.29 × 10⁹ reuses of one slot. Whether that is reachable, and what should happen if it is, is undecided.
3. **`SymId` is deliberately excluded.** It is content-addressed and append-only, so it needs no generation (architecture §E.4). Sweeping it into the same change would add risk for no benefit.
4. **Python invalidation semantics.** No rule exists today for how long a Python-held id stays valid. M2c makes ids opaque; it does not answer this, and it must be answered before mutation lands.
