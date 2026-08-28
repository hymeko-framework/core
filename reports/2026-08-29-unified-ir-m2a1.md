# Unified IR M2a.1 — accessor codegen regression resolved

**Date:** 2026-08-29
**Authorisation:** `APPROVED-CORE-EDIT: unified-ir-m2a1-accessor-performance`
**Addendum to:** `reports/2026-08-28-unified-ir-m2a.md`

---

## 0. Verdicts

| Gate | Verdict |
|---|---|
| **SCALAR** | **ZERO-COST** — generated code equivalent; measured within noise on random point *and* dependent-chain latency |
| **SCAN** | **WITHIN-NOISE** — iterator vs raw `Vec` iteration, 5.47 µs vs 5.47 µs |
| **MAP** | **WITHIN-NOISE** — `as_node` vs a fair direct control, +1.6 % / +2.7 % |
| **CHECKED-COMPOSITE** | `node_for_decl` **equals** a hand-written composite; its extra cost is the back-link validation it is specified to do |
| **M2b may begin** | **Yes** |

### Starting state

| Item | Value |
|---|---|
| Local HEAD | `e660b491c10938debec0dbe12969fb2f9c42b940` (`main`) |
| Tree at HEAD | `9248d275ade07687ce92cc990471f3a53233366d` |
| Worktree | clean but for the three untracked IFAC artifacts, preserved |
| Host at measurement | median 3 % (min 0, max 6) for the primary runs; drift documented in §5.4 |

---

## 1. The M2a hypothesis was wrong

M2a attributed the +20…49 % accessor overhead to "the cost of materialising `Option<&T>` and branching on it". **That explanation is false**, and two facts refute it:

| Fact | Value |
|---|---|
| `size_of::<&DeclNode>()` | 8 |
| `size_of::<Option<&DeclNode>>()` | **8** |

`Option<&T>` is niche-optimised. It occupies no extra space and requires no materialisation — the null pointer *is* the `None` discriminant. There was never a representation cost to pay.

The second fact is the one that mattered:

```rust
[1u8, 2, 3].get(usize::MAX)  // => None
```

`DeclId::NONE` **is** `usize::MAX`, and a `Vec` length can never reach it. So the explicit guard I wrote —

```rust
if id.is_none() { return None; }
self.nodes.get(id.0)
```

— was **redundant**. `.get()` already rejects the sentinel.

Both facts are now pinned by tests (`tests/unified_ir_m2a/sentinel.rs`), so the claim cannot rot.

---

## 2. Codegen: what the redundant guard actually cost

Isolated in a standalone probe compiled with `rustc -O --emit=asm` — no tooling installed, nothing retained in git.

**Guarded** (M2a), per element in the hot loop:

```asm
cmpq  $-1, %rdx      ; is_none()
setne %bl            ; materialise a bool
cmpq  %rcx, %rdx     ; bounds check
setb  %sil           ; materialise a bool
testb %sil, %bl      ; combine
je    .LBB0_9
```

**Guard-free**, per element:

```asm
cmpq  %rcx, %rdx     ; bounds check
jb    .LBB0_6
```

**Six instructions versus two.** The damage is worse than the instruction count suggests: with two independent conditions LLVM abandons the branch and emits a **branchless `setne`/`setb`/`testb` chain**, so every element pays the full sequence. With one condition it emits a single predictable branch that the predictor handles for free.

LLVM cannot eliminate the redundancy itself. Doing so requires proving `x < len ⟹ x ≠ usize::MAX`, which needs the fact that a `Vec` length is bounded by `isize::MAX`. It does not derive that here.

**So the regression was self-inflicted, in code I wrote in M2a, and it was avoidable.**

### Inlining and other checks

- Every accessor **is** inlined at the call site — the probe shows no `call` in the loops.
- The `Vec` length **is** hoisted out of the loop (`movq 16(%rcx), %rcx` before the loop; the comparison reuses `%rcx`).
- Both forms unroll 2×; neither vectorises, which is expected for gather-style loads.
- `node_for_decl`'s extra work is exactly its specification: a second bounds-checked load plus a `decl == id` comparison.

---

## 3. What changed

Guards removed from the `Option`-returning lookups only:

