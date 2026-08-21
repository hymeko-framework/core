//! Hypergraph convolution operators. The agnostic traits
//! (`HypergraphConv`, `compute_degree`, …) and weight initialisers live
//! in `hymeko_core::tensor::conv` and are re-exported from there.

pub mod gcn_clique;
pub mod hgnn;
pub mod signed_hgnn;
