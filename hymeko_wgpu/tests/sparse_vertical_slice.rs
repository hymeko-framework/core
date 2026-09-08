//! The sparse edge-to-node vertical slice, as **one** integration-test target.
//!
//! # Why one target and not three
//!
//! Measured on 2026-09-08, on an RTX 3070 Laptop / Vulkan host. Cargo runs integration-test
//! binaries **strictly sequentially** here — sampling every process under `target/debug/deps`
//! showed 14 binaries with zero overlapping starts, each beginning 30–60 ms after the previous one
//! ended. So the failure mode that appeared when this slice added three more binaries is *not*
//! process parallelism. It is `RapidSequentialDeviceLifecyclePressure`: 14 binaries inside ~20 s,
//! several of them creating and dropping a Vulkan device per test (`arena` 21, `activation` 19,
//! `skeleton` 16, `spmm` 15), and the driver failing under that churn rate.
//!
//! Two reductions follow, and both are applied:
//!
//! * **one device per target** — every module below shares the single `OnceLock` backend in
//!   `hymeko_wgpu::testing::shared_backend`, so this slice creates one device instead of three;
//! * **one target instead of three** — the package returns to 11 GPU-using binaries, the count
//!   measured stable at default threading (5/5), rather than the 13 that was not (2/5).
//!
//! The alternative — declaring `--test-threads=1` canonical for the whole package — was rejected:
//! the default gate was green before this slice, and relaxing a working gate to accommodate an
//! addition is the wrong trade.
//!
//! # What is deliberately unchanged
//!
//! Three scientific responsibilities, three separate test populations, every assertion and every
//! mutation control. They are modules, not a merged file: `scatter_mean` owns the pooling primitive
//! and the core-validation boundary, `highway_identity` owns the transition, and
//! `resident_hsikan_chain` owns the composed chain, its residency evidence and its parity. No test
//! helper moved into production code.

#[path = "support/mod.rs"]
mod support;

#[path = "sparse/highway_identity.rs"]
mod highway_identity;
#[path = "sparse/resident_hsikan_chain.rs"]
mod resident_hsikan_chain;
#[path = "sparse/scatter_mean.rs"]
mod scatter_mean;
