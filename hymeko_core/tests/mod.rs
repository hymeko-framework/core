// APPROVED-CORE-EDIT: core-manifest-and-hymeko-core-clippy — this integration
// test crate is large and predates strict `clippy -D warnings`; the library
// uses targeted allows in `src/lib.rs` instead.
#![allow(warnings)]
#![allow(clippy::approx_constant, clippy::needless_lifetimes)]

#[path = "minimal_tests/mod.rs"]
mod minimal_tests;

#[path = "typical_graphs/mod.rs"]
mod typical_graphs;

#[path = "intermediate_tests/mod.rs"]
mod intermediate_tests;

#[path = "traversal/mod.rs"]
mod traversal;

pub mod test_asserts;
pub mod test_helpers;

mod aggregations;
mod basic;
mod computations;
mod hash;
mod test_tensor_representations;

// APPROVED-CORE-EDIT: unified-ir-m0-5-tests-and-benchmarks (2026-08-28).

// APPROVED-CORE-EDIT: unified-ir-m0-6-hash-and-traversal (2026-08-28).

// APPROVED-CORE-EDIT: unified-ir-m2a-accessor-api (2026-08-28).

// APPROVED-CORE-EDIT: unified-ir-m2b-vv-core-consumers (2026-08-29).
#[path = "unified_ir/mod.rs"]
mod unified_ir;

// P1 benchmark-harness consolidation: frozen-identity KAT for the shared
// deterministic sparse-CSR fixture (no core-src change; tests/ is allowlisted).
#[path = "sparse_workload_kat.rs"]
mod sparse_workload_kat;

mod ir_stream_admission;

// APPROVED-CORE-EDIT: ir-stream-cbor-snapshot (2026-09-15): canonical CBOR over ir_stream.
mod ir_stream_cbor_snapshot;

// Shared incidence-test scaffolding (§6.1) used by the P2 + P2.1 suites below.
#[path = "incidence_support.rs"]
mod incidence_support;

// P2 §4.1 isolation profile (temporary; wired here because autotests=false).
#[path = "incidence_precompute_profile.rs"]
mod incidence_precompute_profile;

// P2.1 canonical incidence backbone: PreparedIr owned bundle + borrowed view.
#[path = "prepared_ir_backbone.rs"]
mod prepared_ir_backbone;

// Part-2 §4 Bx profile (temporary; autotests=false).
#[path = "bx_profile.rs"]
mod bx_profile;

// APPROVED-CORE-EDIT: generic-arena-contract (2026-09-27): Pool<B> and HostArena.
mod compute_arena;
