# Unified IR — M2b V&V corrective

**Written:** 2026-08-29 16:10 CEDT · **Authorisation:** `APPROVED-CORE-EDIT: unified-ir-m2b-vv-corrective`
**Base:** `14dae837fb456b6323f5bf4175fefb51ac943798` (M2b, clean) · **Scope:** this repository only

M2b shipped four defects. This phase fixes them and, where a number was published, retracts it
additively rather than editing it away. The network dependency bump was **not** started.

| # | What was wrong in M2b | Fixed in |
|---|---|---|
| 1 | Malformed IR was silently accepted by two consumers | §A |
| 2 | The V&V taxonomy counted characterizations as satisfied requirements | §B |
| 3 | Corpus validation skipped-and-passed when the corpus was absent | §C |
| 4 | M2c's memory arithmetic was wrong and a codegen claim was unmeasured | §D |

---

## A. Malformed IR is now observable

The accessor layer stays exactly as it was: **total, `Option`-valued, never panicking**. That is the
right contract for a read API. The defect was never in the accessor — it was that its two strict
consumers treated `None` as "nothing more to do" and finished normally.

Four consumer classes, now explicit (also recorded in `docs/unified-ir-dynamic-test-plan.md`):

| Class | On absence | Instances |
|---|---|---|
| total reader | returns `None` | `ir::access` |
| diagnostic | emits a visible marker | `pretty_print` — `(arc_rec missing)`, **kept** |
| strict | typed error | `IrWriter::write_all`, `HyperGraphView::try_from_ir` |
| compatibility shim | documented fail-fast | `HyperGraphView::from_ir` |

**Silence is not one of the classes.**

### `IrWriter`

`write_all`'s signature is unchanged: `pub fn write_all<W: Write>(&self, w: &mut W) -> io::Result<()>`.
Three malformed shapes now yield `io::ErrorKind::InvalidData` instead of a quietly truncated document:

* a declaration named by a hierarchy link that does not exist;
* a dangling child or sibling link;
* a cyclic sibling chain — bounded by `decl_count()`, so it terminates rather than hanging.

This is **not** a second coherence validator. It checks exactly the links it is about to follow, which
is the minimum a serialiser needs in order to report what it could not write. Structural diagnosis of
*why* an `Ir` is malformed remains `ir::coherence`'s job.

One correction made inside the phase: the first implementation pre-checked the child link in
`write_children` *and* resolved it in `write_decl`. Coverage then showed `write_decl`'s error path was
dead. The pre-check was removed — one check, no dead code, at the cost of a message naming the child
rather than the parent.

### `HyperGraphView`

New typed error and checked constructor:

```rust
pub enum ViewBuildError {
    DanglingArc { edge: EdgeId, arc: HyperArcId },
    DanglingTarget { arc: HyperArcId, target: DeclId },
    MissingNodeMapping { target: DeclId },
}
pub fn try_from_ir<E>(ir: &Ir, cfg: &AggCfg, ex: &E) -> Result<Self, ViewBuildError>
```

`from_ir` keeps its signature, delegates to `try_from_ir`, and fails fast with a documented
`# Panics`. The error carries ids only, never caller-supplied text, so a malformed `Ir` cannot make
the message echo a large payload.

Absence of a node mapping has **three** causes and only two are errors:

| Cause | Verdict |
|---|---|
| target is an `Edge`/`HyperArc` declaration | legitimate edge-to-edge reference — skipped, as always |
| target is a `Node` declaration with no record | `MissingNodeMapping` |
| target is in no declaration table at all | `DanglingTarget`, **but only when there is a declaration table to ask** |

The last qualification is load-bearing, and it was found by a self-inflicted regression. The first
implementation used a bare `!contains_decl(target)` and broke three pre-existing tests. The cause was
not a fixture bug in those tests: `HyperSpec::ir_with_weights` populates `nodes`, `edges`, `arcs` and
`decl_to_node` but leaves `decl_nodes` empty, and `from_ir` has **never** read `decl_nodes`. The
strict check invented a requirement the function never had. Narrowed to `decl_count() > 0`; the
partial fixture is recorded as `UIR-DEFECT-04`, not repaired here.

