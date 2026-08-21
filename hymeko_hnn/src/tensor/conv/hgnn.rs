//! Level 2: HGNN — Hypergraph Neural Network (Feng et al. 2019)
//!
//! X' = σ(D_v⁻½ · B · W_e · D_e⁻¹ · Bᵀ · X · Θ)
//!
//! Uses the two-step gather/scatter from message_passing.rs
//! with degree normalization inserted between steps.

use hymeko::tensor::common::Real;
use hymeko::tensor::conv::traits::{compute_degree, inv_sqrt_degree, DegreeMode};
use hymeko::tensor::tensor_val::{IncVal, EdgeWeight};
use crate::traversal::hypergraphview::HyperGraphView;


pub struct HgnnLayer<F: Real> {
    /// Per-edge learnable weights W_e: num_edges
    edge_weights: Vec<F>,
    /// Feature transform Θ: d_in × d_out
    theta: Vec<F>,
    d_in: usize,
    d_out: usize,
    /// Precomputed D_v^{-1/2}
    inv_sqrt_dv: Vec<F>,
    /// Precomputed D_e^{-1}
    inv_de: Vec<F>,
    num_nodes: usize,
    num_edges: usize,
}

impl<F: Real> HgnnLayer<F> {
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

        let dv = compute_degree(
            &hg.edge_offsets, &hg.node_offsets,
            &hg.flat_edge_nodes, &hg.flat_edge_sign, &hg.flat_edge_w,
            nn, ne, DegreeMode::VertexSum,
        );
        let de = compute_degree(
            &hg.edge_offsets, &hg.node_offsets,
            &hg.flat_edge_nodes, &hg.flat_edge_sign, &hg.flat_edge_w,
            nn, ne, DegreeMode::EdgeSum,
        );

        Self {
            edge_weights: vec![F::one(); ne],
            theta: vec![F::from_other(1.0 / (d_in as f64).sqrt()); d_in * d_out],
            d_in, d_out,
            inv_sqrt_dv: inv_sqrt_degree(&dv),
            inv_de: de.iter().map(|&d| {
                if d > F::zero() { F::one() / d } else { F::zero() }
            }).collect(),
            num_nodes: nn,
            num_edges: ne,
        }
    }

    /// Forward pass: 5-step pipeline
    ///
    /// 1. Z = X · Θ                  (feature transform)
    /// 2. x_e = Bᵀ · Z              (gather: nodes → edges)
    /// 3. x_e *= D_e⁻¹ · W_e        (normalize + edge weight)
    /// 4. Y = B · x_e                (scatter: edges → nodes)
    /// 5. Y = D_v⁻½ · Y, then ReLU  (node normalization + activation)
    pub fn forward_pass<V, EW>(
        &self,
        hg: &HyperGraphView<V, EW, F>,
        x_in: &[F],   // n × d_in flat
        x_out: &mut [F], // n × d_out flat
    )
    where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
    {
        let n = self.num_nodes;
        let m = self.num_edges;

        // Step 1: Z = X · Θ  (n×d_in × d_in×d_out → n×d_out)
        let mut z = vec![F::zero(); n * self.d_out];
        for i in 0..n {
            for j in 0..self.d_out {
                let mut acc = F::zero();
                for k in 0..self.d_in {
                    acc += x_in[i * self.d_in + k] * self.theta[k * self.d_out + j];
                }
                z[i * self.d_out + j] = acc;
            }
        }

        // Steps 2–4: per feature dimension (reuses gather/scatter pattern)
        for f in 0..self.d_out {
            // Extract column f of Z
            let z_col: Vec<F> = (0..n).map(|i| z[i * self.d_out + f]).collect();

            // Step 2: Gather x_e[e] = Σ_v B[v,e] · z[v]
            let mut x_edges = vec![F::zero(); m];
            for e in 0..m {
                let s = hg.edge_offsets[e];
                let eend = hg.edge_offsets[e + 1];
                let mut acc = F::zero();
                for p in s..eend {
                    let v = hg.flat_edge_nodes[p].0;
                    acc += hg.flat_edge_w[p].degree_mass() * z_col[v];
                }
                x_edges[e] = acc;
            }

            // Step 3: x_e *= D_e⁻¹ · W_e
            for e in 0..m {
                x_edges[e] = x_edges[e] * self.inv_de[e] * self.edge_weights[e];
            }

            // Step 4: Scatter y[v] = Σ_e B[v,e] · x_e[e]
            let mut y_col = vec![F::zero(); n];
            for e in 0..m {
                let s = hg.edge_offsets[e];
                let eend = hg.edge_offsets[e + 1];
                for p in s..eend {
                    let v = hg.flat_edge_nodes[p].0;
                    y_col[v] += hg.flat_edge_w[p].degree_mass() * x_edges[e];
                }
            }

            // Step 5: D_v⁻½ normalization + ReLU
            for i in 0..n {
                let val = self.inv_sqrt_dv[i] * y_col[i];
                x_out[i * self.d_out + f] = if val > F::zero() { val } else { F::zero() };
            }
        }
    }
}