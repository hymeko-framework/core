//! Level 3: Signed-HGNN — Separate positive/negative incidence channels.
//!
//! X' = σ(D⁻½ B₊ W₊ B₊ᵀ X Θ₊  +  D⁻½ B₋ W₋ B₋ᵀ X Θ₋)
//!
//! This is NOVEL — no prior art on signed-incidence-aware hypergraph
//! convolution with separate learnable transforms per sign channel.
//! The signed semantics from HyMeKo's IR (Plus/Minus/Neutral incidence)
//! map directly to excitatory/inhibitory message passing channels.

use hymeko::tensor::common::Real;
use hymeko::tensor::tensor_val::{IncVal, EdgeWeight};
use crate::traversal::hypergraphview::HyperGraphView;

use hymeko::tensor::conv::traits::inv_sqrt_degree;

pub struct SignedHgnnLayer<F: Real> {
    /// Θ₊: d_in × d_out (positive channel transform)
    theta_plus: Vec<F>,
    /// Θ₋: d_in × d_out (negative channel transform)
    theta_minus: Vec<F>,
    /// Per-edge learnable W₊, W₋
    w_plus: Vec<F>,
    w_minus: Vec<F>,
    /// Precomputed inverse sqrt degree for positive/negative subgraphs
    inv_sqrt_dv_plus: Vec<F>,
    inv_sqrt_dv_minus: Vec<F>,
    inv_de_plus: Vec<F>,
    inv_de_minus: Vec<F>,
    d_in: usize,
    d_out: usize,
    num_nodes: usize,
    num_edges: usize,
}

impl<F: Real> SignedHgnnLayer<F> {
    pub fn from_view<V, EW>(
        hg: &HyperGraphView<V, EW, F>,
        d_in: usize,
        d_out: usize,
    ) -> Self
    where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
    {
        let nn = hg.num_nodes();
        let ne = hg.num_edges();

        // Compute separate degree vectors for +/- channels
        let mut dv_plus = vec![F::zero(); nn];
        let mut dv_minus = vec![F::zero(); nn];
        let mut de_plus = vec![F::zero(); ne];
        let mut de_minus = vec![F::zero(); ne];

        for e in 0..ne {
            let s = hg.edge_offsets[e];
            let eend = hg.edge_offsets[e + 1];
            for p in s..eend {
                let v = hg.flat_edge_nodes[p].0;
                let w = hg.flat_edge_w[p].degree_mass();
                match hg.flat_edge_sign[p] {
                    1 => {
                        dv_plus[v] += w;
                        de_plus[e] += w;
                    }
                    -1 => {
                        dv_minus[v] += w;
                        de_minus[e] += w;
                    }
                    _ => {
                        // Neutral: contribute to both
                        dv_plus[v] += w;
                        dv_minus[v] += w;
                        de_plus[e] += w;
                        de_minus[e] += w;
                    }
                }
            }
        }

        let inv_de = |d: &[F]| -> Vec<F> {
            d.iter().map(|&x| if x > F::zero() { F::one() / x } else { F::zero() }).collect()
        };

        let scale = F::from_other(1.0 / (d_in as f64).sqrt());

        Self {
            theta_plus: vec![scale; d_in * d_out],
            theta_minus: vec![scale; d_in * d_out],
            w_plus: vec![F::one(); ne],
            w_minus: vec![F::one(); ne],
            inv_sqrt_dv_plus: inv_sqrt_degree(&dv_plus),
            inv_sqrt_dv_minus: inv_sqrt_degree(&dv_minus),
            inv_de_plus: inv_de(&de_plus),
            inv_de_minus: inv_de(&de_minus),
            d_in, d_out,
            num_nodes: nn,
            num_edges: ne,
        }
    }

    /// Forward: dual-channel signed message passing
    pub fn forward_pass<V, EW>(
        &self,
        hg: &HyperGraphView<V, EW, F>,
        x_in: &[F],
        x_out: &mut [F],
    )
    where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
    {
        let n = self.num_nodes;
        let m = self.num_edges;

        // Z₊ = X · Θ₊,  Z₋ = X · Θ₋
        let mut z_plus = vec![F::zero(); n * self.d_out];
        let mut z_minus = vec![F::zero(); n * self.d_out];
        for i in 0..n {
            for j in 0..self.d_out {
                let mut ap = F::zero();
                let mut am = F::zero();
                for k in 0..self.d_in {
                    let xik = x_in[i * self.d_in + k];
                    ap += xik * self.theta_plus[k * self.d_out + j];
                    am += xik * self.theta_minus[k * self.d_out + j];
                }
                z_plus[i * self.d_out + j] = ap;
                z_minus[i * self.d_out + j] = am;
            }
        }

        // Per-feature signed gather/scatter
        for f in 0..self.d_out {
            let zp_col: Vec<F> = (0..n).map(|i| z_plus[i * self.d_out + f]).collect();
            let zm_col: Vec<F> = (0..n).map(|i| z_minus[i * self.d_out + f]).collect();

            // Signed gather: only accumulate matching-sign incidences
            let mut xe_plus = vec![F::zero(); m];
            let mut xe_minus = vec![F::zero(); m];

            for e in 0..m {
                let s = hg.edge_offsets[e];
                let eend = hg.edge_offsets[e + 1];
                for p in s..eend {
                    let v = hg.flat_edge_nodes[p].0;
                    let w = hg.flat_edge_w[p].degree_mass();
                    match hg.flat_edge_sign[p] {
                        1  => xe_plus[e]  += w * zp_col[v],
                        -1 => xe_minus[e] += w * zm_col[v],
                        _  => {
                            xe_plus[e]  += w * zp_col[v];
                            xe_minus[e] += w * zm_col[v];
                        }
                    }
                }
            }

            // Edge-level normalization + weights
            for e in 0..m {
                xe_plus[e]  = xe_plus[e]  * self.inv_de_plus[e]  * self.w_plus[e];
                xe_minus[e] = xe_minus[e] * self.inv_de_minus[e] * self.w_minus[e];
            }

            // Signed scatter + combine
            let mut y = vec![F::zero(); n];
            for e in 0..m {
                let s = hg.edge_offsets[e];
                let eend = hg.edge_offsets[e + 1];
                for p in s..eend {
                    let v = hg.flat_edge_nodes[p].0;
                    let w = hg.flat_edge_w[p].degree_mass();
                    match hg.flat_edge_sign[p] {
                        1  => y[v] += w * xe_plus[e],
                        -1 => y[v] += w * xe_minus[e],
                        _  => y[v] += w * (xe_plus[e] + xe_minus[e]),
                    }
                }
            }

            // Node normalization + activation (LeakyReLU)
            for i in 0..n {
                let val_p = self.inv_sqrt_dv_plus[i] * y[i];
                let val_m = self.inv_sqrt_dv_minus[i] * y[i];
                let combined = val_p + val_m;
                x_out[i * self.d_out + f] = if combined > F::zero() {
                    combined
                } else {
                    combined * F::from_other(0.01) // LeakyReLU
                };
            }
        }
    }
}