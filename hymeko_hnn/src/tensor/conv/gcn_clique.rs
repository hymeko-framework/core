//! Level 1: GCN on clique-expanded adjacency A = B·Bᵀ
//!
//! X' = σ(D⁻½ A D⁻½ X W)
//!
//! Uses existing `build_explicit_a()` → `TensorCsr` → `spmm()`.

use hymeko::tensor::common::Real;
use hymeko::tensor::representations::tensor_csr::TensorCsr;

use hymeko::tensor::conv::traits::{compute_degree, inv_sqrt_degree, DegreeMode, HypergraphConv};
use hymeko::tensor::conv::weight_init::weight_init::{WeightInit, Xavier};
use hymeko::tensor::tensor_val::{IncVal, EdgeWeight};
use crate::traversal::hypergraphview::HyperGraphView;
use crate::tensor::message_passing::{build_explicit_a, CliqueStepCfg};


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
    where V: IncVal<F>, EW: EdgeWeight<V, F>,
    {
        Self::from_view_with_init(hg, d_in, d_out, &Xavier)
    }

    /// Build from HyperGraphView. Precomputes the normalized adjacency.
    pub fn from_view_with_init<V, EW, I>(
        hg: &HyperGraphView<V, EW, F>,
        d_in: usize,
        d_out: usize,
        init: &I
    ) -> Self
    where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
        I: WeightInit<F>,
    {
        let cfg = CliqueStepCfg { use_abs: true, include_self: true };
        let a_coo = build_explicit_a(hg, cfg);

        // Convert COO → dense then build normalized CSR
        // For production: use COO→CSR directly + diagonal scaling
        let n = hg.num_nodes();
        let deg = compute_degree(
            &hg.edge_offsets, &hg.node_offsets,
            &hg.flat_edge_nodes, &hg.flat_edge_sign,
            &hg.flat_edge_w, n, hg.num_edges(),
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
        for i in 0..n { row_ptr[i + 1] += row_ptr[i]; }

        let norm_adj = TensorCsr {
            num_rows: n,
            num_cols: n,
            row_ptr,
            col_ind,
            val,
        };

        // Initialize weights with Xavier uniform
        let weights = init.create(d_in, d_out);

        Self { weights, d_in, d_out, norm_adj }
    }

    pub fn reinit_weights<I: WeightInit<F>>(&mut self, init: &I) {
        init.init(self.d_in, self.d_out, &mut self.weights);
    }

    /// Forward: X' = ReLU(Ã · X · W)
    /// x_in: n × d_in (row-major flat), x_out: n × d_out (row-major flat)
    pub fn forward_relu(&self, x_in: &[F], x_out: &mut [F]) {
        let n = self.norm_adj.num_rows;

        // Step 1: H = X · W  (dense matmul, n×d_in × d_in×d_out → n×d_out)
        let mut h = vec![F::zero(); n * self.d_out];
        for i in 0..n {
            for j in 0..self.d_out {
                let mut acc = F::zero();
                for k in 0..self.d_in {
                    acc += x_in[i * self.d_in + k] * self.weights[k * self.d_out + j];
                }
                h[i * self.d_out + j] = acc;
            }
        }

        // Step 2: X' = Ã · H  (sparse × dense, uses spmm)
        let result = self.norm_adj.spmm(&h, self.d_out);

        // Step 3: ReLU
        for i in 0..result.len() {
            x_out[i] = if result[i] > F::zero() { result[i] } else { F::zero() };
        }
    }
}

impl<F: Real> HypergraphConv<F> for GcnCliqueLayer<F> {
    fn forward(&self, x_in: &[F], d_in: usize, x_out: &mut [F], d_out: usize) {
        debug_assert_eq!(d_in, self.d_in);
        debug_assert_eq!(d_out, self.d_out);
        self.forward_relu(x_in, x_out);
    }
    fn num_params(&self) -> usize { self.weights.len() }
    fn params_mut(&mut self) -> &mut [F] { &mut self.weights }
    fn params(&self) -> &[F] { &self.weights }
}