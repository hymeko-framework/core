# TensorCoo AoS Test Suite

**File:** `hymeko_core/tests/test_tensor_representations/test_coo_aos.rs`
**Module registration:** Add `mod test_coo_aos;` to `test_tensor_representations/mod.rs`
**Run command:** `cargo test -p hymeko_core test_tensor_coo_aos -- --nocapture`

---

## Shared Test Fixture

All correctness tests use a single known tensor built by `make_known_coo()`:

```
Shape: 3 slices × 4 rows × 4 columns
6 entries, pushed in this exact order:

  Index | k | i | j |  v
  ------|---|---|---|-----
    0   | 0 | 0 | 1 | 1.0
    1   | 0 | 1 | 0 | 1.0
    2   | 1 | 2 | 3 | 2.5
    3   | 1 | 3 | 2 | 2.5
    4   | 2 | 0 | 3 | 0.5
    5   | 2 | 3 | 0 | 0.5
```

This produces three dense slices:

```
Slice 0:          Slice 1:          Slice 2:
  0 1 0 0          0 0 0 0           0 0 0 0.5
  1 0 0 0          0 0 0 0           0 0 0 0
  0 0 0 0          0 0 0 2.5         0 0 0 0
  0 0 0 0          0 0 2.5 0         0.5 0 0 0
```

The same entries are defined as a constant `KNOWN_ENTRIES` array for assertion comparisons throughout the suite.

---

## Correctness Tests (9 tests)

### `test_coo_empty`

**What it verifies:** A freshly constructed tensor has `len() == 0`, `is_empty() == true`, and metadata (`num_slices`, `dim_i`, `dim_j`) is preserved from the constructor.

**Why it matters:** Ensures the AoS `entries: Vec::new()` initialization behaves identically to the old four-Vec default.

---

### `test_coo_push_and_len`

**What it verifies:** After one push, `len() == 1` and `is_empty() == false`. After two pushes, `len() == 2`.

**Why it matters:** The most basic contract — `push` increments the count by exactly 1. In the old layout, `len()` read from `self.v.len()`. In the new layout, it reads from `self.entries.len()`. This test catches any mismatch.

---

### `test_coo_entry_accessor`

**What it verifies:** `t.entry(idx)` returns a `CooEntry` with `k`, `i`, `j`, `v` exactly matching the pushed values, for all 6 entries in the known fixture.

**Why it matters:** This is the primary read path. Every consumer that used to do `coo.k[t]` / `coo.i[t]` / `coo.j[t]` / `coo.v[t]` now uses `coo.entry(t)`. If the struct field layout is wrong, this test catches it.

---

### `test_coo_iter`

**What it verifies:** Collecting `t.iter().map(|e| (e.k, e.i, e.j, e.v))` produces a vector identical to `KNOWN_ENTRIES`.

**Why it matters:** The iterator is the main read path for `dense_view_slice`, `project_sum_over_slices`, all `tensor_convert` functions, and `util::print_dense_block`. If the iterator skips entries, reorders them, or returns garbage, this test catches it.

---

### `test_coo_ordering_preserved`

**What it verifies:** Entries pushed in deliberately non-sorted order `(i=99, i=0, i=50)` come back in exactly that order via `entry()`.

**Why it matters:** The COO format does not sort entries. The star expansion and clique expansion rely on push order matching the traversal order of the IR. If the AoS layout or the iterator accidentally sorts or reorders, this test catches it.

---

### `test_coo_reserve_does_not_change_len`

**What it verifies:** Calling `reserve(10_000)` on an empty tensor keeps `len() == 0` and `is_empty() == true`. A subsequent push brings `len()` to 1.

**Why it matters:** `reserve()` changed from four `Vec::reserve` calls to one. This test verifies the new version doesn't accidentally push default entries or change the length.

---

### `test_coo_into_soa_roundtrip`

**What it verifies:** `into_soa()` on the known fixture produces a `CooSoa` with:
- `num_slices == 3`, `dim_i == 4`, `dim_j == 4` (metadata preserved)
- `k`, `i`, `j`, `v` arrays each with length 6
- Each array element matches the corresponding `KNOWN_ENTRIES` value

**Why it matters:** `into_soa()` is the new method that didn't exist before. It's the FFI boundary — the Python API calls it to produce separate Arrow arrays. If it transposes incorrectly (e.g., swapping k and i, or dropping the last entry), the PyTorch tensors will be wrong.

---

### `test_coo_into_soa_empty`

**What it verifies:** `into_soa()` on an empty tensor produces empty arrays with metadata preserved.

**Why it matters:** Edge case. An IR with no arcs produces an empty COO. The Python API must not crash.

