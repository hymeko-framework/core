# HyMeKo audit reproduction and Kato manifest

Created: 2026-09-05. All timings are local Windows measurements unless explicitly
labelled historical. No Kato/LAN/browser performance result is claimed.

## Source and environment

Use the local implementation commits listed in `../2026-09-05-platform-audit.md`.
`repositories.json` records original paths, revisions, upstreams and dirty state;
`integration-source-snapshot.json` records source/copy SHA256 equality. The Core
benchmark baseline is `0a34a976`, Graph `b234c487`, Transformation `4298cfca`.
Simulator actually resolves Core-portable-id `93b1f114`, not the benchmark Core.
Graph's locked Core is `70b4f364`; do not replace dependency identities silently.
The source snapshots are local integration evidence, not public clean clones.

`environment.json`, Cargo tree logs and `*.memory.json` preserve hardware,
dependency identities, exact executable SHA256, arguments, working directory,
Rayon thread count and process wall/peak. Benchmarks ran with named dirty task
files before implementation commits; their final diff inventory is recorded in
the report. No protected Core source, manifest or wire contract was changed.

Local host: Windows 11 10.0.26200, Core Ultra 9 275HX, 24 logical CPUs, RAM
68,097,818,624 B; RTX 5080 Laptop 16,303 MiB, Vulkan driver 610.88, cc 12.0.
Rust/Cargo 1.97.1 / LLVM 22.1.6; NVCC 13.2.86 / MSVC 19.51.36252. The CUDA
executable additionally prints its linked runtime/driver API versions (13030).
All sparse backend controls use f32. `RAYON_NUM_THREADS=24` is per measured child.
Rust builds use `--locked --offline`, release/bench; compilation is outside timing.
The initial Simulator dependency fetch was the recorded exception to offline use.

## Build and correctness commands

In the Core, Graph and Transformation roots separately:

```powershell
cargo test --workspace --release --locked --offline --no-fail-fast -j 6
cargo fmt --check
cargo clippy --workspace --all-targets --locked --offline -j 6 -- -D warnings
```

Transformation workspace fmt/Clippy has recorded pre-existing failures; scoped
formatting passes. Its two debug-only panic tests must run in debug. Full tests
can rewrite six generated Gazebo files; inspect and preserve user work, and only
restore task-generated differences in a clean, isolated worktree.

```powershell
# Chordex worktree
cargo test -p chordex-transport --release --features zenoh --locked --offline -- --include-ignored
cargo clippy -p chordex-transport --all-targets --features zenoh --locked --offline -- -D warnings
# Simulator worktree
cargo test --release --locked --offline --no-default-features --features zenoh-ingress --test model_ingress -- --nocapture
cargo check --lib --target wasm32-unknown-unknown --no-default-features --locked --offline -j 6
node --test web/zenoh-ingress.test.mjs
```

Use the actual installed Node 24.19.0 executable if an unconfigured nvm shim is
first on PATH. Simulator whole-library Clippy has pre-existing failures; its
integration-test Clippy completion is not claimed. Chordex whole-workspace tests
also fail in the unchanged musical `Quality::suffix(self)` API.

## Criterion and supervised diagnostics

From own Core, call the existing helper, keeping its returned object:

```powershell
$measurement = & scripts/dev/measure_peak_memory.ps1 -Bench cbor_bench -Filter canonical_cbor -Threads 24 -LogPrefix reports/platform-audit/reproduced-cbor
if ($measurement.exit_code -ne 0) { throw 'Benchmark failed' }
```

The helper returns `exit_code` in its result; it does not set shell `$LASTEXITCODE`
for a .NET-launched child. It polls OS high-water counters every 100 ms and stops
an observed working set above 16,384 MiB. This is not a kernel quota; the final
poll window can miss a peak. A short process's observed peak can underestimate
its transient maximum. `peak_observed=false` is missing evidence, not zero RSS.
Stdout/stderr and the metric JSON are saved before a cap exception is raised.

Run each selected cell in three separate processes. Snapshot only Criterion
JSON after each repetition, retaining `new/{benchmark,sample,estimates}.json`.
The fixed matrix filters are:

| Cargo bench | Filter |
| --- | --- |
| `cbor_bench` | `canonical_cbor` |
| `ir_bench` | `ir_canonical_hash/` |
| `ir_bench` | `ir_construction/` |
| `csr_bench` | `csr_spmm_paths/(serial\|parallel)/nnz16k/fd32` |
| `spmm_bench` | `(spmm_cpu\|spmm_gpu_oneshot\|spmm_gpu_static_csr\|spmm_gpu_resident)/.*/medium/fd32` |
| `spmm_bench` | `spmm_prepare/` |

