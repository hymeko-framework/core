# Canonical CBOR codec — implementation, validation, and measurement

Created-at 2026-09-04 14:02 CEST

Branch: `feat/canonical-cbor-codec`

Base/merge recovery: `d6be4ea77990af9fab3e6d086d8323835671d339`

Validated implementation tree: `6c8dbaebb54847345825e380d534970931d1ef8d`

## Summary

HyMeKo Core now owns a production CBOR facade for canonical compiled programs:

- `encode_compiled` turns a `CompiledProgram` plus its interner into explicit
  schema-v1, portable CBOR;
- `encode` is the lower-level checked entry point;
- `decode` preflights hostile CBOR before Serde allocation, reconstructs and
  validates all semantic companions, and returns only `DecodedCborPayload`;
- paths have one workspace-relative `/` wire spelling;
- derived index and canonical hash fields remain in schema v1 for compatibility,
  but are rebuilt/recomputed and compared rather than trusted;
- the canonical incidence hash now streams the exact legacy byte order into
  BLAKE3, eliminating the per-incidence temporary without changing schema,
  semantic order, public API, or IR layout.

The result is ready for a byte-transparent Chordex/Zenoh transport integration.
Transport and simulator/XR wiring are deliberately outside this change.

The implementation closes `UIR-CBOR-01..03`; `UIR-CBOR-04/05` remain passing
characterizations with their identified trust/portability risks enforced. The
traceability matrix now has 77 PASS, 3 PARTIAL, 3 FAIL, 9 NOT_IMPLEMENTED,
17 BLOCKED, 1 FIXED, and 1 PASS_WITH_REGRESSION across 111 rows.

## Authorization and CORE.YAML

Core production files changed under the two user-supplied approvals:

```text
APPROVED-CORE-EDIT: canonical-cbor-codec
APPROVED-CORE-EDIT: canonical-hash-streaming-incidence
```

`CORE.YAML` itself was not changed. The first token covers the writer facade,
validation, direct `ciborium` dependency and tests. The second covers only the
bit-identical internal incidence-hash streaming repair. Frozen v1 KATs and the
105-fixture corpus digest were rollback gates.

## Contracts and failure boundary

Encode requires non-empty portable paths, a coherent IR/index/interner, valid
IDs and canonical-hash v1 position counts. Decode guarantees that message,
payload, string, container and nesting limits are checked before deserialization;
indefinite-length CBOR, truncation, trailing bytes, unsupported/missing schema,
absolute/drive/parent/backslash/NUL paths, invalid IDs or symbols, incoherent
hierarchy/namespaces, missing alias targets, unrebuildable indexes and hash
mismatches fail closed.

A successful decode returns mutually consistent IR, reconstructed interner,
rebuilt index, relative root/import paths and canonical hash. It does not publish
partially validated data. Arc/ref positions beyond canonical-hash v1's `u16`
domain are rejected on both encode and decode.

## Correctness evidence

| Layer | Result | Duration / scope |
|---|---:|---|
| CBOR module unit tests | 14 passed, 0 failed | schema, paths, limits, truncation, indefinite/trailing input, IDs, aliases, index, hash, v1 position bounds |
| CBOR integration tests | 3 passed, 0 failed; 4 measurement helpers ignored by default | all 105 lowerable corpus programs; deterministic small/medium/large; corpus fault injection |
| frozen canonical-hash tests | 7 passed, 0 failed | clean hash-only worktree; all known-answer vectors unchanged |
| frozen corpus digest | passed | `916e27c92ddc4d6b46d36c94d4532d52ad5dd3db2b134a0eb805699ceaffd3c8` unchanged |
| `hymeko_core` library | 134 passed, 1 ignored | 0.13 s |
| `hymeko_core` integration | 243 passed, 7 ignored | 60.11 s |
| `hymeko_wgpu` library | 52 passed | 0.76 s |
| WGPU integration suites | 96 passed | 11.59 s across activation/arena/profiling/readback/session/skeleton/spmm |
| doctests | 27 passed | 0.24 s test execution (compile overhead reported separately by Cargo) |
| full workspace aggregate | **552 passed, 8 ignored, 0 failed** | **76.837 s**, clean detached validation worktree |
| `cargo fmt --all -- --check` | pass | staged implementation tree |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass | staged implementation tree |

The full workspace rerun was repeated after an unexpected host shutdown. Its
tree hash exactly matched the pre-shutdown staged implementation tree, proving
the restart did not alter the tested code.

The shared primary worktree also contains unrelated, unstaged `CLAUDE.md` and
WebXR edits from another workstream. They were excluded from the validation tree,
commit scope, and these results.