| Method | Location | Change |
|---|---|---|
| `Ir::node` / `edge` / `arc` | `ir/access.rs` | `self.nodes.get(id.raw())` |
| `Ir::decl_node` | `ir/ir.rs` | `self.decl_nodes.get(id.raw())` |
| `Ir::as_node` / `as_edge` / `as_arc` | `ir/ir.rs` | `.get(d.raw()).copied().flatten()`, plus `#[inline]` |

**Guards deliberately kept** — and tested:

`Ir::parent`, `Ir::first_child`, `Ir::next_sibling`, `Ir::decl_children`. These index **directly** after the check (`self.decl_nodes[item.0]`), so their `NONE` guard is load-bearing: removing it would convert absence into a panic. `contains_*` and the `_of` hierarchy accessors inherit the fix through `decl_node` and need no change of their own.

`#[inline]` was added to `as_node`/`as_edge`/`as_arc`, which previously had none and are now cross-module hot paths. No `#[inline(always)]` was applied anywhere — nothing measured justified it, and blanket application costs instruction cache.

**Behaviour is unchanged for every id**, proved three ways: the sentinel/bounds argument above, a test comparing each optimised method against an explicitly-guarded reference implementation over every valid id plus `n`, `n+1`, `usize::MAX-1` and `usize::MAX`, and the unchanged full suite (313 passed / 1 known drift, identical to M2a).

---

## 4. The benchmark model was also wrong

M2a looped **sequentially over every dense id**. That measures a scan with perfectly predictable, independent loads — not scalar point lookup. M2a.1 separates the cases.

| Cell | What it measures |
|---|---|
| `m2a1_random_point` | deterministically shuffled valid ids — defeats prefetch and stride prediction |
| `m2a1_dependent_chain` | next id derived from the record just read — defeats load overlap; true latency |
| `m2a1_mixed_validity` | 1 % / 10 % / 50 % invalid, mixing `NONE`, past-the-end and far-out-of-range |
| `m2a1_decl_to_record` | mapping-only pair kept separate from the checked-composite pair |
| `m2a1_end_to_end` | full read-only traversal of every edge, arc and reference |

The shuffle uses the LCG pattern already established in the Phase 10B1 model tests — **no dependency added**.

---

## 5. Results

Variants registered in alternating rounds throughout.

### 5.1 The direct before/after, same cell, Criterion's own baseline

| Cell | Change vs M2a | p |
|---|---:|---:|
| `m2a_decl_lookup/accessor` @ 1 000 | **−32.7 %** | 0.00 |
| `m2a_decl_lookup/accessor` @ 10 000 | **−18.3 %** | 0.00 |

Absolute: **527.9 ns**, against the M2a `direct_guarded` control of **527 ns**. The gap M2a reported as +49 % is **gone**.

### 5.2 Scalar lookup — ZERO-COST

| Cell | Direct guarded | Accessor | Verdict |
|---|---:|---:|---|
| random point, 1 000 | 520.3 ns | 519.6 ns | intervals overlap |
| random point, 10 000 | 7.53 µs | 7.55 µs | intervals overlap |
| **dependent chain** (true latency) | 199.8 µs | 200.7 µs | intervals overlap |

The dependent chain is the strongest of these: it removes load-overlap entirely, so it measures latency rather than throughput, and the two forms are still indistinguishable.

### 5.3 Mixed validity — the accessor is *faster*

| Invalid fraction | Direct guarded | Accessor | Delta |
|---|---:|---:|---:|
| 1 % | 9.61 µs | **7.16 µs** | **−25 %** |
| 10 % | 8.82 µs | **6.60 µs** | **−25 %** |
| 50 % | 12.3 / 15.1 µs | 12.2 / 16.0 µs | equal — misprediction dominates |

The accessor now **beats** the hand-written control, because that control still performs the double check (`d.is_some() && d.0 < len`) that the accessor shed. This is the same defect measured from the opposite direction, and it is a useful confirmation: the fix is not a benchmark artefact.

### 5.4 Mapping and composite

Settled rounds (the `r0` round of this group was contaminated — ambient 13 % at start, and it reported `as_node` at 21.3 µs against 10.2 and 9.2 µs in the following rounds; alternation is what made that visible):

| Pair | Direct | Accessor | Delta |
|---|---:|---:|---:|
| map-only, r1 | 9.95 µs | 10.22 µs | +2.7 % |
| map-only, r2 | 9.10 µs | 9.25 µs | +1.6 % |
| composite, r1 | 17.02 µs *(manual)* | 16.78 µs *(`node_for_decl`)* | −1.4 % |

