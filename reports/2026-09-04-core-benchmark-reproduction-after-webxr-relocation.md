# Core benchmark reproduction after WebXR relocation

Date: 2026-09-04 CEST

Measured core: `f5181de6735380abb1efc060bebad548ccf459aa`

## Verdict

The WebXR ownership move causes no canonical-hash implementation or harness
change: `hymeko_core/src/ir/canonical_hash.rs` has blob
`dd9fc8b8e45d86d30339be342163b413eb5416e3` both before (`d6be4ea`) and after
(`f5181de`) the move, and `hymeko_core/benches/ir_bench.rs` has blob
`6beef6fa7cde9169e2a6f48d64f84607cefee6cf` at both revisions.

The strong qualitative result reproduces: the 15,000-declaration arity-4
canonical hash remains substantially below the old 10.74 ms pre-authority-flip
measurement. The exact historical 0.643 / 7.47 / 22.84 ms headline does not
reproduce within 3% in this workstation session, so those absolute values must
not be treated as a portable release gate.

## Current Criterion runs

Command:

```powershell
cargo bench -p hymeko_core --bench ir_bench -- ir_canonical_hash --noplot
```

Two consecutive runs used Criterion's 3-second warmup, 20 samples, and the
benchmark's 6-second measurement target per cell.

| Cell | Historical | Run 1 median (CI) | Run 2 median (CI) |
|---|---:|---:|---:|
| 1.5k declarations, arity 4 | 0.643 ms | 0.696 ms (0.685–0.710) | 0.709 ms (0.695–0.726) |
| 15k declarations, arity 4 | 7.47 ms | 8.104 ms (8.045–8.188) | 8.543 ms (8.461–8.645) |
| 15k declarations, 160k incidences | 22.84 ms | 24.319 ms (24.191–24.461) | 26.129 ms (25.909–26.298) |

The five-sample ambient window before the second run was 15.2% average and 20%
maximum CPU. Consecutive-run medians moved by approximately +1.9%, +5.4% and
+7.4% respectively, confirming that this host/session cannot support a strict
3% absolute-time conclusion. The Kato rerun should preserve per-host samples
and compare distributions rather than pool raw values across machines.

## Related graph evidence

The independent informed A*/Dijkstra/Berge harness was rerun separately on the
same workstation. Its semantic result and deterministic work tables reproduce
exactly, while the median new/baseline absolute-time ratio across 48 cells is
1.0519 under a higher-ambient-load session. Its within-run algorithmic ratios
remain stable. D*/LPA* is still explicitly unimplemented rather than simulated
with repeated A*.
