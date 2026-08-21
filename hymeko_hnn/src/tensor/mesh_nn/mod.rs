//! Mesh-aware hypergraph convolution.
//!
//! Boundary operator ∂: maps k-cells to (k-1)-cells (face → edges → vertices)
//! Co-boundary operator δ: maps k-cells to (k+1)-cells (vertex → edges → faces)
//!
//! In HyMeKo terms:
//!   ∂ = B^T (incidence transpose: edges → nodes)
//!   δ = B   (incidence: nodes → edges)
//!
//! The mesh Laplacian: L_k = ∂_{k+1} ∂_{k+1}^T + ∂_k^T ∂_k
//! For k=0 (node features): L_0 = B B^T (same as clique expansion!)

use hymeko::tensor::common::Real;
use hymeko::tensor::tensor_val::{IncVal, EdgeWeight};
use crate::traversal::hypergraphview::HyperGraphView;

/// Mesh-aware message passing with boundary/co-boundary operators.
pub struct MeshConvLayer<F: Real> {
    /// Boundary weights (edge → node direction): d_edge × d_node
    w_boundary: Vec<F>,
    /// Co-boundary weights (node → edge direction): d_node × d_edge
    w_coboundary: Vec<F>,
    d_node: usize,
    d_edge: usize,
}

impl<F: Real> MeshConvLayer<F> {
    pub fn new(d_node: usize, d_edge: usize) -> Self {
        let scale = F::from_other(0.01);
        Self {
            w_boundary: vec![scale; d_edge * d_node],
            w_coboundary: vec![scale; d_node * d_edge],
            d_node, d_edge,
        }
    }

    /// Forward pass with bidirectional message passing.
    ///
    /// 1. Edge features: h_e = σ(W_co · (δ^T h_v))   (co-boundary: aggregate node features per edge)
    /// 2. Node features: h_v' = σ(W_bd · (∂ h_e))     (boundary: scatter edge features to nodes)
    pub fn forward<V, EW>(
        &self,
        hg: &HyperGraphView<V, EW, F>,
        h_nodes: &[F],    // num_nodes × d_node
        h_nodes_out: &mut [F], // num_nodes × d_node
    )
    where
        V: IncVal<F>, EW: EdgeWeight<V, F>,
    {
        let n = hg.num_nodes();
        let m = hg.num_edges();

        // Step 1: Co-boundary (nodes → edges): h_e = W_co · (Σ_v B[v,e] · h_v)
        let mut h_edges = vec![F::zero(); m * self.d_edge];
        for e in 0..m {
            let s = hg.edge_offsets[e];
            let eend = hg.edge_offsets[e + 1];

            // Aggregate node features into edge
            let mut agg = vec![F::zero(); self.d_node];
            for p in s..eend {
                let v = hg.flat_edge_nodes[p].0;
                let w = hg.flat_edge_w[p].degree_mass();
                for d in 0..self.d_node {
                    agg[d] += w * h_nodes[v * self.d_node + d];
                }
            }

            // Transform: W_co · agg
            for j in 0..self.d_edge {
                let mut val = F::zero();
                for k in 0..self.d_node {
                    val += self.w_coboundary[k * self.d_edge + j] * agg[k];
                }
                h_edges[e * self.d_edge + j] = if val > F::zero() { val } else { F::zero() };
            }
        }

        // Step 2: Boundary (edges → nodes): h_v' = W_bd · (Σ_e B[v,e] · h_e)
        for i in 0..n {
            let mut agg = vec![F::zero(); self.d_edge];
            let ns = hg.node_offsets[i];
            let ne = hg.node_offsets[i + 1];

            for p in ns..ne {
                let eid = hg.flat_node_edges[p].0;
                let w = hg.flat_node_w[p].degree_mass();
                for d in 0..self.d_edge {
                    agg[d] += w * h_edges[eid * self.d_edge + d];
                }
            }

            for j in 0..self.d_node {
                let mut val = F::zero();
                for k in 0..self.d_edge {
                    val += self.w_boundary[k * self.d_node + j] * agg[k];
                }
                // Residual connection + ReLU
                let res = h_nodes[i * self.d_node + j] + val;
                h_nodes_out[i * self.d_node + j] = if res > F::zero() { res } else { F::zero() };
            }
        }
    }
}