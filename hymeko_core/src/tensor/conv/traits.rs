use crate::tensor::common::Real;
use crate::tensor::tensor_val::IncVal;

/// Unifying trait for all hypergraph convolution layers.
///
/// The `forward` method takes node features X (flattened: n × d)
/// and produces output features X' (flattened: n × d_out).
///
/// `params` holds learnable weight matrices as flat arrays.
pub trait HypergraphConv<F: Real> {
    /// Forward pass: X (n × d_in) → X' (n × d_out)
    fn forward(&self, x_in: &[F], d_in: usize, x_out: &mut [F], d_out: usize);

    /// Number of learnable parameters
    fn num_params(&self) -> usize;

    /// Mutable access to parameter slice (for optimizer updates)
    fn params_mut(&mut self) -> &mut [F];

    /// Immutable access to parameter slice
    fn params(&self) -> &[F];
}

/// Degree vector computation modes.
#[derive(Clone, Copy, Debug)]
pub enum DegreeMode {
    /// d(v) = Σ_e |b(v,e)|  (standard vertex degree)
    VertexSum,
    /// d(e) = Σ_v |b(v,e)|  (hyperedge degree = cardinality)
    EdgeSum,
    /// d(v) = Σ_e b(v,e)²   (squared incidence, used in clique_diag)
    VertexSquaredSum,
}

/// Compute degree vector from HyperGraphView.
pub fn compute_degree<F: Real, V: IncVal<F>>(
    edge_offsets: &[usize],
    node_offsets: &[usize],
    flat_edge_nodes: &[crate::common::ids::NodeId],
    flat_edge_sign: &[i8],
    flat_edge_w: &[V],
    num_nodes: usize,
    num_edges: usize,
    mode: DegreeMode,
) -> Vec<F> {
    match mode {
        DegreeMode::VertexSum => {
            let mut deg = vec![F::zero(); num_nodes];
            for e in 0..num_edges {
                let s = edge_offsets[e];
                let eend = edge_offsets[e + 1];
                for p in s..eend {
                    let v = flat_edge_nodes[p].0;
                    deg[v] += flat_edge_w[p].degree_mass();
                }
            }
            deg
        }
        DegreeMode::EdgeSum => {
            let mut deg = vec![F::zero(); num_edges];
            for e in 0..num_edges {
                let s = edge_offsets[e];
                let eend = edge_offsets[e + 1];
                for p in s..eend {
                    deg[e] += flat_edge_w[p].degree_mass();
                }
            }
            deg
        }
        DegreeMode::VertexSquaredSum => {
            let mut deg = vec![F::zero(); num_nodes];
            for e in 0..num_edges {
                let s = edge_offsets[e];
                let eend = edge_offsets[e + 1];
                for p in s..eend {
                    let v = flat_edge_nodes[p].0;
                    let w = flat_edge_w[p].degree_mass();
                    deg[v] += w * w;
                }
            }
            deg
        }
    }
}

/// Inverse square root of degree: D^{-1/2}
#[inline]
pub fn inv_sqrt_degree<F: Real>(deg: &[F]) -> Vec<F> {
    deg.iter()
        .map(|&d| {
            if d > F::zero() {
                F::one() / d.sqrt()
            } else {
                F::zero()
            }
        })
        .collect()
}
