# PY0 — HyMeKo Python PoW-0 (`hymeko_python`)

**STRATEGY-ID:** HYMEKO-PLATFORM-2026-09-01-v1 · **ROADMAP-TASK:** PY0 ·
**APPROVED-CORE-EDIT:** add-hymeko-python-workspace-crate · **APPROVED-DEPENDENCY-EDIT:**
pyo3-python-packaging. 2026-09-02 CEDT. Branch `feat/hymeko-python-pow0` off core `main`
`791939b`; **not merged**.

A usable, tested `hymeko_python` crate exposes the **real** parser + `hymeko_core`
canonical `Ir` to Python as `import hymeko` — read-only value views, no second IR.

## 1. Commit SHA & remote verification
Recorded in the return summary (branch tip pushed + `ls-remote`-verified). No merge to
`main`. Core `main` `791939b` untouched.

## 2. Python / Rust / toolchain versions
rustc 1.97.1 (x86_64-pc-windows-msvc), edition 2024. **pyo3 0.23.5** (abi3-py312;
offline-cached in the crates.io registry — *not* the framework vendor dir, which the
worktree does not use). **maturin 1.15.0**, **CPython 3.12.13**, **pytest 9.1.1** — all
**project-local via `uv`** (managed dir; no global Python/toolchain install). One
`abi3` wheel: `hymeko-0.1.0-cp312-abi3-win_amd64.whl`.

## 3. Dependency & lockfile diff
`Cargo.toml` (root): `hymeko_python` added to `[workspace] members`. `hymeko_python/
Cargo.toml`: `pyo3 = { version = "0.23", default-features=false, features=["abi3-py312",
"macros"] }` (+ `extension-module` feature), `hymeko_core` (path), `parser` (workspace).
`Cargo.lock`: **additions only** — `hymeko_python`, `pyo3`, `pyo3-build-config`,
`pyo3-ffi`, `pyo3-macros`, `pyo3-macros-backend`, `heck`, `indoc`, `memoffset`,
`target-lexicon`, `unindent`. **No existing pin moved, nothing removed** (audited:
`git diff Cargo.lock` has zero `-name`/`-version` lines). No `hymeko_core`/`parser`
version change.

## 4. Ownership
`Model` owns `Arc<ModelInner> = { Interner (moved out of the store via mem::take),
Arc<CompiledProgram> {Ir, Index, canon_hash}, DeclId→path }`. The `ModuleStore` is
dropped after build; `Ir`+`Interner`+`Index` are kept (names need the interner; canonical
paths + hash need the index). Entities/ids hold `Arc<ModelInner>` → a view keeps its
model alive. Dependency edges: `hymeko_python → {parser, hymeko_core}` only; no
`hymeko_core → PyO3`. See `docs/python-architecture.md`.

## 5. Public Python API
`hymeko.parse(source, filename)`, `hymeko.load(path)` → `Model`. `Model`:
`declaration_count/node_count/edge_count/hyperarc_count`, `canonical_hash()`,
`declaration/node/edge/hyperarc(id)`, `declarations()/nodes()/edges()/hyperarcs()`,
`parent_of/children_of(decl)`, `incidences_of_arc(arc)`, `incidences_of_node(node)`,
`incidence_arrays()`. Entities (`Declaration/Node/Edge/HyperArc/Incidence/BaseRef`) are
frozen read-only views exposing name/canonical-path, hierarchy, `base`/`isa`, target,
slot, `+/−/~` sign, weights, annotation, value. Types + exceptions typed in `hymeko.pyi`.

## 6. Parser → Ir proof
The pipeline is the production `ModuleStore::compile` (RealParser → resolve+const+imports
→ lower → `Ir` → coherence), **not** a test builder — verified by the in-memory,
from-file, and import-fixture tests, and by the Rust oracle driving the same
`ModuleStore` path. Both `parse` (MemProvider) and `load` (StdFsProvider, imports
resolved) are exercised.

