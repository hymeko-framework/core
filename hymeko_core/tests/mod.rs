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

// APPROVED-CORE-EDIT: webxr-semantic-vocabulary (2026-09-01).
#[path = "webxr/mod.rs"]
mod webxr;