## Corpus witnesses

| Scale | Fixture | Declarations | CBOR bytes | SHA-256 |
|---|---|---:|---:|---|
| small | `data/minimal_examples/minimal_example.hymeko` | 1 | 584 | `50217CF5008A02058B5FA5E345E40AC1632F5F25E859C29E087771E9944D99D5` |
| medium | `data/robotics/galambos/galambos_env.hymeko` | 45 | 10,578 | `BF2715CDED508188B18AC1F6B7855B8553DCBD756570506A78DA3826F9EE71B3` |
| large | `data/robotics/robots/quadruped.hymeko` | 409 | 99,181 | `606D6F6739CC52012BE8F0091915A89944900C9514C31E8CD862CF71D648D903` |

No random seed or system entropy is used. Workload selection is deterministic
from the sorted frozen corpus.

## Criterion latency

Criterion 0.8.2; release mode; 30 samples after warm-up; 5 s measurement window
per cell. The machine was checked for competing Cargo/Rust/CBOR processes before
the run. Values are per iteration in microseconds; IQR is Q3−Q1.

| Operation | Scale | Materialized median / IQR / worst | Streaming median / IQR / worst | Median change |
|---|---|---:|---:|---:|
| encode | small | 44.885 / 6.767 / 62.281 | 31.568 / 4.729 / 42.443 | **−29.67%** |
| encode | medium | 156.035 / 28.733 / 206.893 | 100.578 / 9.098 / 122.501 | **−35.54%** |
| encode | large | 873.480 / 249.811 / 1,655.827 | 629.591 / 52.736 / 777.753 | **−27.92%** |
| decode | small | 6.825 / 0.634 / 13.111 | 5.719 / 0.099 / 6.314 | **−16.21%** |
| decode | medium | 141.196 / 14.650 / 244.433 | 120.162 / 3.875 / 136.671 | **−14.90%** |
| decode | large | 1,297.893 / 123.951 / 1,743.357 | 1,088.568 / 21.601 / 1,176.524 | **−16.13%** |

The latency change is favorable in every measured cell. It is a same-host A/B,
not a promise of absolute latency on Kato or consumer machines.

Numerical source:
`reports/data/2026-09-04-canonical-cbor-codec-benchmark.tsv`. Plotted source:
`reports/figures/2026-09-04-canonical-cbor-codec.svg`.

## Peak working set and the failed first design

The production-scale memory fixture has 1,024 nodes, 4,096 edges, arity 256 and
1,048,576 incidences. Its unchanged CBOR image is 51,422,046 bytes. Peak working
set was sampled from a fresh release test process using Windows
`Process.PeakWorkingSet64` at 5 ms intervals.

| Hash path | Baseline peak | Materialized peak | Decode peak | Validation/hash increment | Decode increment | Decoded IR heap | Increment / IR heap | Gate |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| materialized | 56.5 MB | 151.9 MB | 303.2 MB | 151.3 MB | 246.6 MB | 94.2 MB | **2.6175×** | **FAIL** |
| streaming | 56.6 MB | 151.9 MB | 155.9 MB | 4.0 MB | 99.4 MB | 94.2 MB | **1.0545×** | **PASS** |

The plan gate was decode increment `< 2 ×` decoded IR heap and absolute peak
below 16 GiB. The inherited materializing hash failed. Streaming the same ordered
bytes reduced the validation/hash increment by 147.3 MB and passed at 1.0545×;
absolute peak was 155.9 MB, well below the 16 GiB cap. The cap was observed, not
kernel-enforced, because this Windows host has no cgroup RSS limiter.

The post-change decoder successfully consumed the exact pre-change 51,422,046-byte
fixture. That, the v1 KATs and frozen corpus digest jointly establish wire/hash
identity rather than merely semantic similarity.

Numerical source: `reports/data/2026-09-04-canonical-cbor-codec-memory.tsv`.

## CPU profile

Samply 0.13.1 captured five debug-symbol release decode runs of the same
1,048,576-incidence fixture (0.67–0.70 s each) at 1,000 Hz. Artifact:
`reports/data/2026-09-04-canonical-cbor-codec-profile.json.gz`, 200,120 bytes,
SHA-256 `57FDAD851BF97D45D41B62EA6F69CD6F407F90CBD1CA04FE60BBF873285B136F`.

The compact Firefox profile retains PDB references and resolves them when loaded
with the captured build tree; it does not embed a portable symbol table. No
hot-symbol percentage is claimed from the JSON alone. The earlier materialized
version was not profiled before optimization, which is a protocol gap recorded
here rather than reconstructed after the fact. The discriminating
baseline/materialize/decode process cells isolate the removed allocation more
directly, but do not replace that missing pre-change CPU profile.

