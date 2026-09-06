# Generational canonical entity storage — representation study & deferral decision

**APPROVED-CORE-EDIT: canonical-generational-arenas**
2026-08-31 19:38 CEDT · branch `feat/core-generational-arenas` · base `67771c9`

## Decision (up front)

**HARD STOP on production migration.** Of four arena representations measured
against the CLAUDE.md §1.5 gates, **none meets the ≤3 % valid-lookup regression
bar** — the best (SoA, rep B) is **+56 %**, 18× over budget. Per §1.5 the
prototype, benchmark and this decision are pushed; production migration is not
forced. Generational entity storage is **deferred** until a deletion-capable
mutable IR is a real requirement (see *Why the gate is structurally
unsatisfiable*).

## What was done

1. **ID-domain inventory** (`docs/plans/2026-08-31-generational-arenas/id-domain-inventory.md`).
   The four canonical entity IDs (`DeclId`/`NodeId`/`EdgeId`/`HyperArcId`) are in
   scope; `SymId`, interner indexes, dense node/edge indexes, CSR row/column and
   shader buffer indexes are excluded. The excluded dense/CSR/shader indices are
   raw integers (not `Id<T>`), so there is no accidental-generationalization risk;
   `SymId` *does* share `Id<T>`, which forces a **separate** `GenId<T>` type for
   the entity domains rather than a change to `Id<T>`.
2. **Representation prototypes** (`proto_arena.rs`, `bprime`) — four reps vs the
   current dense `Vec` baseline across the full §1.3 metric matrix.
3. **Wire-form design + validation** (`wire_form.rs`, frozen as tests in
   `tests/basic/gen_arena_contracts.rs`) — 8 B `GenId`, 8 B `Option<GenId>`, all
   known-answer / rejection / legacy vectors.

## Measured results (diagnostic, Instant-based; host x86_64 windows, rustc 1.97.1, release)