Final readback control: one of the following per process; rotate order across
three repetitions: `spmm_gpu_resident/E_resident_readback/medium/fd32$`,
`spmm_gpu_resident/E_resident_reused_readback/medium/fd32$`,
`spmm_gpu_resident/E_reused_single_wait/medium/fd32$`.
The benchmark checks the CPU oracle before timed resident cells. Raw files with
`wgpu-before-consolidation-` preserve the earlier control; `wgpu-final-run` holds
the final closure-consolidated code. The old `prepare_warm_cache` accumulated
unsubmitted writes; its time span differs from `prepare_warm_cache_completed`.

For existing diagnostic tests, first build with Cargo `--no-run --message-format=json`.
Select the `compiler-artifact.executable` for the exact target, not an arbitrary
stale executable hash. Then pass that executable and the table's arguments to
`measure_peak_memory.ps1 -Executable ... -ArgumentString ... -Threads 24`.

| Target | Exact executable arguments |
| --- | --- |
| Core test `mod` | `unified_ir::validation::corpus::bench_lowering_corpus --exact --ignored --nocapture` |
| Core test `mod` | `unified_ir::verification::incidence::bench_incidence_operator_vs_clique --exact --ignored --nocapture` |
| Core test `mod` | `unified_ir::validation::cbor_roundtrip::write_canonical_cbor_memory_fixture --exact --ignored --nocapture` |
| Core test `mod` | `unified_ir::validation::cbor_roundtrip::canonical_cbor_memory_read_baseline --exact --ignored --nocapture` |
| Core test `mod` | `unified_ir::validation::cbor_roundtrip::canonical_cbor_memory_materialize_only --exact --ignored --nocapture` |
| Core test `mod` | `unified_ir::validation::cbor_roundtrip::canonical_cbor_memory_decode --exact --ignored --nocapture` |
| Graph `informed_search_bench` | `--nocapture` |
| Graph `direct_ir_bench` | `--ignored --nocapture --test-threads 1` |
| Transformation example `ir_seam_bench` | no arguments |
| Transformation integration test `integration` | `bench_compile_vs_validate --exact --ignored --nocapture` |
| Core WGPU example `phase7_probe` | `--part a` |
| Core WGPU example `phase7_probe` | `--part b` (14 sizes, 3 warmups + 24 steady samples per variant) |
| Simulator test `model_ingress`, native Zenoh feature | `live::canonical_cbor_stage_probe --exact --ignored --nocapture` |

The initial Graph direct survey ran two test functions concurrently and is retained
as exploratory evidence; final comparison uses `graph-direct-serial-run*` only.
Never apply `--ignored` indiscriminately to a whole workspace: some ignored tests
regenerate goldens. The table selects known diagnostic targets.

`graph_counterexamples.rs` and `graph_berge_counterexamples.rs` are external test
executables. Build with `rustc --edition 2024 --test -O`, `-L dependency=<Graph release deps>`,
and the matching `--extern hymeko_graph=<rlib>` / `--extern hymeko=<rlib>` pair from
the same Cargo feature resolution. Run the first normally for the three fixed
counterexamples; its `bounded_diamond_enumeration_probe --exact --ignored --nocapture`
produces 3 warmups + 20 samples for 2/4/8/12/16 diamonds. Each returned path has
2d steps, with analytical 2^d full outputs, 4,096-result and 1,000,000-expansion caps.
The second executable failed both Berge-limit counterexamples before approval and
passes both after Graph commit `94efe35489459bc52ae34a7d8362eea4f91abcad`.
The original failure log is retained; `berge-budget-graph_berge_counterexamples-after.log`
records the successful rerun against the corrected library pair.

The approved continuation compares Graph `25e8c9f2` with `94efe354` using the same
diamond source, output oracle and release dependency resolution. Build the first
external executable against each revision, then run three supervised processes:

