//! Level 1: GCN on clique-expanded adjacency A = B·Bᵀ
//!
//! X' = σ(D⁻½ A D⁻½ X W)
//!
//! Uses existing `build_explicit_a()` → `TensorCsr` → `spmm()`.

use hymeko::tensor::common::Real;
use hymeko::tensor::representations::tensor_csr::TensorCsr;

use crate::tensor::message_passing::{CliqueStepCfg, build_explicit_a};
use crate::traversal::hypergraphview::HyperGraphView;
use hymeko::tensor::conv::traits::{
    DegreeMode, HypergraphConv, compute_degree_by_index, inv_sqrt_degree,
};
use hymeko::tensor::conv::weight_init::weight_init::{WeightInit, Xavier};
use hymeko::tensor::tensor_val::{EdgeWeight, IncVal};

pub struct GcnCliqueLayer<F: Real> {
    /// Learnable weight matrix W: d_in × d_out (row-major)
    weights: Vec<F>,
    d_in: usize,
    d_out: usize,
    /// Precomputed normalized adjacency: D⁻½ A D⁻½
    norm_adj: TensorCsr<F>,
}

impl<F: Real> GcnCliqueLayer<F> {
    pub fn from_view<V, EW>(hg: &HyperGraphView<V, EW, F>, d_in: usize, d_out: usize) -> Self
    where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
    {
        Self::from_view_with_init(hg, d_in, d_out, &Xavier)
    }

    /// Build from HyperGraphView. Precomputes the normalized adjacency.
    pub fn from_view_with_init<V, EW, I>(
        hg: &HyperGraphView<V, EW, F>,
        d_in: usize,
        d_out: usize,
        init: &I,
    ) -> Self
    where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
        I: WeightInit<F>,
    {
        let cfg = CliqueStepCfg {
            use_abs: true,
            include_self: true,
        };
        let a_coo = build_explicit_a(hg, cfg);

        // Convert COO → dense then build normalized CSR
        // For production: use COO→CSR directly + diagonal scaling
        let n = hg.num_nodes();
        let deg = compute_degree_by_index(
            &hg.edge_offsets,
            &hg.flat_edge_nodes,
            &hg.flat_edge_w,
            n,
            hg.num_edges(),
            |d| d.as_usize(),
            DegreeMode::VertexSquaredSum,
        );
        let inv_sqrt_d = inv_sqrt_degree(&deg);

        // Build normalized CSR: scale each entry a[i][j] by inv_sqrt_d[i] * inv_sqrt_d[j]
        let mut norm_entries: Vec<(usize, usize, F)> = Vec::with_capacity(a_coo.len());
        for entry in a_coo.iter() {
            let scaled = entry.v * inv_sqrt_d[entry.i] * inv_sqrt_d[entry.j];
            if scaled != F::zero() {
                norm_entries.push((entry.i, entry.j, scaled));
            }
        }

        // Sort by (row, col) and build CSR
        norm_entries.sort_unstable_by_key(|&(i, j, _)| (i, j));

        let mut row_ptr = vec![0usize; n + 1];
        let mut col_ind = Vec::with_capacity(norm_entries.len());
        let mut val = Vec::with_capacity(norm_entries.len());

        for &(i, j, v) in &norm_entries {
            row_ptr[i + 1] += 1;
            col_ind.push(j);
            val.push(v);
        }
        for i in 0..n {
            row_ptr[i + 1] += row_ptr[i];
        }

        let norm_adj = TensorCsr {
            num_rows: n,
            num_cols: n,
            row_ptr,
            col_ind,
            val,
        };

        // Initialize weights with Xavier uniform
        let weights = init.create(d_in, d_out);

        Self {
            weights,
            d_in,
            d_out,
            norm_adj,
        }
    }

    pub fn reinit_weights<I: WeightInit<F>>(&mut self, init: &I) {
        init.init(self.d_in, self.d_out, &mut self.weights);
    }

    /// Forward: X' = ReLU(Ã · X · W)
    /// x_in: n × d_in (row-major flat), x_out: n × d_out (row-major flat)
    ///
    /// # Postconditions
    /// The first `num_rows * d_out` elements of `x_out` are overwritten. A
    /// longer `x_out` is tolerated and its tail left untouched, matching the
    /// previous behaviour.
    pub fn forward_relu(&self, x_in: &[F], x_out: &mut [F]) {
        let n = self.norm_adj.num_rows;

        // Step 1: H = X · W  (dense matmul, n×d_in × d_in×d_out → n×d_out)
        //
        // Loop order i → k → j walks a contiguous row of `weights` in the
        // innermost loop. Each H[i][j] still accumulates over k in 0..d_in
        // order, so the result is unchanged bitwise; the previous i → j → k
        // order strode `weights` by d_out and could not vectorise.
        let mut h = vec![F::zero(); n * self.d_out];
        for (i, h_row) in h.chunks_mut(self.d_out).enumerate() {
            let x_row = &x_in[i * self.d_in..(i + 1) * self.d_in];
            for (k, &xik) in x_row.iter().enumerate() {
                let w_row = &self.weights[k * self.d_out..(k + 1) * self.d_out];
                for (hj, &wj) in h_row.iter_mut().zip(w_row.iter()) {
                    *hj += xik * wj;
                }
            }
        }

        // Step 2: X' = Ã · H, written straight into the caller's buffer.
        // The allocating `spmm` used to materialise a second n × d_out Vec
        // here purely to be copied out again.
        let (out, _tail) = x_out.split_at_mut(n * self.d_out);
        self.norm_adj.spmm_into(&h, self.d_out, out);

        // Step 3: ReLU, in place.
        //
        // Written as an explicit select rather than `if *slot <= zero`, so a
        // NaN still maps to zero exactly as the original
        // `if result[i] > zero { result[i] } else { zero }` did. `NaN <= 0` is
        // false, so the inverted form would have let NaNs through.
        for slot in out.iter_mut() {
            *slot = if *slot > F::zero() { *slot } else { F::zero() };
        }
    }
}

impl<F: Real> HypergraphConv<F> for GcnCliqueLayer<F> {
    fn forward(&self, x_in: &[F], d_in: usize, x_out: &mut [F], d_out: usize) {
        debug_assert_eq!(d_in, self.d_in);
        debug_assert_eq!(d_out, self.d_out);
        self.forward_relu(x_in, x_out);
    }
    fn num_params(&self) -> usize {
        self.weights.len()
    }
    fn params_mut(&mut self) -> &mut [F] {
        &mut self.weights
    }
    fn params(&self) -> &[F] {
        &self.weights
    }
}
