//! `hymeko_hnn` — hypergraph neural-network operators and the bipartite
//! incidence view that underpins them.
//!
//! Extracted from `hymeko_core` on 2026-04-18 so the tensor-op tangle
//! that blocked the earlier [`hymeko_hre`] extraction could finally be
//! unwound. `hymeko_core` keeps the tensor primitives (`TensorCoo`,
//! `Real`, `aggregation`, `tensor_val`, etc.); everything that touches a
//! [`traversal::hypergraphview::HyperGraphView`] lives here.
//!
//! Module layout mirrors the pre-extraction shape inside
//! `hymeko_core::{traversal,tensor}` so downstream crates only need a
//! crate-prefix search-and-replace:
//!
//! - `hymeko::traversal::*` → `hymeko_hnn::traversal::*`
//! - `hymeko::tensor::{common_traversal,message_passing,mesh_nn,tensor}::*` → `hymeko_hnn::tensor::…::*`
//! - `hymeko::tensor::conv::{hgnn,signed_hgnn,gcn_clique}::*` → `hymeko_hnn::tensor::conv::…::*`
//! - `hymeko::tensor::representations::tensor_csr_representations::*` → `hymeko_hnn::tensor::representations::tensor_csr_representations::*`
//! - `hymeko::tensor::common::calc_approx_nnz` → `hymeko_hnn::tensor::common::calc_approx_nnz`
//!
//! The last one is the only function-level split: `calc_approx_nnz` was
//! the single `HyperGraphView`-aware item in core's `tensor::common`;
//! the rest of that file (`Real`, `AsF32`, `AsF64`, `signed_incidence`)
//! stays in core.

// Hot loops and `pub mod tensor` layout match the pre-split core shape;
// clippy would require wide refactors for marginal gain here.
#![allow(
    clippy::module_inception,
    clippy::needless_range_loop,
    clippy::unnecessary_cast,
)]

pub mod traversal;
pub mod tensor;