**MAP passes at < 3 %.** An earlier run showed `as_node` at +24 %, and that was **my benchmark's fault again**: the control called `.is_some()` and never extracted the id, loading only the 8-byte discriminant, while `as_node` returns the id and loads the whole 16-byte `Option<NodeId>`. Making the control extract the id closed the gap. This is the same error class as M2a's original comparison — I made it twice.

`node_for_decl` matches a hand-written composite, so its cost over mapping-only is the back-link validation it exists to perform, not overhead.

### 5.5 Scan and end-to-end

| Cell | Direct | Accessor |
|---|---:|---:|
| iteration, 10 000 | 5.47 µs | 5.47 µs |
| end-to-end traversal | 28.8 / 29.5 µs | 30.9 / 28.5 µs |

Iteration is **identical**. End-to-end alternates in both directions across rounds — within noise, and the absolute effect of the whole accessor layer on a full IR traversal is unmeasurable against ambient variation.

### 5.6 Host stability, reported not hidden

The M2a report flagged an unexplained 16.5 → 7.0 µs shift on the 10 000-element cell. M2a.1 now has a partial explanation: **that group is load-sensitive**. During this phase the same cells moved by 70 % when ambient load was 37 %, and settled when it returned to 3–8 %. Every number above is from a run with ambient ≤ 13 % and is annotated where drift was visible. The 1 000-element cells were stable throughout and carry the load of the conclusions.

This does **not** fully explain the M2a observation, which occurred at ambient 2–4 %. That remains open.

---

## 6. Acceptance

| Requirement | Result |
|---|---|
| No allocation in lookup | ✅ — every accessor returns a reference or `Copy` id |
| Invalid lookup O(1) | ✅ — one bounds check, no scan |
| Iterator no allocation | ✅ — `enumerate().map()`, no collection |
| No `unsafe`, no `get_unchecked` | ✅ — verified by grep in the audit |
| No construction / hash / HNN / WGPU regression | ✅ — those paths untouched; suite unchanged |
| Code-size change | Not material: three guard blocks removed, three `#[inline]` added. No `inline(always)`. |

---

## 7. New tests

**10 new**, `tests/unified_ir_m2a/sentinel.rs`:

`option_reference_is_niche_optimised` · `the_none_sentinel_is_rejected_by_the_bounds_check_alone` · `guard_free_lookups_reject_none` · `guard_free_lookups_reject_exact_end_and_far_out_of_range` · `guard_free_lookups_still_resolve_first_middle_and_last` · `an_empty_ir_rejects_every_id_including_zero` · `retained_guards_still_return_none_for_the_sentinel` · `backlink_validation_survives_the_optimisation` · `hierarchy_accessors_remain_total` · `guard_free_agrees_with_an_explicitly_guarded_reference_implementation`

The first two pin the *reasoning*, not just the behaviour: if `Option<&T>` ever stops being niche-optimised, or `NONE` stops exceeding any possible length, the optimisation's premises fail loudly rather than silently.

M2a's 19 accessor tests and the 96-fixture corpus comparison are unchanged and still pass.

---

## 8. M2b migration rules

1. **Dense scans use the iterators** — `decls()`, `node_records()`, `edge_records()`, `arc_records()`. Measured identical to raw `Vec` iteration.
2. **Point lookups use the accessors** — `decl_node`, `node`, `edge`, `arc`. Measured zero-cost.
3. **Mapping-only migration uses `as_node`/`as_edge`/`as_arc`.** Do **not** substitute `node_for_decl` mechanically: it additionally resolves the record and validates the back-link, which is more work and a different contract. Use it when you want the record *and* the guarantee.
4. **Do not** convert a repeated accessor lookup into a hidden dense iterator. The contracts are deliberately distinct.

---

## 9. Mistakes

1. **I misdiagnosed the M2a regression.** I attributed it to `Option<&T>` materialisation without checking that `Option<&T>` is niche-optimised — a one-line check I did not run. The real cause was a redundant branch I had written myself.
2. **I built an unfair benchmark control twice.** First in M2a (direct indexing vs a safe accessor), then again in M2a.1 (`is_some()` vs a value-returning `as_node`). Both times the "overhead" was work my control never performed.
3. **I measured on a host at 37 % ambient load** and had to discard the run. The alternating-round discipline is what made the contamination visible rather than plausible.
