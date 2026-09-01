# Measurement contract / metric definitions (§1)
Created 2026-09-01 12:35 CEDT.

## Phases (never compare across phases)
- **cold**: parse + lowering + index/build + operation (fresh).
- **prepared**: Ir/index/workspace already built; measure the operation.
- **resident**: repeated operation, no new allocation or rebuild.
- **end-to-end**: source → canonical result.
A resident kernel time is NEVER compared to another path's cold time.

## Reported quantities (per cell)
median; where the harness gives it reliably: CI, p95/p99; throughput; allocations; allocated bytes;
retained bytes; peak RSS; bytes/{declaration,node,hyperarc,incidence}; CPU thread count; ambient CPU
load; GPU clock/temp/load (GPU cells only); compiler/version/profile; exact git tree hash.

## Method
- One typed time helper; a single unit per table (µs or ms), labelled — no µs/ns mislabelling.
- A/B comparisons are interleaved (alternating), never two large sequential blocks.
- An A/A null run is included to estimate estimator + host drift.
- Raw output is machine-readable TSV/CSV.

## Reuse rule (CLAUDE.md)
A trustworthy prior measurement is cited, not re-run from scratch. New measurement targets: unified-method
verification of headline claims, gaps with no prior number, and A/A drift. The 2026-08-25 published
baseline and the 2026-09-01 incidence/graph/webxr reports are the citable baseline (see baseline table).
