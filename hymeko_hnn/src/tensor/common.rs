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

/// Incidence count below which the **scalar** hypergraph kernels stay serial.
///
/// Applies to `gather` / `scatter` / `clique_diag`, whose inner loop performs
/// one multiply-add per incidence and reaches ~2 Gelem/s serially.
///
/// Measured on the reference host (24 logical cores) with
/// `cargo bench --bench hnn_bench -- 'mp_(gather|scatter)_paths'`:
///
/// | incidences | gather serial | gather par | scatter serial | scatter par |
/// |----------:|-----:|-----:|-----:|-----:|
/// | 512       | 223 ns  | 14.92 us | 249 ns   | 17.77 us |
/// | 8 192     | 3.38 us | 21.70 us | 4.20 us  | 21.93 us |
/// | 16 384    | 6.73 us | 22.45 us | 8.66 us  | 23.20 us |
/// | 65 536    | 30.30 us| 25.82 us | 113.3 us | 35.19 us |
/// | 1 048 576 | 730 us  | 119.0 us | -        | -        |
///
/// Rayon's fork/join floor is ~15-18 us. The two kernels do **not** share a
/// crossover: gather's is ~45 k, scatter's ~25 k (its serial form reads
/// `x_edges` randomly and falls off a cache cliff between 16 k and 65 k —
/// 1.89 Gelem/s down to 578 Melem/s). One constant is kept rather than two,
/// at roughly the geometric mean, because the regret either side is small
/// (~1.4x on gather in 33-45 k, ~1.2x on scatter in 25-33 k) and a second knob
/// is not worth that. The first draft, 16 384, was measurably too low: at that
/// size the parallel path is 3.3x slower on gather.
pub const PAR_INCIDENCE_THRESHOLD: usize = 32_768;

/// Work count below which the **feature-blocked** kernels stay serial.
///
/// Applies to the HGNN / Signed-HGNN gather, scatter and dense-transform tiles,
/// whose inner loop runs over contiguous equal-length slices and vectorises.
/// Work is `incidences * tile_width` for the message-passing tiles and
/// `nodes * d_in * d_out` for the dense transform.
///
/// Applies ONLY to the dense `X . Theta` transform, whose inner loop walks two
/// contiguous rows. It deliberately does **not** apply to the gather/scatter
/// tiles: those loop over `width` contiguous features per incidence but reach
/// each incidence through a *random* `v * d_out + f0` slice, measured at only
/// ~2.5 Gelem/s serially, so they use [`PAR_INCIDENCE_THRESHOLD`] instead. An
/// earlier draft applied this constant to the tiles too and measurably slowed
/// `conv_hgnn_forward/small` by 9 %.
///
/// An order of magnitude above [`PAR_INCIDENCE_THRESHOLD`] and deliberately so:
/// a vectorised inner loop reaches ~11 Gelem/s serially against the scalar
/// kernels' ~2, so it takes proportionally more work before the same fork/join
/// cost pays for itself. This mirrors the SpMV-vs-SpMM split measured in
/// `TensorCsr::PAR_SPMM_WORK_THRESHOLD`, and is set to the same value for the
/// same reason.
pub const PAR_BLOCKED_WORK_THRESHOLD: usize = 262_144;

/// Upper bound, in elements, on the feature-blocking scratch buffer the
/// convolution forward passes allocate.
///
/// The forward passes trade scratch memory for incidence traversals: holding
/// `num_edges * tile` intermediate values lets one traversal serve `tile`
/// output features instead of one. Left unbounded that is `num_edges *
/// d_out`, which is fine at the sizes this workspace benchmarks (~13 MiB at
/// 100k edges x 32 features) but unbounded in principle. This cap keeps the
/// scratch at roughly 4 MiB of `f32` regardless of graph size, degrading
/// gracefully toward the old per-feature behaviour on very large graphs
/// instead of allocating proportionally to `edges x features`.
pub const FEATURE_TILE_BUDGET: usize = 1 << 20;

/// Number of output features to process per incidence traversal.
///
/// # Postconditions
/// Returns a value in `1..=d_out` (or 0 when `d_out` is 0), chosen so that
/// `num_edges * tile <= FEATURE_TILE_BUDGET` wherever that is achievable.
#[inline]
#[must_use]
pub fn feature_tile(num_edges: usize, d_out: usize) -> usize {
    if d_out == 0 {
        return 0;
    }
    if num_edges == 0 {
        return d_out;
    }
    (FEATURE_TILE_BUDGET / num_edges).clamp(1, d_out)
}
