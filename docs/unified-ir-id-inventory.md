# Unified IR — ID-use inventory (Phase 0, Part D)

**Date:** 2026-08-28 · design/measurement only · companion to `reports/2026-08-28-unified-ir-phase0-audit.md`

Every use of `DeclId`, `NodeId`, `EdgeId`, `HyperArcId` and `SymId`, classified by category. Categories are reported **separately** because they migrate differently: a `raw()` call is a one-line fix, a dense index is a semantic hazard, and an FFI site is a published contract.

---

## 1. Method and its limits

Scope: `.rs` files that actually reference one of the five ID types, excluding `target/` and the `phase3-backup-20260825/` mirror (a backup tree, not a live consumer).

```bash
# The file set every count below is taken over
grep -rl "DeclId\|NodeId\|EdgeId\|HyperArcId\|SymId" <repo> --include=*.rs | grep -v "/target/"

# Per-category, over that file set
xargs grep -oh "\[[a-z_]*\(id\|_id\|key\|d\)\?\.0\]"        # dense index
xargs grep -oh "\b[a-z_]*\.0\b"                              # raw field access
xargs grep -oh "\.raw()"                                     # accessor
xargs grep -oh "NONE\|is_none()\|is_some()"                  # sentinel / absence
xargs grep -lh "Serialize\|Deserialize\|serde"               # serialization (files)
xargs grep -oh "pyo3\|#\[pyfunction\|#\[pymethods\|wasm_bindgen\|extern \"C\""   # FFI
xargs grep -oh "BTreeMap\|BTreeSet\|\.sort\|Ord\b\|cmp("     # order-related
xargs grep -oh "\(DeclId\|NodeId\|…\)::new([0-9]"            # literal IDs
```

**Two honest limits.**

1. **The "order" column is an upper bound, not a measurement.** It counts order-related constructs *in files that reference IDs* — it does not prove the ordering is applied *to* an ID. `graph`'s 459 is dominated by ordinary graph-algorithm containers. Only manual triage can convert this column into a true count; it is reported as a **triage queue**, not a result.
2. These counts supersede an earlier looser pass that did not restrict to ID-referencing files. The ratios sharpened; no conclusion changed.

---

## 2. Inventory

| Repository | files | dense index | `.0` access | `raw()` | `NONE`/absence | serde files | FFI | order (triage) | literal IDs |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `hymeko_core` | 36 | **52** | 253 | 5 | 52 | 10 | 0 | 37 | 28 |
| `hymeko_hnn` | 6 | 6 | 28 | 0 | 0 | 0 | 0 | 3 | 3 |
| **`transformation`** | 20 | **127** | 250 | 24 | 100 | 1 | 0 | 23 | 26 |
| `graph` | 23 | 7 | 137 | 1 | 21 | 1 | 0 | 459 † | 0 |
| `hymeko_python` | 1 | 0 | 4 | 10 | 1 | 1 | **23** | 33 | 0 |
| `tensor` | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 4 | 0 |
| `vscode-plugin` | 4 | 0 | 14 | 16 | 11 | 0 | 0 | 10 | 0 |
| `network` | 1 | 4 | 4 | 1 | 1 | 0 | 0 | 6 | 0 |
| `hotaru` | 3 | 0 | 3 | 3 | 4 | 0 | 0 | 5 | 0 |
| **`parser`** | **0** | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| **`proofs`** | **0** | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| **Total** | **95** | **196** | **693** | **60** | **190** | **13** | **23** | 580 † | **57** |

† triage queue, not a count — see §1.

### Persistence and DTO exposure

| Repository | persistence sites | ID-shaped DTO fields |
|---|---:|---:|
| `hymeko_core` | 4 (`writers/ir_writer.rs`) | 0 |
| `transformation` | 3 | **3** (`snapshot::{NodeDto,ArcDto}`) |
| `hymeko_python` | 2 | **8** |
| `vscode-plugin` | 0 | 2 |
| `graph` | 1 (PGIP I/O) | 0 |

---

## 3. What each category means for the migration

| Category | Total | Migration character | Risk |
|---|---:|---|---|
| **Dense index** `x[id.0]` | **196** | Each site assumes *ID ≡ array offset*. Under stable keys this is wrong. | **Highest.** Silent under D4 — compiles, indexes the wrong element. |
| **`.0` access** | 693 | Mostly formatting, comparison, and passing through. Mechanical. | Low individually; the volume is the cost. |
| **`raw()`** | 60 | Already the *sanctioned* accessor. These are the sites that were written correctly. | Low — they become the template. |
| **`NONE`/absence** | 190 | Sentinel checks. Migrate with D6 (niche). Behaviour-preserving if the niche is introduced correctly. | Medium — a missed site silently treats `NONE` as a valid index. |
| **Serialization** | 13 files | D1: no type tag. Wire-format change. | **High** — versioned coordination required. |
| **Persistence** | 10 | On-disk artifacts embed raw indices. Old files must still load. | **High** — needs a format version, not just a code change. |
| **FFI** | 23 | Published pyo3 ABI. Python code holds IDs across calls. | **High** — external contract. |
| **Order** | 580 (triage) | D5: `Ord` is allocation order. Any *semantic* reliance breaks under stable keys. | Unknown until triaged. |
| **Literal IDs** | 57 | `NodeId::new(3)` in tests/fixtures. Break loudly. | Low — loud is good. |

---

## 4. The three categories that decide the plan

**Dense index (196).** The migration's real work. 127 of them — **65%** — are in `transformation`, more than core's own 52. See `docs/unified-ir-consumer-map.md` §3.

**FFI + serialization + persistence (46 sites, 13 files).** These are *contracts*, not code. They cannot be fixed by a refactor because the other side is a browser, a Python package, or a file already on disk. Each needs a version negotiation. `hymeko_formats::snapshot` emits `target_id: usize` into JSON with **no format version field** — that is the sharpest instance.

**Order (580, untriaged).** The largest number and the least understood. `Ord` on `Id<T>` is allocation order (D5); under generational keys it becomes meaningless. Most of the 580 are ordinary containers, but *some* subset encodes "iterate declarations in ID order" and will silently change traversal order — and traversal order feeds `canonical_program_hash`. **Triaging this column is a prerequisite for M3**, not for M0 or M2.

---

## 5. Verification note

`parser` and `proofs` referencing **zero** ID types was checked directly, not assumed. It is the migration's one clean edge: the parser produces an AST and never speaks `Id<T>`, so lowering is the single contract to hold stable. Protect it deliberately — do not let a convenience import create an edge that does not exist today.
