# P2.1 — canonical incidence backbone & O(1) star view

**STRATEGY-ID:** HYMEKO-PLATFORM-2026-09-01-v1 · **ROADMAP-TASK:** P2.1 ·
**APPROVED-CORE-EDIT:** canonical-incidence-backbone · **APPROVED-GRAPH-EDIT:** promote-pgraph-dedup-only.
2026-09-02 15:56 CEDT. Core branch `feat/core-canonical-incidence-backbone` @ base P2 `52c2649`;
**not merged** to core `main` (`5fe2c88`). Graph `main` fast-forward-promoted `9655f07 → b234c48` (P1.3 dedup),
remote-verified.

> **Plan-bundle note.** Per MEMORY `no-latex-toolchain-on-this-host`, the four-format `plan.{tex,pdf,tikz,mmd}`
> bundle is not buildable on this host (no LaTeX). As in the P1.x/P2 phases this session, the design is captured
> in this report and the committed, test-gated diff is the traceable artifact. The implementation was **not**
> back-dated: the representation decision (§ below) drove the code.

---

## 1. Self-referential problem, solved (the core deliverable)

No safe-Rust struct can own an `Ir` **and** an `IncidenceIndex<'borrowing-self>`. The fix separates the
**owned, lifetime-free derived payload** from the **borrowed pairing**:

- **`IncidenceIndexData { row, arcs, slots }`** — the SoA CSR reverse index, **borrows nothing**. All the
  logic (build, resolve, incidences, validate, `Bx`/`Bᵀy`/`BᵀBx`, coefficient precompute) lives here as
  methods that take `ir: &Ir` where they need it.
- **`IncidenceIndex<'ir> { ir: &'ir Ir, data: IncidenceIndexData }`** — the existing borrowed index,
  **public API unchanged**, now a thin forwarder to `data`.
- **`IncidenceView<'a, 'ir> { ir: &'ir Ir, data: &'a IncidenceIndexData }`** — `Copy`, two references, the
  same query+operator surface; created `O(1)` / zero-alloc.
- **`PreparedIr { ir: Ir, data: IncidenceIndexData }`** — the **owned canonical bundle**. `new(ir) ->
  Result<Self, IncidenceError>` builds + bijection-validates the index; `ir()`, `index()`, `into_ir()`;
  `incidences() -> IncidenceView<'_,'_>` and `star_operator() -> StarOperator<'_,'_>`, both `O(1)` zero-alloc.
  Not self-referential, no `unsafe`, no pointer identity; cannot be paired with a foreign `Ir`; cheaply
  `Arc`-shareable (share the whole bundle).
- **`StarOperator<'a,'ir>`** — `Copy` borrowed handle for the implicit `A_star = BᵀB` (`[[0,Bᵀ],[B,0]]`);
  `apply`/`node_to_arc`/`arc_to_node`. **Never** materialises the star adjacency, a clique/2-section, or a
  second oriented copy of the payload — every apply is `O(nnz)`.

## 2. Representation decision — A/B/C (§8 gate)

Measured on the same data + math (200 001-node windowed workload, ~1e6 incidences, release):

| representation | Bᵀy | BᵀBx | retained | view create | verdict |
|---|--:|--:|--:|--:|---|
| **A** borrowed `IncidenceIndex` (SoA, borrows `Ir`) | 13.23 ms | 22.60 ms | 12.8 B/inc | n/a | baseline |
| **B** owned `PreparedIr` + borrowed `IncidenceView` | 13.01 ms | 22.22 ms | 12.8 B/inc | **~0 ns** | **chosen** |
| **C** flat arc-major (arc,slot)/coeff SoA | — | — | +8 B/inc | — | **deferred (no consumer)** |

**Chosen: B.** A and B share the operator code (it lives on `IncidenceIndexData`), so B is a byte-identical,
**no-regression** (1.02× — within noise) drop-in; retained memory is **identical** (B *owns* the same SoA
arrays A *borrows*); view creation is `O(1)`/zero-alloc (a 16-byte reference-pair copy, fully elided).
**C is not forced:** the flat arc-major SoA is the P2 `bx_profile` result — a *second* arc-major coefficient
array buys ~2× on `Bx` but costs **+8 B/incidence** with **no repeated-`Bᵀ` consumer** to justify it, so it
stays a documented opt-in, exactly as the mission's "if flat SoA doesn't win on real code, don't force it"
clause directs. The negative SoA result is reported, not buried.

## 3. Semantic payload vs derived navigation (§2)

The **canonical semantic payload** (target / sign / weight / annotation / hyperarc ownership / slot order) is
stored **exactly once — in the `Ir`**. `IncidenceIndexData` holds **only derived** navigation data (`row`
offsets + `arcs`/`slots` coordinate permutation). It is not a second truth source: private fields,
factory-only construction (`build`), **bijection-validated** (`validate` proves every `(arc,slot)` appears
exactly once in its true target's row — the dense-position marking catches duplicate/missing/swapped
coordinates), **never serialized**, and type/lifetime-bound to its `Ir` through the wrappers. No coefficient
array is added by this phase; the node-major coefficient stays the opt-in `IncidenceOperatorPlan` (8 B/inc).

