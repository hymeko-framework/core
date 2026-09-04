# Host / environment capture — platform performance survey
Captured 2026-09-01 12:35 CEDT.

## Primary host (local)
- OS: Windows 11 (build 26200), MSYS/MINGW64 shell.
- CPU: 24 logical cores.
- GPU: NVIDIA GeForce RTX 5080 Laptop GPU (driver 32.0.16.1088, ~4 GiB) + Intel integrated.
- Toolchain: rustc 1.97.1 (8bab26f4f 2026-07-14), cargo 1.97.1. Release profile for benchmarks.
- Ambient CPU load at capture: ~10.6% (borderline the <10% quiet-host target).

## Second host (Amaterasu)
- Reachable: yes (ssh kyberszittya@192.168.1.228). Linux Ubuntu 24.04, kernel 7.0.0, 16 CPUs.
- **No Rust toolchain installed** (`rustc` absent). Running the Rust benchmarks would require
  installing a toolchain + cloning both repos + building — an outward, hard-to-reverse change to a
  foreign host not performed this session. **Two-host comparison: NOT PERFORMED** (single-host only);
  numbers are not invented for it.

## Quiet-host status (§1)
Ambient ~10.6% > the <10% target. Therefore: correctness and diagnostic measurements are valid;
fine-grained regression / crossover deltas are **PROVISIONAL**. Where a measurement is a re-verification
of a prior trustworthy report, the prior number is cited rather than re-measured from scratch (CLAUDE.md).
