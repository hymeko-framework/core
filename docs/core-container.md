# Core SDK and verification container

The container builds and checks the existing Core workspace on Linux. It contains
the canonical Core sources, fixtures, pinned Rust SDK and locked dependency
sources. Core remains a library with its existing CLI; this adds no server or
alternative IR. CPU Core checks are mandatory. WGPU execution is a separate
hardware check, because its tests can skip when no adapter is available.

Build on Amaterasu itself. The Dockerfile pins the official Rust 1.97.1 Bookworm
linux/amd64 image by digest. The host's Rust installation is not used. The
`.dockerignore` allowlist excludes Git metadata, target directories, host Cargo
configuration, vendor trees and previous reports. Build with the selected commit's
Cargo.lock: `cargo fetch --locked` selects parser
`284c490d68232973a006bcba974279fed6f89ce8`, even if its declared branch advances.
This revision refresh was approved as `core-parser-latest-revision`; preserve the
lock during image builds and update it only through a reviewed dependency change.

## Build with bounded resources

Start from a clean clone at the intended revision, or extract `git archive` of
that exact revision on Amaterasu. Record the revision and archive SHA256 when
using an archive. Do not copy a Windows worktree's `.git` pointer to Linux.
For an archive produced on Windows, use `git -c core.autocrlf=false archive`
and verify its file hashes against Git blobs; otherwise checkout conversion
can introduce CRLF even when the stored sources use LF. The recorded final
Core build uses a clean Linux Git clone transferred through a Git bundle.

Create a dedicated builder; these commands do not select or replace the host's
other builders. Choose a unique builder name if this one already exists.

```sh
docker buildx create --name hymeko-core --driver docker-container \
  --driver-opt memory=16g --driver-opt memory-swap=16g \
  --driver-opt cpu-period=100000 --driver-opt cpu-quota=600000
```

The parser is a private SSH Git dependency. Supply an agent that is authorized
to read it and a known-hosts file containing its previously verified host key.
If building over SSH, forward that agent for the build connection with `ssh -A`.
Amaterasu's existing deploy identity may have access to other repositories only;
successful host login does not prove parser access.

From the Core checkout on Amaterasu:

```sh
docker buildx build --builder hymeko-core --progress plain --load \
  --add-host amaterasu.local:192.168.1.228 \
  --ssh default="$SSH_AUTH_SOCK" \
  --secret id=known_hosts,src="$GITEA_KNOWN_HOSTS" \
  --build-arg SOURCE_REVISION="$CORE_REVISION" \
  --metadata-file core-image-build.json \
  -f tools/ci/Dockerfile -t "hymeko-core:$CORE_REVISION" .
```

`CORE_REVISION` is the full checked-out commit hash; `GITEA_KNOWN_HOSTS` names
the trusted host-key file. The host/IP mapping is specific to this installation.
The agent and host-key file are mounted only for dependency fetching. Private
keys are not build arguments, environment variables, copied files or image
layers. No global Git/SSH configuration is rewritten. The independent
`toolchain` build target needs no parser credentials.

## Verify offline

Create a new target volume for the first verification. A later run may reuse
that task-owned Linux volume, but the first clean-build claim must identify
its empty starting state. No source bind mount is required. The source copy
inside the container is writable because existing hash tests produce ignored
CSV files in their working directory.

```sh
docker volume create hymeko-core-target
docker run --name hymeko-core-verify --init \
  --network=none --memory=16g --memory-swap=16g --cpus=6 \
  --mount type=volume,src=hymeko-core-target,dst=/workspace/core/target \
  "hymeko-core:$CORE_REVISION" all
docker inspect hymeko-core-verify
docker logs hymeko-core-verify > core-verification.log 2>&1
```

The entry point rejects a missing/excessive memory limit, enabled swap and
unknown actions. It emits a timestamp at each stage, real cgroup memory usage
every 30 seconds during long commands, and GNU time elapsed/max-RSS plus cgroup
peak at completion. `cgroup_container_peak_bytes` is cumulative over the whole
container lifetime, includes page cache, and is not a stage peak or process RSS.
Retain the container's exit/OOM state alongside logs, then remove only the named
task container if desired.

Available actions:

| Action | Existing checks composed by the entry point |
| --- | --- |
| `smoke` | Core CLI on the minimal fixture; full lowerable-corpus public CBOR roundtrip test |
| `test` | Core release unit, integration and documentation tests, default features disabled |
| `lint` | Workspace rustfmt; workspace/all-targets Clippy with warnings denied |
| `bench-smoke` | Existing three-scale encode/decode Criterion target with `--test` |
| `all` | Toolchain/dependency inventory followed by all four actions |

Benchmark smoke proves the workloads execute; it is not a latency measurement.
Do not use blanket `--ignored`: ignored tests include golden writers and long
diagnostics. Do not prebuild `ir_depth_probe` as a smoke target; its presence
activates an otherwise skipped deep-hierarchy subprocess experiment.
Optional `arrow-schema` and `ipc` are separate feature campaigns; IPC additionally
needs clang/libclang and native headers and is not enabled by this image's checks.

The provider's existing `hymeko-contract-gates` image remains the owner of
symbols/duplication/jscpd checks. Run it separately against a valid Linux Git
checkout; an archive without Git metadata is not a valid `git ls-files` input.
No copy of the provider is shipped here.