## 4. Plan migration (§5)

`IncidenceOperatorPlan` now borrows `{ ir: &Ir, data: &IncidenceIndexData, coeff }` instead of
`&IncidenceIndex`, so it is constructible identically from an `IncidenceIndex`, an `IncidenceView`, or a
`PreparedIr` (`prepare_operator()` on index and view). **No new coefficient array**; prepared `Bᵀy`/`BᵀBx`
remain **bit-identical** to the legacy operator and serial==Rayon (node-partitioned, no atomics) — proven in
both the retained P2 suite and the new differential. The "no repeated-`Bᵀ` consumer yet" statement stands.

## 5. Tests (§7) — all green

Shared scaffolding consolidated into `tests/incidence_support.rs` (§6.1): the `node`/`r`/`arc` `Ir`-builders,
`windowed` workload, `dense_bt` oracle, `bits_eq`, `median_ms` — the P2 suite and the new P2.1 suite both
consume it (removed ~105 lines of duplicated helpers from the P2 file).

`tests/prepared_ir_backbone.rs` (new):
- **old-vs-new differential** — for mixed + 0/1/2/63/64/65 + 50 000-node workloads: `incident_arcs`/`slots`,
  `degree`, resolved incidences, `resolve` (incl. weights), and `B`/`Bᵀ`/`BᵀB` are **bit-identical** between
  `IncidenceIndex` and `PreparedIr`'s view; `StarOperator` == `star_operator`; view-plan == index-plan ==
  legacy `Bᵀ`; serial == Rayon.
- **owned-bundle lifecycle** — `new`/`ir`/`index`/`into_ir`; view bijection-validates.
- **index-rebuild parity** — a cloned `Ir` (deserialize stand-in: the wire carries only canonical `Ir`, the
  index is rebuilt on load) reproduces `Bᵀy` bit-for-bit; the data-level `IncidenceIndexData::build` path too.
- **zero-alloc view creation (structural)** — `IncidenceView`/`StarOperator` are `Copy` and exactly two thin
  references (`size_of_val == 2·usize`); a compile-time `assert_copy`; 1e6 creations allocate nothing.
- **wrong-owner fault mutant** — an `IncidenceIndexData` validated against a *foreign* `Ir` fails the
  bijection (runtime guard for the raw-data path; the wrappers make the wrong pairing a compile error).
- **§8 representation bench** — the A/B/C gate table above.

Fault mutants against the in-module `fault_tests` (duplicate-coordinate, target-mismatch, out-of-range slot,
truncated row) all still caught after repointing to `idx.data.*`. **No mutant committed** (git-clean).

| layer | result |
|---|---|
| `mod` integration (incl. P2.1 suite + benches) | **253 passed, 0 failed, 3 ignored** |
| lib unit | **120 passed, 0 failed, 1 ignored** |
| doctests | **5 passed, 0 failed** |

## 6. Dynamic-future compatibility (design only — no transaction/snapshot framework built)

The model already splits along the axis a future dynamic `Ir` needs:

- **Structural vs value snapshot.** Topology (arcs/refs/targets) is *structural*; sign/weight/annotation are
  *value*. `IncidenceIndexData` is a pure function of the **structural** snapshot — a value-only edit (reweight)
  leaves it valid; only a **topological** commit rebuilds it.
- **`Arc`-shared immutable structure.** `PreparedIr` is designed to be wrapped in `Arc` and shared; readers
  hold `Arc<PreparedIr>` and borrow `IncidenceView`s. A topological commit publishes a **new** `PreparedIr`
  (build index, then atomically swap the `Arc`) — the derived index is published **together with** its `Ir`,
  never observably out of sync.
- **Rebuild location.** Index rebuild belongs at the **topological-commit boundary** (`PreparedIr::new`), not
  per query — the existing factory is exactly that seam.
- **Stale-view rejection.** Today a view cannot outlive its `PreparedIr` (compile-time borrow). A future
  mutable model adds a monotone **structure version**; a long-lived handle re-borrows through the current
  `Arc`, so a stale borrow is a compile error, and a stale *version* is a cheap runtime check — no dangling.
- **Wire.** Serde carries **only** the canonical `Ir`; the index is always rebuilt on load (proven by the
  rebuild-parity test). **Stable public ID vs snapshot-local compact coordinate stay separate concepts:**
  `DeclId`/`HyperArcId` are the stable public identity; `row`/`arcs`/`slots` positions are snapshot-local
  compaction that a rebuild may renumber. Nothing persists the compact coordinate.

