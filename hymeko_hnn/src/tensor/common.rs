//! `hymeko_hnn::tensor::common` — the single `HyperGraphView`-aware
//! helper that was split out of `hymeko_core::tensor::common` during
//! the 2026-04-18 extraction. Everything else in `common.rs` (the
//! `Real` trait, `AsF32` / `AsF64`, `signed_incidence`) stays in core.

use hymeko::tensor::common::Real;
use hymeko::tensor::tensor_val::{EdgeWeight, IncVal};

use crate::traversal::hypergraphview::HyperGraphView;

/// Rough upper bound on the non-zero count of a clique expansion.
/// Sum over edges of `deg_e * (deg_e - 1)`.
#[inline]
pub fn calc_approx_nnz<V, EW, F>(hg: &HyperGraphView<V, EW, F>, num_edges: usize) -> usize
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    (0..num_edges)
        .map(|e| {
            let d = hg.edge_offsets[e + 1] - hg.edge_offsets[e];
            d * d.saturating_sub(1)
        })
        .sum()
}