## 7. Canonical hash & entity parity
`model.canonical_hash()` == the core `canonical_program_hash` **bit-for-bit**: the Rust
oracle `tests/hash_oracle.rs` recomputes the hash for 9 fixtures via the core
(independently of the binding's precomputed value, and asserts the direct recompute ==
`compile`'s `canon_hash`) and freezes the hex; the Python suite asserts the **same** hex
for the same fixtures via `hymeko.load` — parity holds for all 9 (e.g.
`minimal_example_base_elements` → `ee3db3cb…`, `fano_graph` → `b88715ed…`). Entity counts
equal `len(declarations()/nodes()/edges()/hyperarcs())`; signs `[+1,-1,0]`, duplicate
incidences, weights `[0.85]`, and `isa` bases all read back correctly.

## 8. ID-safety limits
Cross-model id → `HyMeKoIdError` (token); wrong domain → `TypeError` (distinct types);
out-of-range → `HyMeKoIdError` (defensive; not reachable from Python — ids are only minted
by the model, never mutated, no public constructor). Raw `usize` is **not** a public
contract. **Not** stable across pickle/snapshot/transaction — not promised. Generational
migration = `(gen, index, token)` behind the same opaque type, no surface change.

## 9. Tests & coverage
- **Rust**: `fmt` clean; `clippy --all-targets -D warnings` clean; lib unit **2/2**
  (stage-classifier, scalar-weight policy), `hash_oracle` **1/1** (9-fixture parity +
  direct-vs-compile hash equality), `perf_timing` **1/1** (also the 1e5 production-scale
  smoke).
- **Python** (stdlib `unittest`, run from the **installed wheel** in a fresh venv, and
  via `maturin develop`): **32 passed, 13 subtests** — import; in-memory/file/import
  parse; syntax/resolution/io errors; counts; **hash parity vs the Rust oracle**;
  hierarchy; node/edge/arc read; `+/−/~`; weights; annotations/values; duplicate
  incidence; canonical path; empty graph; Unicode string value; cross-model + wrong-domain
  id; id equality/hash as dict keys; deterministic iteration; model lifetime via entities;
  batch arrays == scalar path; golden typical_graphs (fano/sunflower/k4/generic);
  generated 1e3 / 1e4 / **1e5**-incidence sources.
- **Coverage**: `cargo llvm-cov` not installed on this host → **UNMEASURED** (no estimate,
  per contract); correctness rests on the bit-exact oracle + the 32-case Python suite +
  the Rust unit tests, which drive every public entry.

Fixture note (honest): no `webxr` / `service` / `VLAN` / `Steiner` / `Mycielski` fixtures
exist under `data/` (which is CORE.YAML-locked and read-only), so the golden set uses the
present `typical_graphs` (fano, sunflower Δ-system, k4 3-uniform, generic hypergraph) plus
the import fixture; large-scale sources are a deterministic windowed emitter in
test-support (a trivial text generator, not a competing generator framework — the core's
combinatorial generators emit index-designs, not source, and are unreachable from Python).

## 10. Scalar / batch benchmark (release, 24-core host)
| size | nnz | parse ms | hash ms | scalar lookup | 1e3 loop ms | batch export ms |
|---|--:|--:|--:|--:|--:|--:|
| small | 1 000 | 0.96 | 0.0016 | 300 ns | 0.047 | 0.005 |
| medium | 10 000 | 9.9 | 0.0016 | 300 ns | 0.225 | 0.033 |
| **1e5** | 500 000 | 696 | 0.0018 | 300 ns | 0.223 | **9.02** |

`import hymeko` (cold): **1.5 ms**. **Rust-only baseline** (`perf_timing`, release, 5·10⁵
incidences): compile 668 ms, coherence 16 ms, hash 150 ms, SoA build 6.45 ms. **Python
`parse` == Rust compile+coherence** (696 vs 684 ms → GIL-released build has ~0 overhead).
**Batch overhead = 9.02 / 6.45 = 1.40×** — that 40% is the single bulk copy (~10.5 MB) of
the SoA into an immutable `PyBytes` (option B), fully decomposed; > 25% but explained per
the mission's allowance, and 16× cheaper than the per-incidence-object path (option A).
Scalar lookup (300 ns) is the Python↔Rust boundary cost — informative; batch is the
intended bulk path.

## 11. Memory & allocation
Peak process RSS at 5·10⁵ incidences: **353 MiB** (≪ 16 GiB cap). Python heap
(`tracemalloc` peak): **25.5 MiB** — the bulk of retained memory is the native canonical
`Ir`, not Python objects. Batch path allocates **no per-incidence Python object** (five
bulk `PyBytes` + lazy `memoryview`s).

## 12. Wheel / import check
Wheel contents audited: `hymeko/__init__.py`, `hymeko/hymeko.pyd`, `hymeko.pyi`,
`py.typed`, `dist-info/{METADATA,WHEEL,RECORD,sboms/…}` — no absolute local paths, no
venv/build artifacts. Installed into a **fresh** venv and imported from site-packages
(not the source tree); the full Python suite passes from the installed wheel. Debug +
release extension builds both succeed. `.gitignore` excludes `target`, `.venv*`, `dist`,
`*.pyd`/`*.dll`/`*.whl`, `__pycache__` — `git add --dry-run` confirms **zero** artifacts
staged.

## 13. Own mistakes / retracted hypotheses
- **Batch "optimization" retracted (§6.5#18).** I switched the SoA build to five
  `PyBytes::new_with` passes assuming the `Vec→PyBytes` copy was the cost; measured it
  *slower* (10.3 vs 9.0 ms) — the bottleneck is the 5·10⁵-struct traversal, not the copy.
  Reverted to the single pass; documented.
- **Wrong HyMeKo syntax** in the first smoke (`node a {}`); the binding correctly raised
  `HyMeKoSyntaxError`. Corrected against real fixtures (`Desc{}` header, `A;`, `@E {
  (+A,-B); }`).
- **Debug-vs-release bench confusion**: an interim bench ran the `maturin develop` debug
  build (parse 3.4 s); re-run on the release wheel for the reported numbers.
- Two initial Python test bugs (compared `int` to a `DeclId`; asserted declaration tags
  on a fixture that has none) — fixed; the binding was correct in both.

## 14. Next step
Mutable transaction API; the canonical incidence/star view (`prepare_incidence_index` /
`incident_arcs` / O(1) `star_operator`) once `feat/core-canonical-incidence-backbone`
promotes; optional NumPy/WGPU integration (zero-copy arrays, GPU operators). PEP-561 stub
placement inside the package is a minor packaging refinement.

## Provenance / HARD STOP
rustc 1.97.1, x86_64-pc-windows-msvc, 24 cores. Branch `feat/hymeko-python-pow0` off core
`main` `791939b`; pushed + remote-verified (tip in the return summary); **not merged**.
No `hymeko_core/**` edit. **HARD STOP after the return report.**