### Hot-path cost — found and removed

The first strict implementation cost real time on **valid** input:

* `ir.arc(aid).ok_or(ViewBuildError::DanglingArc { .. })?` — `ok_or` takes its argument by value, so
  the error struct was built on every successful iteration.
* `if ir.as_node(target).is_none() { classify }` followed by `if let Some(nid) = ir.as_node(target)` —
  **two** `as_node` lookups per reference on the success path.

This is the same shape M2a.1 already diagnosed once (a redundant `is_none()` guard costing four
instructions). Both were replaced by a single `let … else`, so classification runs only on absence and
the error is constructed only on the malformed path. Measured effect in §F.

### Tests added

| Consumer | Tests |
|---|---|
| `IrWriter` | dangling root declaration · dangling child · dangling sibling · sibling cycle · well-formed `Ir` byte-unchanged |
| `HyperGraphView` | dangling arc · dangling target · `Node` declaration with no record · edge-to-edge reference accepted · `from_ir` still fails fast (`#[should_panic]`) · `try_from_ir` ≡ `from_ir` on the 96-fixture corpus · `Display`/`Error` impls · declaration-less `Ir` accepted |

`ViewBuildError` variants are asserted exactly, not by string matching. `HyperGraphView` is not
`Debug`, so the error tests use `let Err(e) = … else { panic!(…) }` rather than `expect_err`.

---

## B. V&V taxonomy corrected

Every traceability row now declares its **kind**, because a rate that mixes kinds is not a rate of
anything:

| Kind | Rows | Counts toward satisfaction? |
|---|---|---|
| NORMATIVE_REQUIREMENT | 49 | yes |
| DESIGN_CONSTRAINT | 5 | no — tracked separately |
| CHARACTERIZATION | 5 | **no** |
| KNOWN_DEFECT | 4 | **no** — counted as defects |

Statuses: `PASS` · `FAIL` · `PARTIAL` · `NOT_IMPLEMENTED` · `BLOCKED`. `NOT_IMPLEMENTED` means the
subject does not exist yet; `BLOCKED` means it cannot even be expressed with what exists —
`UIR-DYN-04` (stale-ID aliasing) is BLOCKED because dense ordinal IDs *cannot* express it.

### The `bases` gap is no longer a satisfied requirement

`UIR-HASH-03` read *"bases is covered by no current identity system"* with status `VERIFIED`. That is a
characterization of a **gap**, and marking it VERIFIED put a defect in the numerator of the
satisfaction rate. A reader scanning for green rows would have concluded inheritance was protected.
It is now split:

| Row | Kind | Status |
|---|---|---|
| `UIR-IDENT-01` — semantic snapshot identity covers inheritance (`bases`) | NORMATIVE_REQUIREMENT | **NOT_IMPLEMENTED** |
| `UIR-DEFECT-01` — no current identity system covers `bases` | KNOWN_DEFECT | **FAIL** |

`hash_coverage_matrix` remains the evidence for both. It always was evidence of a gap; only the label
was wrong. `UIR-IDENT-02` was added on the same pattern for annotations, values and weights.

### The two silent-skip requirements were rewritten

`UIR-M2B-03` and `UIR-M2B-06` asserted that malformed input was *survived*. Surviving and silently
truncating are the same observable, so those rows certified the very defect §A fixes. They now
require: no UB, never silently accepted, **typed error** on the strict API, **documented fail-fast**
on the compatibility API.

### Corrected rates

| Measure | Definition | Value |
|---|---|---|
| Traceability / evidence coverage | rows citing ≥ 1 running test ÷ all rows | **41 / 63 = 65.1 %** |
| Implemented normative satisfaction | normative PASS ÷ normative rows whose subject exists | **31 / 31 = 100 %** |
| End-state normative satisfaction | normative PASS ÷ all normative rows | **31 / 49 = 63.3 %** |
| Known defects | rows of kind KNOWN_DEFECT | **4** |

