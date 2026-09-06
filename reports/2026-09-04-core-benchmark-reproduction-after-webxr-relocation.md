# Core benchmark reproduction after WebXR relocation

Date: 2026-09-04 CEST

Initial WebXR-only measurement: `f5181de6735380abb1efc060bebad548ccf459aa`

Integrated measurement inputs: `47a17af16df7d333a397146d78bd855d18ede0a9`
(canonical CBOR, streaming incidence hash, and agent contract) merged with
`410a4d5dabce545a0ab1ff536048c4d6eb5c17f8` (WebXR relocation and the initial
post-relocation evidence).

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

## Integrated-tree rerun

The combined source tree was measured after the CBOR corpus gate was adjusted
from 105 to the post-relocation 102 lowerable core fixtures. The full workspace
then passed with 528 tests, 0 failures and 7 intentional ignores; all-target,
all-feature Clippy also passed with warnings denied.

The host CPU window immediately before the CBOR run was 9.4% average / 13%
maximum. Raw Criterion samples give the following per-iteration medians:

| CBOR operation | Scale | Original streaming median | Integrated median | Change |
|---|---|---:|---:|---:|
| encode | small | 31.568 us | 27.225 us | -13.76% |
| encode | medium | 100.578 us | 96.763 us | -3.79% |
| encode | large | 629.591 us | 616.783 us | -2.03% |
| decode | small | 5.719 us | 5.954 us | +4.11% |
| decode | medium | 120.162 us | 124.734 us | +3.81% |
| decode | large | 1,088.568 us | 1,133.382 us | +4.12% |

The original values are the streaming medians recorded in
`reports/2026-09-04-canonical-cbor-codec.md`. The integrated run used the same
three deterministic workloads, 30 samples, 3-second warm-up and 5-second
measurement window. The result reproduces the claimed performance class; it
does not reproduce byte-identical absolute timings, nor should it be used as a
cross-host absolute gate.

The host CPU window immediately before the integrated canonical-hash run was
11.2% average / 19% maximum. Its raw-sample results were:

| Cell | Raw median | IQR | Worst sample |
|---|---:|---:|---:|
| 1.5k declarations, arity 4 | 0.69285 ms | 0.01120 ms | 0.70543 ms |
| 15k declarations, arity 4 | 8.21721 ms | 0.40883 ms | 10.90610 ms |
| 15k declarations, 160k incidences | 15.82595 ms | 0.26757 ms | 19.10190 ms |

The first two cells remain in the same range as the WebXR-only reruns. The
high-incidence cell is materially faster than both the historical 22.84 ms
headline and the WebXR-only 24.32-26.13 ms runs because the integrated tree
contains the approved streaming-incidence canonical hash. This is the expected
algorithmic effect, not an effect of relocating WebXR.

### Production-scale peak memory

The 1,048,576-incidence release fixture was regenerated and each cell was then
run in a fresh process while Windows' process high-water counter was sampled at
5 ms. The wire image remained 51,422,046 bytes and the decoded public IR heap
estimate remained 94,221,761 bytes.

| Cell | Integrated peak | Original streaming peak |
|---|---:|---:|
| read baseline | 56,557,568 B | 56,553,472 B |
| materialize only | 151,937,024 B | 151,928,832 B |
| validated decode | 155,901,952 B | 155,906,048 B |

The validation/hash increment is 3,964,928 bytes. Decode adds 99,344,384 bytes
over the read baseline, or **1.0544x** the decoded IR heap, versus the original
streaming result of 1.0545x. The final peak differs from the recorded value by
only 4 KiB and remains far below the 16 GiB cap. This reproduces the strongest
memory claim essentially exactly on the integrated source tree.

## Related graph evidence

The independent informed A*/Dijkstra/Berge harness was rerun separately on the
same workstation. Its semantic result and deterministic work tables reproduce
exactly, while the median new/baseline absolute-time ratio across 48 cells is
1.0519 under a higher-ambient-load session. Its within-run algorithmic ratios
remain stable. D*/LPA* is still explicitly unimplemented rather than simulated
with repeated A*.