## Optional NVIDIA verification

Amaterasu exposes its RTX 2070 SUPER through the installed NVIDIA container
runtime. The image includes `libvulkan1` and `libegl1`: in the recorded control,
the NVIDIA ICD could not create a Vulkan instance until EGL was installed,
although `nvidia-smi` already worked. No host driver change was needed.

```sh
set -eu
docker run --name hymeko-core-gpu --init \
  --network=none --memory=16g --memory-swap=16g --cpus=6 \
  --gpus all -e NVIDIA_DRIVER_CAPABILITIES=compute,utility,graphics \
  -e XDG_RUNTIME_DIR=/tmp \
  --mount type=volume,src=hymeko-core-target,dst=/workspace/core/target \
  --entrypoint sh "hymeko-core:$CORE_REVISION" -ec '
    timeout --kill-after=5s 180s cargo test -p hymeko_wgpu --release --locked --offline -- --nocapture --test-threads=1
    timeout --kill-after=5s 120s cargo bench -p hymeko_wgpu --locked --offline --bench spmm_bench -- spmm_gpu_resident --test
  '
docker logs hymeko-core-gpu > core-gpu.log 2>&1
test "$(docker inspect --format '{{.State.ExitCode}}' hymeko-core-gpu)" = 0
! grep -q SKIPPED core-gpu.log
grep -F 'GPU: NVIDIA GeForce RTX 2070 SUPER [Vulkan] DiscreteGpu' core-gpu.log
```

Require all three checks: container exit zero, no `SKIPPED`, and the actual
discrete adapter identity. Existing GPU tests can return success when no adapter
is available; test totals alone do not establish hardware execution. The
resident benchmark's `--test` mode verifies execution, not consumer throughput.
GPU tests run serially: the default parallel run stalled in `tests/session.rs`
on Amaterasu, while the same nine tests completed serially in 2.26 seconds.
This is a verified execution setting, not a fix for the unresolved parallel
stall. The timeout covers the command including compilation; raise it explicitly
for a fresh GPU target if the recorded compilation baseline requires more time.

## Further integration

After Core verification, use this SDK as the controlled environment for locked
consumer builds. The next stages are dependency/CI integration, real browser
model updates, and a measured consumer workload on Amaterasu. An SDK image does
not itself prove browser state changes or GPU execution. Preserve distinct
source revisions, hardware identity, input fixtures and timing boundaries.

Build behavior follows Docker's [SSH and secret mounts](https://docs.docker.com/build/building/secrets/),
[container resource limits](https://docs.docker.com/engine/containers/resource_constraints/)
and Cargo's [locked dependency fetch](https://doc.rust-lang.org/cargo/commands/cargo-fetch.html).

## Distributed admission and ARM64 portability

The existing `cross-toolchain` target adds the ARM64 Rust standard library,
GNU linker/libc sysroot and static QEMU user interpreter. The default SDK target
is unchanged. Build natively with the existing bounded builder:

```sh
docker buildx build --builder hymeko-core-amaterasu-20260905   --target cross-toolchain --load -t hymeko-core-cross:1.97.1   -f tools/ci/Dockerfile .
```

Use the existing Core source checkout and compatible Cargo/target caches. Run
`sh tools/ci/core.sh distributed-build` in this image with the usual 16-GiB,
no-swap container limits, four Cargo jobs and `CARGO_BUILD_TARGET` unset for
native Linux or `aarch64-unknown-linux-gnu` for ARM64. The action compiles the
existing integration harness and `ir_stream_bench` without attempting to execute
a foreign binary. It emits stage timing/RSS and cgroup progress as other actions do.

Run the ARM64 ELF in an ARM64 Debian runtime with `--platform linux/arm64` and
an explicitly mounted native static QEMU entry point. Both the runtime image
architecture and the executable ELF machine must be checked. This requires
neither privileged Docker nor host binfmt registration. Mount the exact benchmark
and output directory, disable networking, set four CPUs and 16 GiB with no swap.
The runtime image/guest libc and QEMU hashes are part of measurement provenance.

Correctness uses the real `ir_stream_admission` integration tests, including the
Lean-generated reference matrix. Benchmark preflight precedes timing:

```sh
./ir_stream_bench --test '^(ir_stream_admission/|ir_stream_replay/(exact_duplicate|conflicting_duplicate|replay_window_miss)$)'
CRITERION_HOME=/results/run1/criterion RAYON_NUM_THREADS=4   ./ir_stream_bench --bench '^(ir_stream_admission/|ir_stream_replay/(exact_duplicate|conflicting_duplicate|replay_window_miss)$)'
```

Use separate result directories for three process repetitions. The existing
`scripts/dev/criterion_report.ps1 -CriterionRoot <directory> -Baseline new`
reads retained samples and reports median, interpolated quartiles and worst
iteration-batch mean. These are not individual-request tail latencies. Admission
setup constructs a fresh assembler outside the timed closure; validation,
admission and closure-owned assembler destruction are timed. Criterion batching
can retain many assemblers, so peak process memory describes the harness as well
as the operation. Native and QEMU timings must be reported separately; the latter
are not native ARM hardware performance or distributed speedup.

The [recorded campaign](../reports/2026-09-06-distributed-verification-benchmarks.md)
contains exact commands, source/lock/image hashes, raw samples and resource limits.