Regression vs BASE (today's dense `Vec<Rec>`, plain index, no generation),
median-of-9, at N = 100 k / 1 M:

| rep | append build | **random valid lookup** | iteration | bytes/live |
|---|---|---|---|---|
| A tombstone `Slot{gen,Option<Rec>}` | +37 … +47 % | **+72 … +122 %** | +63 … +75 % | 48 (base 32) |
| B SoA `gens`+`vals` (parity) | −7 … −17 % | **+56 … +59 %** (best) | +30 … +55 % | 36 |
| B′ interleaved `{gen,Rec}` | +25 % | **+66 … +99 %** | +58 … +65 % | 40 |
| C dense + slot indirection | +2 … +30 % | **+147 … +150 %** | −4 … −8 % | 44 |

Tombstone density (rep B, N = 1 M): lookup speeds up as live-count drops (0 % →
11.5 ms, 50 % → 5.1 ms over the live set); iteration *slows* (scans dead slots,
2.48 → 3.20 ms); bytes/live grows 36 → 76 (dead slots retain record storage).
Insert/remove/reuse churn (100 k ×3 half-cycles) = 2.68 ms, allocation-free via
free-list reuse. **Stale detection is O(1)** in every rep (single generation
compare) — the one gate all reps pass.

**Handle size is 8 B** in every rep; the wire form achieves 8 B `GenId` **and**
8 B `Option<GenId>` via a `NonZeroU64` niche — so the size invariants are
reachable. The blocker is purely lookup latency.

## Why every representation fails the lookup gate (mechanistic, not a guess)

Today's entity lookup is `self.<table>.get(id.raw())` — **one bounds-checked
load** into a dense `Vec` (≈ 7.1 ns at 1 M, cache-miss-bound). A generational
lookup must additionally prove the handle is current, which forces one of:

- **a second array load** — read `gens[slot]` before `vals[slot]` (B, C): a
  second cache miss ⇒ +56 … +150 %;
- **a fatter slot** — carry the generation inline with the record (A, B′): +25 …
  +50 % bytes/entity ⇒ more memory traffic per random access ⇒ +66 … +122 %.

There is no third option: the generation must be read and compared on the hot
path, and on a single-load dense lookup that is unavoidably ≫ 3 %. The `is_none()`
guard M2a.1 already removed from this exact path for being too costly is a
fraction of a generation check.

## Why the gate is structurally unsatisfiable here (the real finding)

The §1.5 gate measures regression **against today's append-only `Vec`**.
Generational IDs exist to make **deletion** safe — to reject a handle to an
entity that was removed and whose slot was reused. **The current Ir never
deletes** (grep-verified: no `remove`/`swap_remove`/`drain`/`clear` on
`decl_nodes`/`nodes`/`edges`/`arcs`; no removal API at all). So the generation
check guards a hazard that cannot occur, at a cost the append-only hot path
cannot absorb.

The two are structurally incompatible: **you cannot add deletion-safety for free
to a structure that never deletes.** Generational arenas become justified only
when a deletion-capable mutable IR is a real requirement — and at that point the
honest baseline is a deletion-capable structure (which already pays for
tombstones or compaction), against which the generation check is cheap, not the
append-only `Vec`. Forcing the migration now would spend a permanent +56 %
lookup tax to protect against a hazard the codebase does not yet have.

## What is preserved / ready for the future

- **Wire form is designed, validated, and frozen** as known-answer tests
  (`gen_arena_contracts.rs`): 8 B `GenId`, 8 B `Option`, version/generation/slot
  layout, legacy-index → genesis decode, typed rejection of gen-0 / future
  version / overflow. When a mutable IR arrives, the encoding is settled.
- **`Id::new`/`raw` remain the single seam** (M2c sealing); the migration surface
  is unchanged and greppable.
- **The canonical hash is already slot/generation-independent** (reads content,
  not raw ID values), so that invariant is free whenever the migration happens.

## §1.2 invariant checklist

| invariant | status |
|---|---|
| Id = 8 B | reachable (validated) |
| Option<Id> = 8 B (niche) | reachable (validated) |
| deletion doesn't renumber; freed slot reusable; old id stale; O(1) stale | all reps satisfy |
| cross-domain compile error | preserved (phantom tag; separate `GenId<T>`) |
| explicit u64 FFI; no unsafe; no unchecked narrowing | design satisfies |
| generation wrap ⇒ retire slot (no revive) | design satisfies (saturating gen 0xFFFFFF) |
| canonical hash / semantics slot-&-generation-independent | already true |
| compact dispatch stays 8 B/decl, no generation copied in | preserved (untouched) |
| **append build / valid lookup / 0–10 % tombstone iteration ≤ 3 %** | **FAIL — migration gate not met** |

## Files (study only — no production source edit)

- `docs/plans/2026-08-31-generational-arenas/{plan.tex,plan.tikz,plan.mmd,id-domain-inventory.md,proto_arena.rs,wire_form.rs,baseline-measured.txt}`
- `hymeko_core/tests/basic/gen_arena_contracts.rs` (+ `mod` line) — frozen wire-form contracts, 6 tests, pass.
- **CORE.YAML items touched: none.** No dependency change. `plan.pdf` not built (no LaTeX toolchain on this host).

## Follow-ups

- Revisit when a mutable/deletion-capable IR is specified; re-baseline the gate
  against that structure, not the append-only `Vec`.
- If handle-safety is wanted *sooner* without the lookup tax, a debug-only
  generation check (validated at explicit boundaries, not on every `get`) is an
  option — but it does not give O(1)-on-every-lookup stale detection, so it does
  not satisfy §1.2 as written.

## Provenance

Branch `feat/core-generational-arenas` @ base `67771c9`; rustc 1.97.1
(8bab26f4f 2026-07-14), x86_64-pc-windows-msvc, release. Prototypes seeded
(xorshift `0x2545f4914f6cdd1d`). Diagnostic Instant-based measurements; the +56 %
floor vs a 3 % gate is far outside any measurement-noise band, so a criterion
confirmation would not change the verdict.