No transaction, snapshot, or `IRDelta` type was created (out of scope).

## 7. PGRAPH entry API (§9 — design only, migration NOT executed)

The owned bundle removes the blocker Part 1 identified. Executable migration shape:

```rust
pub struct CompiledPGraph {
    prepared_ir: Arc<PreparedIr>,   // owns Ir + derived incidence index; not self-referential
    domain: PGraphDomain,           // classification/costs — NO topology
}
impl CompiledPGraph {
    pub fn view(&self) -> PGraphView<'_> { /* O(1): borrow prepared_ir.incidences() + &domain */ }
}
```

- **No self-referential lifetime** — `Arc<PreparedIr>` owns `Ir`+index; `PGraphView` borrows from it on demand
  (`O(1)`), so `compile_to_lowered` can return an **owned** `CompiledPGraph` (fixing the Part 1 API break where
  a borrowed `LoweredPGraph<'ir>` could not own its `Ir`).
- **`schema.incoming`/`outgoing` removable** — all adjacency comes from the borrowed incidence view; the AND
  closure's `.iter().all(…)` folds become allocation-free `for_each_*` callbacks over `&BTreeSet`/incidences.
- **This phase does not execute the PGRAPH production migration** — it proves the core ownership/representation
  the migration needs, per the entry condition. The graph-side work here was limited to the approved
  `promote-pgraph-dedup-only` fast-forward (P1.3 dedup), already remote-verified.

## 8. V&V (§10)

`cargo fmt --check` clean · `cargo clippy --all-targets -D warnings` clean · `cargo check`/`build` ok · full
`mod`+lib+doctests green (above) · §8 bench smoke green. Coverage: `cargo llvm-cov` is **not installed** on
this host (tools.yaml) → per contract **UNMEASURED, no estimate**; correctness rests on the bit-exact
differential + independent dense oracle + bijection validator + fault mutants, which exercise every new public
function (`IncidenceIndexData`, `IncidenceView`, `PreparedIr`, `StarOperator`, and the migrated plan). Touched
`incidence.rs` public surface is driven by the new + retained suites.

## 9. Files touched

| file | Δ | note |
|---|---|---|
| `hymeko_core/src/ir/incidence.rs` | +≈370 / −≈150 | extract `IncidenceIndexData`; `IncidenceIndex` forwards; add `IncidenceView`/`PreparedIr`/`StarOperator`; migrate plan to `{ir,data,coeff}` |
| `hymeko_core/tests/incidence_support.rs` | **new** ~120 | shared §6.1 scaffolding |
| `hymeko_core/tests/prepared_ir_backbone.rs` | **new** ~260 | P2.1 differential + lifecycle + rebuild + zero-alloc + fault + §8 bench |
| `hymeko_core/tests/incidence_precompute_profile.rs` | −105 | adopt shared scaffolding (no behaviour change) |
| `hymeko_core/tests/mod.rs` | +8 | wire the two new modules |

**CORE.YAML items touched:** `hymeko_core/src/ir/incidence.rs` — authorized by `APPROVED-CORE-EDIT:
canonical-incidence-backbone`. No dependency/toolchain change. No other crate touched.

## 10. Own mistakes / retracted

- The `incidences` forwarder first failed to compile — the borrowed view yields `IncidenceRef<'ir>` for
  lifetime `'a`, which needs `'ir: 'a`; added the bound to `IncidenceView`/`StarOperator`. Caught by `cargo
  check`, fixed before tests.
- A stale struct doc block survived the struct rewrite (the old "bound to the Ir" text prepended to the new
  lifetime-free `IncidenceIndexData` doc — semantically wrong); removed.
- The in-module `fault_tests` inject via private fields (`idx.slots`/`idx.row`); these needed `idx.data.*`
  after the extraction. Caught by `clippy --all-targets`, repointed.

## 11. Acceptance vs the §8 gate

Star view/operator `O(1)` zero-alloc ✓ · payload stored once (in `Ir`) ✓ · no `unsafe` ✓ · exact
bit-identical operators ✓ · no >3% proven regression (1.02×) ✓ · retained memory ≤ current (identical,
12.8 B/inc) ✓ · no wrong pairing (compile-time in wrappers; runtime bijection for raw data) ✓ · corrupt
coordinate fail-closed ✓ · serde wire unchanged (index rebuilt, never carried) ✓.

## Provenance / HARD STOP

rustc 1.97.1 (8bab26f4f 2026-07-14), x86_64-pc-windows-msvc, 24 cores. Core base `52c2649`; branch
`feat/core-canonical-incidence-backbone`, pushed + remote-verified (tip recorded in the return summary),
**not merged** to core `main` `5fe2c88`. Graph `main` `b234c48` (P1.3 dedup FF-promotion, verified). No remote
worktrees/build trees deleted. **HARD STOP after this report.**