**Retraction.** The previously published **96.9 %** ("implemented") and **70.5 %** ("end-state") are
withdrawn. Their denominator was `VERIFIED + PARTIAL` over a set that mixed requirements with
characterizations, and their numerator counted the `bases` gap as satisfied. The original text stays
in place in `requirements.md` and `coverage-baseline.md`, with the correction appended below it.

**Read the 100 % correctly.** It says every normative requirement *whose subject exists today* is met.
It does not say the system is complete: 18 normative requirements are NOT_IMPLEMENTED or BLOCKED, and
4 known defects stand. The bad news lives in the defect count, which is exactly why it is reported as
its own number instead of being folded into a percentage.

---

## C. Corpus validation is fail-closed

Two changes in `tests/unified_ir/support/corpus.rs`:

1. The root is resolved from `env!("CARGO_MANIFEST_DIR")`, not from a relative path against the
   process working directory. A relative `../data` happens to work under `cargo test` and resolves
   somewhere else under any other runner — which would silently turn every corpus test into a no-op.
2. `require_fixtures()` **fails** when the root is missing, is not a directory, is unreadable, or
   holds zero `.hymeko` files. The previous form printed `skipped` and returned, so a validation suite
   that validated nothing still reported green — precisely the failure mode a validation suite exists
   to prevent.

CWD-independence was verified by running the suite from the parent directory; all five corpus tests
passed. `fixture_files()` / `fixture_count()` remain available for genuinely optional diagnostic
probes, which must then label themselves diagnostic and stay out of the normative validation rate.

---

## D. M2c corrections

Appended to `reports/2026-08-29-unified-ir-m2c-readiness.md` as a *Correction* section rather than
edited in place.

**C1 — the arithmetic was wrong.** `Option<Id>` goes 16 B → 8 B, so the three maps go
`3 × 20 001 × 16 = 960 048 B` → `3 × 20 001 × 8 = 480 024 B`. That is **2×**, not 4×, and the
published "~240 KB" figure was wrong. The **80 004 B** figure (`20 001 × 4`) belongs to the
architecture document's *different* plan — removing the three maps entirely and inlining a `u32`
discriminator. That is a separate optimization, not the generational-ID niche result, and the two must
not be quoted as one number.

**C2 — claim strengths separated.** For packed `NonZeroU64` vs `{ idx: u32, gen: NonZeroU32 }`:

| Claim | Strength |
|---|---|
| the two are size-equivalent (8 B, both niche-optimised inside `Option`) | **MEASURED** |
| field readability / FFI clarity favours the struct form | **STRUCTURAL** |
| the struct form decodes faster at runtime | **UNMEASURED HYPOTHESIS** |

Calling the packed form measurably slower without codegen or a benchmark was not warranted. That A/B
is M2d's job.

---

## E. Coverage

Same tool (`cargo-llvm-cov 0.9.0`), same command, same denominator as the M2b baseline:

```
cargo llvm-cov --workspace --all-features --lcov \
  --output-path reports/coverage/after-corrective.lcov \
  -- --skip test_load_meta_geometry --skip test_coo_construction_throughput
```

Artifact: `reports/coverage/after-corrective.lcov`.

| Scope | baseline | M2b | corrective |
|---|---|---|---|
| workspace production lines | 83.10 % | 84.56 % | **84.72 %** |
| workspace production fns | 80.64 % | 82.16 % | **82.28 %** |
| `hymeko_core/src/ir/` | 90.84 % | 90.84 % | **90.84 %** |
| `writers/ir_writer.rs` | 0.00 % | 97.50 % | **100.00 %** |
| `traversal/hypergraphview.rs` | 90.00 % | 90.00 % | **94.85 %** |
| `util/pretty_print.rs` | 0.00 % | 89.86 % | 89.86 % |
| `ir/access.rs` | 94.64 % | 94.64 % | 94.64 % |

Branch coverage is **`n/a`, not 0 %** — `cargo llvm-cov --branch` is nightly-only and this host is
stable 1.97.1. No estimate was substituted.

