# Distributed Core verification and platform benchmarks

Started: 2026-09-06T04:28:31+02:00
Status: in progress on `feat/core-distributed-verification-benchmarks`, base `92383b38`.

Scope: Core admission/replay semantics, formal model and implementation conformance,
then identical Criterion workloads on Windows, native Amaterasu and Docker/QEMU ARM64.
The P5/P6/P7 production transaction/publication/apply roadmap is not claimed implemented
by these admission checks. The unmerged executable-model-contract-v3 branch is a
separate change, not silently included. No new checkout or worktree.

## Discovery

```text
DISCOVERY LEDGER - distributed verification and three-platform measurements
  find:    existing distributed plan, Lean model, admission tests, ir_stream_bench
  grep:    replay/resume -> TransactionAssembler and Criterion groups
  symbols: SnapshotVersion, TransactionAssembler -> existing Core owners
  read:    admission fix report, roadmap, Dockerfile, core.sh, measurement helpers
  verdict: EXTEND the existing model, tests, benchmark and Docker recipe;
           generated oracle cases and measurement outputs are new evidence.
```

## Initial evidence and interpretation

The prior admission fix has nine passing public-API regressions and 19 Lean theorems;
Core main now has 417 passing tests after branch consolidation. No distributed speedup,
durable exactly-once installation, arbitrary delta application or network liveness was
proved by that work. Version axes denote artifact dependencies, not replica vector clocks.

Amaterasu: x86_64 Linux, 16 logical CPUs, about 32 GiB RAM; existing bounded Core builder,
SDK images and the existing clean Core checkout are available. No ARM64 binfmt handler
or qemu-aarch64-static is currently installed on the host. Plan an explicit interpreter
inside a bounded container, reusing native cross-compilation and avoiding global setup.

Docker documents that QEMU emulation is substantially slower for CPU-intensive work;
use cross-compilation for build cost and interpret emulated runtime separately from
native ARM hardware: https://docs.docker.com/build/building/multi-platform/ .
QEMU user mode executes foreign-architecture userspace on the host OS, not a hardware
board model: https://www.qemu.org/docs/master/user/index.html .

Plan: ../docs/plans/2026-09-05-distributed-semantics/plan.tex (continuation section).

## Verified implementation checkpoint

23 Lean 4.19.0 theorems pass (standard propext/Quot.sound only). The added theorem
`committed_replay_after_suffix` preserves modeled state when an installed transaction
is replayed after any finite suffix. It does not make rejected transactions permanent.
The deliberately false resumed-zero bootstrap theorem is rejected by the kernel.
The monitored positive check used 285.457 MiB peak working set and 1.064 seconds.

The model generates 26,406 cases over four ternary version axes. The public Rust
encode/validate/assembler path agrees for all cases under both [0,1,2] and
[0,1,u64::MAX] order-preserving embeddings: 52,812 cases. Checks include typed
outcomes and retained version/digest. This is finite conformance, not Rust refinement.
Oracle regeneration reproduced identical bytes; hashes/counts are in oracle-provenance.json.

Windows: full Core release suite **418 passed / 7 ignored**, all-target/all-feature
Clippy with denied warnings, fmt, symbols and duplication passed. Duplication remains
422 clones / 8163 lines / 197 files. Selected benchmark preflights pass. No production
source, parser pin or lockfile changed. Timed measurements are still pending.

The cross-toolchain image is built natively with the ARM64 Rust standard library,
GNU cross linker and libc development sysroot. The first candidate lacked the libc
headers/startup objects; this was corrected before cross compilation. The final
image is `sha256:326930c465e5d47623a3de3d5e0253d521b91f144b336422fad32529ca567270`.
BuildKit retains its existing six-CPU/16-GiB quota; Cargo uses four jobs. No host
binfmt registration, privileged setup or host package installation was performed.