## Performance decision

All declared acceptance budgets pass after the streaming repair:

- encode and decode medians do not regress on any small/medium/large cell;
- decode peak increment is 1.0545× decoded IR heap, below the 2× plan gate;
- absolute peak is 155.9 MB, below 16 GiB;
- canonical hash bytes and corpus digest are unchanged.

A Kato rerun is useful before publishing cross-machine absolute throughput, but
is not a correctness or merge blocker.

## Dependencies

Added one direct production dependency: `ciborium = "=0.2.2"`. Its normal tree is
`ciborium 0.2.2 -> ciborium-io 0.2.2 + ciborium-ll 0.2.2 -> half 2.7.1`.
`Cargo.lock` already contained the ecosystem transitively through Criterion, so
the lock change only records direct package ownership. No dependency was removed.

## Code health, duplication, and suppressions

The implementation extends the existing `writers::cbor_writer` owner and the
existing unified-IR validation harness; it adds no second codec, IR, interner,
index, limits vocabulary or transport. The canonical-hash legacy materializer is
retained only as a total-behavior fallback for malformed/out-of-v1-domain inputs;
valid v1 inputs take the streaming path.

Clone-delta: **NOT MEASURABLE** — `jscpd` and
`.duplication-baseline.json` are absent on this checkout, while
`rust-code-analysis`, `cargo machete`, and `cargo udeps` are also unavailable.
No zero-delta claim is made. Manual ownership inspection plus warning-denying
workspace Clippy found no second implementation or dead public entry point.

No `allow`, lint suppression, unsafe block, or panic-based public error path was
added. Coverage is not claimed because `cargo llvm-cov` is not installed.

## Files touched

Only the following files belong to this task. Counts are Git added/removed lines;
the profile is binary.

| File | + | − |
|---|---:|---:|
| `Cargo.lock` | 1 | 0 |
| `docs/plans/2026-09-04-canonical-cbor-codec/plan.mmd` | 27 | 0 |
| `docs/plans/2026-09-04-canonical-cbor-codec/plan.tex` | 165 | 0 |
| `docs/plans/2026-09-04-canonical-cbor-codec/plan.tikz` | 42 | 0 |
| `docs/unified-ir-architecture.md` | 14 | 0 |
| `docs/verification/unified-ir/requirements.md` | 20 | 13 |
| `docs/verification/unified-ir/traceability.csv` | 5 | 5 |
| `hymeko_core/Cargo.toml` | 6 | 0 |
| `hymeko_core/benches/cbor_bench.rs` | 101 | 0 |
| `hymeko_core/src/ir/canonical_hash.rs` | 81 | 14 |
| `hymeko_core/src/writers/cbor_writer.rs` | 244 | 5 |
| `hymeko_core/src/writers/cbor_writer/preflight.rs` | 174 | 0 |
| `hymeko_core/src/writers/cbor_writer/tests.rs` | 294 | 0 |
| `hymeko_core/src/writers/cbor_writer/validate.rs` | 304 | 0 |
| `hymeko_core/tests/unified_ir/validation/cbor_roundtrip.rs` | 378 | 0 |
| `hymeko_core/tests/unified_ir/validation/mod.rs` | 1 | 0 |
| `reports/2026-09-04-canonical-cbor-codec.md` | 296 | 0 |
| `reports/data/2026-09-04-canonical-cbor-codec-benchmark.tsv` | 13 | 0 |
| `reports/data/2026-09-04-canonical-cbor-codec-memory.tsv` | 3 | 0 |
| `reports/data/2026-09-04-canonical-cbor-codec-profile.json.gz` | binary | binary |
| `reports/figures/2026-09-04-canonical-cbor-codec.svg` | 61 | 0 |

Explicitly excluded concurrent dirty files: `CLAUDE.md`,
`data/webxr/meta_spatial.hymeko`, `data/webxr/maxwell_fiducial_demo.hymeko`,
`hymeko_core/src/webxr/mod.rs`, `hymeko_core/src/webxr/query.rs`, and
`hymeko_core/src/webxr/query/`.

## Discovery