**Touched-line coverage is 100 %.** The nine lines still uncovered in `hypergraphview.rs` —
`node_edges`, `edge_nodes`, and the `par_sort_unstable_by_key` branch above the 8192-element threshold
— are all **pre-existing** and untouched by this phase.

Two tests were added purely to close touched-line gaps that coverage exposed, and both turned out to
assert something real:

* `view_build_error_display_names_the_offending_ids` — the three `Display` arms and the
  `std::error::Error` impl, none of which the `Debug`-matching tests reached.
* `try_from_ir_accepts_an_ir_with_no_declaration_table` — the "no oracle to ask" skip arm, i.e. the
  exact narrowing that fixed the self-inflicted regression in §A. Reaching it needed a reference whose
  target is in neither `decl_to_node` nor the declaration table, so the test was extended rather than
  duplicated.

The timed COO throughput test's coverage skip is unchanged and remains acceptable — it is an
instrumented performance assertion whose thresholds do not hold under `-C instrument-coverage`. It
**runs and passes** in the normal full gate below.

---

## F. Gates

Host: Intel Core Ultra 9 275HX · Windows 11 Home 10.0.26200 · 63.4 GB RAM ·
`rustc 1.97.1 (8bab26f4f 2026-07-14)` stable · Criterion 0.8.2. `tasklist` showed zero
`rustc`/`cargo`/`python` processes before the benchmark window.

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-features --all-targets -- -D warnings` | pass, 0 diagnostics |
| `cargo check --workspace --all-features` | pass |
| `cargo build --workspace --all-features` | pass |
| `cargo test --workspace --all-features --no-fail-fast` | **513 passed, 1 failed** |
| doctests | 5 targets, all ok |
| focused unified-IR (`-p hymeko_core --test mod unified_ir`) | 85 passed |
| focused writer | 7 passed |
| focused `HyperGraphView` / `try_from_ir` | 16 passed |
| coverage | §E |
| valid-IR `HyperGraphView` Criterion regression | below |

The single failure is `domain_transformations::parse_files::test_parse_files::test_load_meta_geometry`
— the known **57/58** fixture drift, unchanged from M2b and deliberately left visible.

### Performance

Nine valid-IR cells across two Criterion groups (`view_construction_from_ir`,
`view_from_ir_weight_path`), **six alternating A/B rounds** (A = base `14dae83`, D = corrective),
per-round medians read from `estimates.json`, paired in log space.

| Comparison | n | mean | 95 % CI |
|---|---|---|---|
| corrective vs base | 54 | **+1.50 %** | [+0.28 %, +2.75 %] |
| **A/A null** — the identical binary against itself | 27 | **−1.71 %** | [−3.33 %, −0.06 %] |

**The null is the important row.** Comparing the *same* binary against itself with the same estimator
produces a systematic −1.71 % with a per-pair sd of **4.41 %**, and per-cell null deltas ranging from
−8.64 % to +3.07 %. This host has round-to-round drift larger than the 3 % gate itself. Therefore:

* **Measured:** the corrective build is +1.50 % [+0.28, +2.75] slower on the paired estimator.
* **Inferred:** correcting for the half-cycle drift that favours the later (D) run puts the point
  estimate near +2.4 %.
* **Not established:** that the difference is nonzero at all. It sits inside the noise the null
  demonstrates.

**Verdict: under the 3 % gate on both the measured and the drift-corrected estimate, with the stated
caveat that this host cannot resolve a 3 % per-cell effect.** A tighter figure needs the quiet second
host, which is outside this phase's scope. This is reported rather than rounded away because the first
strict implementation *did* carry a real cost (§A); the number above is what remains after removing
it, not a number that was never checked.

An earlier three-round measurement of the pre-fix implementation gave a geomean of +1.84 % with a
worst cell at +6.00 %. That measurement is what prompted looking at the generated work per element,
which is how the double `as_node` lookup was found. It is recorded here because it did diagnostic
work, not as a published baseline.

Other performance properties, established by inspection of the diff:

* **No new allocation on the successful hot path.** `Vec::with_capacity(arc_count * 2)` is unchanged;
  the eager `ok_or` is gone; `ViewBuildError` is a `Copy` POD with no heap.
* **Typed error construction only on the malformed path** — every construction site sits inside a
  diverging `else` or `match` arm.
* **Output, hash, order and floating-point results unchanged.** `try_from_ir` adds validation, not
  different arithmetic or ordering; `sort_reduce_pairs` and both CSR builders are untouched; the
  writer's output for a valid `Ir` is byte-identical, asserted by
  `writer_accepts_a_well_formed_ir_unchanged` and by the 96-fixture corpus equivalence test.

---

## G. Files touched

| File | + | − |
|---|---|---|
| `hymeko_core/src/writers/ir_writer.rs` | 63 | 12 |
| `hymeko_hnn/src/traversal/hypergraphview.rs` | 150 | 21 |
| `hymeko_core/tests/unified_ir/support/corpus.rs` | 81 | 12 |
| `hymeko_core/tests/unified_ir/validation/consumer_equivalence.rs` | 83 | 36 |
| `hymeko_core/tests/unified_ir/validation/derived_views.rs` | 221 | 24 |
| `hymeko_core/tests/unified_ir/validation/corpus.rs` | 5 | 25 |
| `docs/verification/unified-ir/traceability.csv` | 64 | 57 |
| `docs/verification/unified-ir/requirements.md` | 59 | 0 |
| `docs/verification/unified-ir/coverage-baseline.md` | (correction appended) | 0 |
| `docs/unified-ir-dynamic-test-plan.md` | 36 | 0 |
| `reports/2026-08-29-unified-ir-m2c-readiness.md` | 43 | 0 |
| `reports/coverage/after-corrective.lcov` | new artifact | — |

**`CORE.YAML` items touched:** exactly one — `hymeko_core/src/writers/ir_writer.rs`, under
`APPROVED-CORE-EDIT: unified-ir-m2b-vv-corrective`. `hymeko_core` is `lockdown: full`.

The other two touched trees are **not** core, which was checked rather than assumed:
`hymeko_hnn` is declared `lockdown: none` ("deliberately non-core: this is where HyperGraphView,
message passing, and the HGNN / Signed-HGNN convolutions evolve"), and `**/tests/**` is on the
CORE.YAML allowlist. `hymeko_hnn`'s stated acceptance gate — the regression tests in
`hymeko_core/tests/` — is the 513-test run above.

**New / removed dependencies:** none. No manifest, lockfile or vendor directory was modified.

**§6.5 anti-patterns:** none introduced. No new Cartesian function variants — `try_from_ir` is the
checked entry point and `from_ir` delegates to it, rather than being a parallel implementation. No
`_v2` files, no global state, no string-typed config. The corpus helper *removes* a fourth copy of the
same directory walk.

**Error-handling waivers:** none added. `corpus.rs` uses `expect`/`panic!` in **test** code, where a
failed precondition must abort the test.

---

## Open issues

1. **`UIR-DEFECT-04`** — `HyperSpec::ir_with_weights` builds an `Ir` with an empty `decl_nodes`; the
   coherence validator would reject it. Recorded, not repaired: repairing it inside this phase would
   have changed nine benchmark fixtures in the middle of a measurement.
2. **`UIR-DEFECT-01..03`** — the identity gaps (`bases`, annotations, values/weights) stand. M4.
3. **`pretty_print.rs` at 89.86 %** — the `(arc_rec missing)` marker is still unreached because
   `CompiledProgram` cannot be constructed from a test. Unchanged from M2b; adding `Clone`/`Default`
   to production types solely to make a test compile would be the test dictating the design.
4. **Performance resolution** — the 3 % gate is finer than this host's per-cell noise floor. A
   definitive figure needs the quiet second host.
5. **Known 57/58 fixture drift** — `test_load_meta_geometry`, unchanged and deliberately visible.

## Not started, per the brief

Network dependency / lockfile bump · M2c sealing · stable-ID implementation · transaction and
mutation API.
