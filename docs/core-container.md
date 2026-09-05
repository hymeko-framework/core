# Core SDK and verification container

The container builds and checks the existing Core workspace on Linux. It contains
the canonical Core sources, fixtures, pinned Rust SDK and locked dependency
sources. Core remains a library with its existing CLI; this adds no server or
alternative IR. CPU Core checks are mandatory. WGPU execution is a separate
hardware check, because its tests can skip when no adapter is available.

Build on Amaterasu itself. The Dockerfile pins the official Rust 1.97.1 Bookworm
linux/amd64 image by digest. The host's Rust installation is not used. The
`.dockerignore` allowlist excludes Git metadata, target directories, host Cargo
configuration, vendor trees and previous reports. Keep Cargo.lock unchanged:
`cargo fetch --locked` selects parser `e28a205147c48273980e63aa3338ca286974a6ed`,
even if its declared branch has advanced.

## Build with bounded resources

Start from a clean clone at the intended revision, or extract `git archive` of
that exact revision on Amaterasu. Record the revision and archive SHA256 when
using an archive. Do not copy a Windows worktree's `.git` pointer to Linux.

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