---

### `test_coo_f64_precision`

**What it verifies:** A high-precision f64 value (`π × 10¹⁵`) survives `push` → `entry()` → `into_soa()` with exact bit equality.

**Why it matters:** The `CooEntry<F>` struct is generic over `F: Real`. This test verifies that `f64` entries don't get truncated to `f32` anywhere in the pipeline. Catches accidental `as f32` casts.

---

## Integration Tests (3 tests)

### `test_coo_dense_view_slice_correctness`

**What it verifies:** `dense_view_slice(&t, k)` for each of the 3 slices in the known fixture produces the exact expected dense matrix. Checks both non-zero positions and verifies zero positions are actually zero.

**Why it matters:** This is the end-to-end integration test for the read path. `dense_view_slice` uses `coo.iter()` internally. If the AoS layout is wrong, the dense matrices will be wrong, and this test catches it with specific coordinate assertions.

---

### `test_coo_project_sum_over_slices`

**What it verifies:** Summing all slices produces the correct aggregate matrix. Checks 5 specific coordinates and 1 zero coordinate.

**Why it matters:** `project_sum_over_slices` iterates all entries regardless of `k`. This tests that the full iteration path is correct and that values from different slices don't interfere.

---

### `test_coo_duplicate_entries_coalesce_in_dense`

**What it verifies:** Three pushes to the same `(k=0, i=1, j=2)` with values `1.0, 0.5, 0.25` produce `m[1][2] == 1.75` in the dense view. Also verifies the COO itself has 3 entries (not coalesced — COO stores raw entries, dense view sums duplicates).

**Why it matters:** Duplicate coordinates are a normal occurrence in clique expansion (multiple hyperedges can produce the same `(i,j)` pair). The dense view must sum them correctly. This test also verifies that the AoS layout doesn't accidentally deduplicate during push.

---

## Performance Tests (3 tests)

These are not microbenchmarks (those belong in criterion). They are **regression guards** with generous time bounds that will fail only if something is catastrophically wrong.

### `test_coo_construction_throughput`

**What it does:**
1. Creates a tensor with `reserve(5_000_000)`
2. Pushes 5M entries in a tight loop
3. Measures wall time
4. Asserts < 500ms (generous — expect ~50–100ms)
5. Reports M entries/sec to log

**What it catches:** If `push()` accidentally does 4 capacity checks instead of 1 (regression to SoA), or if the `CooEntry` struct has unexpected padding causing cache misses, this will show up as degraded throughput.

**Expected output:**
```
Push throughput: ~80-150M entries/sec (~35-65ms for 5M entries)
```

---

### `test_coo_iteration_throughput`

**What it does:**
1. Builds a 5M-entry tensor
2. Iterates all entries, accumulating `sum_k` and `sum_v` to prevent dead code elimination
3. Measures wall time
4. Asserts < 500ms
5. Reports M entries/sec to log

**What it catches:** If the iterator has overhead (e.g., bounds checking on each entry, or if the `#[repr(C)]` layout causes alignment issues), this will show degraded throughput. The AoS layout should give sequential memory access, which the prefetcher handles well.

**Expected output:**
```
Iteration throughput: ~200-500M entries/sec (~10-25ms for 5M entries)
```

---

### `test_coo_construction_without_reserve`

**What it does:**
1. Creates a tensor with **no** `reserve()` call
2. Pushes 1M entries, forcing amortized reallocation
3. Measures wall time
4. Asserts < 200ms
5. Reports M entries/sec to log

**What it catches:** This is the test that most directly measures the AoS improvement. The old 4-Vec layout would trigger 4 independent reallocation chains (each Vec doubles independently at different times). The new single-Vec layout triggers 1 reallocation chain. On 1M entries, the old layout would call `realloc` roughly `4 × log₂(1M) ≈ 80` times. The new layout calls it roughly `log₂(1M) ≈ 20` times. Each realloc also copies less data in aggregate because it's one contiguous block instead of four.

**Expected output:**
```
Push without reserve: ~50-100M entries/sec (~10-20ms for 1M entries)
```

---

## What is NOT tested here

- **Star/clique expansion correctness** — covered by the existing `tensor_fano.rs` tests and `test_tensor_representation.rs`, which were updated to use `entry()` / `iter()` in the AoS migration.
- **Python FFI** — requires the compiled PyO3 extension. Tested at the `hymeko_py` integration level, not here.
- **Criterion benchmarks** — these tests report throughput to the log but use `assert!` with generous bounds, not statistical analysis. For proper A/B comparison with the old layout, a criterion benchmark comparing `push_4vec` vs `push_aos` on identical workloads would be the right tool.