```text
DISCOVERY LEDGER — canonical CBOR implementation report and measurement artifacts
  find:    `rg --files reports | rg -i "cbor|codec|canonical"` -> 2 hits: `reports/2026-08-30-m2c-seal-canonical-ids.md`, `reports/2026-08-28-phase10a-ir-stream-wire-codec.md`
  grep:    `rg -n -i "CBOR|UIR-CBOR|serialization" docs/verification/unified-ir/requirements.md docs/verification/unified-ir/traceability.csv docs/unified-ir-architecture.md` -> 15 hits: `docs/verification/unified-ir/requirements.md`, `docs/verification/unified-ir/traceability.csv`, `docs/unified-ir-architecture.md`
  symbols: `Test-Path SYMBOLS.md` -> 0 hits: none
  read:    `reports/2026-08-30-unified-ir-platform-conformance.md`, `reports/2026-09-01-incidence-navigation-corrective.md`, `docs/verification/unified-ir/requirements.md`, `docs/verification/unified-ir/traceability.csv`, `docs/unified-ir-architecture.md`
  verdict: NEW `reports/2026-09-04-canonical-cbor-codec.md`, `reports/data/2026-09-04-canonical-cbor-codec-benchmark.tsv`, `reports/data/2026-09-04-canonical-cbor-codec-memory.tsv`, `reports/figures/2026-09-04-canonical-cbor-codec.svg`; rejected `reports/2026-08-30-unified-ir-platform-conformance.md` because it is the immutable pre-implementation audit baseline, not the implementation acceptance record.
```

```text
DISCOVERY LEDGER — canonical CBOR CPU profile artifact
  find:    `rg --files reports | rg -i "profile|flame"` -> 0 hits: none
  grep:    `rg -l -i "samply|profile.json.gz|flamegraph" reports` -> 1 hit: `reports/2026-08-24-cpu-optimization.md`
  symbols: `Test-Path SYMBOLS.md` -> 0 hits: none
  read:    `reports/2026-08-24-cpu-optimization.md`
  verdict: NEW `reports/data/2026-09-04-canonical-cbor-codec-profile.json.gz`; rejected `reports/2026-08-24-cpu-optimization.md` because it documents a different workload and contains no reusable profile artifact.
```

## Environment and provenance

- OS: Windows 11 Home 10.0.26200 build 26200.
- CPU: Intel Core Ultra 9 275HX, 24 logical cores.
- RAM: 63.4 GiB.
- GPU: NVIDIA RTX 5080 Laptop 16 GiB, unused.
- Rust: `rustc 1.97.1 (8bab26f4f 2026-07-14)`, LLVM 22.1.6,
  `x86_64-pc-windows-msvc`.
- Cargo: 1.97.1 (`c980f4866`, 2026-06-30).
- Criterion: 0.8.2. Samply: 0.13.1.
- Source identity: base `d6be4ea`; tested staged tree
  `6c8dbaebb54847345825e380d534970931d1ef8d` in clean detached commit
  `de6545b1a69af1bded2b22b94ab1b24b67d439ac`.
- Benchmark baseline limitation: the pre-streaming CBOR facade was an uncommitted
  intermediate state; its Criterion samples are preserved numerically in the TSV,
  but it has no standalone Git commit. The frozen input hashes and unchanged wire
  fixture make the A/B inputs auditable.
- In-flight experiments: none.

## Waivers and open follow-ups

- No plan PDF: `pdflatex`, `lualatex`, `xelatex`, `tectonic`, `latexmk` and
  MiKTeX are absent per `tools.yaml`. The standalone `.tex` and `.tikz` sources
  are committed.
- No rendered Mermaid: `mmdc`/Node is absent; `plan.mmd` is committed as source.
- No allocation trace: `dhat` is not wired and adding it is a separate core
  dependency event. Peak working set supplies the approved Windows memory gate.
- No coverage delta: `cargo llvm-cov` is absent; no coverage claim is made.
- No clone/dead-code tool delta: the declared tools and baseline are absent, as
  recorded above.
- No animated artifact: the experiment is serialization/CPU/memory behavior,
  with no spatial, temporal-control or policy behavior to animate.
- Samply artifact has no portable embedded symbol table; no hot-symbol rank is
  claimed. A pre-change CPU profile was missed and is explicitly not backfilled.
- Follow-up: wire the simulator/Zenoh receiver directly to Core `decode()` after
  this branch is published; keep Zenoh/Chordex byte-transparent.
- Follow-up: run the same TSV-producing benchmark on Kato before externalizing
  absolute throughput numbers.

## Acceptance decision

**Accept and continue building on it.** The codec has a single Core owner, a
bounded fail-closed input boundary, portable paths, reconstructed derived state,
full frozen-corpus coverage, unchanged canonical identity, passing workspace
gates and production-scale memory below budget. The remaining items are consumer
wiring and cross-host characterization, not defects in the codec contract.