```powershell
foreach ($run in 1..3) {
    $measurement = & ./scripts/dev/measure_peak_memory.ps1 `
        -Executable "$PWD/reports/platform-audit/graph_counterexamples.exe" `
        -ArgumentString 'bounded_diamond_enumeration_probe --exact --ignored --nocapture' `
        -Threads 24 -LogPrefix "$PWD/reports/platform-audit/berge-budget-after-run$run"
    if ($measurement.exit_code -ne 0) { throw "Berge run $run failed" }
}
```

Use the `before` prefix for the old revision. Build outside the measurements;
preserve each executable hash from its memory JSON. The five scales retain the
4,096-result cap, analytical path counts, and exact 2d steps per emitted path.
The targeted regression command is
`cargo test -p hymeko_graph --test direct_ir_optional --release --locked --offline`
from the Graph root. It gives 11 pass after the fix; the full workspace gives
486 pass / 0 fail / 6 ignored. Fmt, Clippy and SYMBOLS pass; duplication is
390 clones / 10015 lines against the previous 392 / 10021, using read-only
`contract-duplication --root . --measure` without creating a Graph baseline.

Run `python summarize.py` from the evidence directory to include the six new logs
in the existing tables and `berge-budget-summary.csv`. With the already available
Matplotlib environment, `python summarize.py --berge-budget-plot-only` produces
the separate PNG/SVG comparison without rewriting the historical GPU/transport
figures. The plot shows three process medians and their range, not a confidence
interval. Small cells are sensitive to timer resolution and measured CV; the d4
median rises by 0.20 us (13.3%), while no larger-cell median slowdown is observed
in this bounded, sequential before/after campaign. No general speedup is claimed.

## CUDA and Kato

Windows build from the prototype directory, in an existing VS developer shell:

```powershell
nvcc -O3 -arch=sm_120 -o spmm_prototype.exe spmm_prototype.cu
```

Portable Unix build uses the existing `tools/cuda_prototype/build.sh`:

```sh
NVCC=/usr/local/cuda/bin/nvcc ARCH=sm_75 OUT=spmm_prototype sh tools/cuda_prototype/build.sh
tools/cuda_prototype/spmm_prototype --edge-cases --verify
tools/cuda_prototype/spmm_prototype --nonfinite --verify
tools/cuda_prototype/spmm_prototype --rows 200000 --deg 8 --fd 32 --bench
tools/cuda_prototype/spmm_prototype --rows 200000 --deg 32 --fd 32 --irregular --bench
```

Use the Kato GPU's actual supported architecture, not `sm_75` blindly. Amaterasu
inventory is Ryzen 3700X, RTX 2070 Super cc7.5/8 GiB, CUDA13.0.88, driver595.91.07;
it was used for PDF/figure rendering only, not this CUDA comparison.
Freeze `git rev-parse HEAD`, `git diff --name-only`, lockfile SHA256, `nvcc --version`,
host compiler, `nvidia-smi`, CPU/OS/RAM, flags and executable SHA256 in a separate
Kato result directory. Keep three serial process repetitions, a 16 GiB resident
limit using the site's existing scheduler/cgroup facility, and a live progress log.
Do not compare different GPUs/hosts as percentage regressions.

The prototype prints seed407715774446, f32, 5 warmups, 50 raw kernel-event/host,
D2H and steady input-to-output samples. Cold setup/H2D/first call occurs once per
process. Benchmark implies the CPU numerical oracle. Full byte-level Rust/CUDA
input KAT parity is still a future gate; matching mathematical shapes and seed
alone do not prove identical sparse inputs.

```powershell
compute-sanitizer --tool memcheck --error-exitcode 99 --leak-check full ./spmm_prototype.exe --rows 4099 --cols 4093 --deg 8 --fd 17 --irregular --verify
ncu --section LaunchStats --section Occupancy --kernel-name spmm_kernel --launch-count 1 --csv ./spmm_prototype.exe --rows 32768 --deg 8 --fd 32 --verify
```

Local memcheck: zero errors/zero leaks. Local Nsight: `ERR_NVGPUCTRPERM`; occupancy
is unmeasured. Device permission settings were not changed. Instrumented timings
are excluded from the normal performance tables.

## Data interpretation and figures

Run `python reports/platform-audit/summarize.py` with Python >=3.10. It restores
missing Criterion snapshots from `criterion-raw.zip` after checking extraction
paths remain within the evidence directory, then rebuilds CSV/JSON and fixture
SHA256 metadata. Archive creation checked all 3,022 raw members against the
original file hashes. The 51,422,046-byte
memory fixture lives under ignored `target/`; regenerate it with the table's test.
The three small transport fixtures are retained, totalling less than 1 MiB.

Quantiles use sorted samples and linear interpolation at `(n-1)*q`. Criterion
uses `time/iterations`, converted ns→us; its p95/p99 are batch-average quantiles.
Transport/CUDA/timestamp/Berge use individual raw samples. Existing diagnostic
median-only logs retain their original resolution and do not gain invented tails.
Instability heuristic: run median range/median >10% or within-run CV >10%.

On a host with existing Matplotlib and Pillow, run the same script `--plot-only`
beside `campaign-summary.json` and `transport-run1.stdout.log`. This writes
`campaign.png`, `campaign.svg`, and `transport-trace.gif`. The GIF progressively
reveals real measured request latencies; it is not browser or simulated motion.
The local system has no Matplotlib; Amaterasu's existing Matplotlib3.6.3 rendered
these retained local measurements without installing dependencies.
