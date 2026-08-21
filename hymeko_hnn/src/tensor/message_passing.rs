use crate::tensor::common::calc_approx_nnz;
use hymeko::tensor::common::{signed_incidence, Real};
use crate::tensor::common_traversal::inc_to_real;
use hymeko::tensor::representations::tensor_coo::TensorCoo;
use hymeko::tensor::tensor_val::{EdgeWeight, IncVal};
use crate::traversal::hypergraphview::HyperGraphView;

#[derive(Clone, Copy, Debug)]
pub struct CliqueStepCfg {
    pub use_abs: bool,      // default: true
    pub include_self: bool, // default: false (kiveszi az önhurkot)
}

impl Default for CliqueStepCfg {
    fn default() -> Self {
        Self { use_abs: true, include_self: false }
    }
}


#[inline(always)]
fn inc_scalar_signed<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    p: usize,
    e: usize,
    use_abs: bool,
) -> F
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    // incidence value -> apply edge weight -> project to scalar
    let mut b = hg.edge_weight[e]
        .apply_to(hg.flat_edge_w[p].clone())
        .as_scalar();

    // sign
    b *= signed_incidence::<F>(hg.flat_edge_sign[p]);

    if use_abs { b = b.abs(); }
    b
}

/// 1) x_e = B^T x
pub fn gather_edges_from_nodes<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    x_nodes: &[F],
    x_edges_out: &mut [F],
    use_abs: bool,
)
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    debug_assert_eq!(x_nodes.len(), hg.num_nodes());
    debug_assert_eq!(x_edges_out.len(), hg.num_edges());

    let m = hg.num_edges();
    // Originally zero initialized in the function
    // To ensure zero vector
    //let mut x_edges = vec![F::zero(); m];

    for e in 0..m {
        let s = hg.edge_offsets[e];
        let eend = hg.edge_offsets[e + 1];

        let mut acc = F::zero();
        for p in s..eend {
            let v = hg.flat_edge_nodes[p].0;
            let b = inc_scalar_signed(hg, p, e, use_abs);
            acc += b * x_nodes[v];
        }
        x_edges_out[e] = acc;
    }
}

/// 2) y = B x_e
pub fn scatter_nodes_from_edges<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    x_edges: &[F],
    y_nodes_out: &mut [F],
    use_abs: bool,
)
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    debug_assert_eq!(x_edges.len(), hg.num_edges());
    debug_assert_eq!(y_nodes_out.len(), hg.num_nodes());

    let m = hg.num_edges();
    // Originally zero initialized in the function
    // let mut y = vec![F::zero(); n];

    // Zero the buffer
    for y in y_nodes_out.iter_mut() {
        *y = F::zero();
    }

    for e in 0..m {
        let s = hg.edge_offsets[e];
        let eend = hg.edge_offsets[e + 1];
        let xe = x_edges[e];

        for p in s..eend {
            let v = hg.flat_edge_nodes[p].0;
            let b = inc_scalar_signed(hg, p, e, use_abs);
            y_nodes_out[v] += b * xe;
        }
    }
}

/// diag[v] = Σ_e b_{v,e}^2
pub fn clique_diag<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    use_abs: bool,
) -> Vec<F>
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    let n = hg.num_nodes();
    let m = hg.num_edges();
    let mut diag = vec![F::zero(); n];

    for e in 0..m {
        let s = hg.edge_offsets[e];
        let eend = hg.edge_offsets[e + 1];

        for p in s..eend {
            let v = hg.flat_edge_nodes[p].0;
            let b = inc_scalar_signed(hg, p, e, use_abs);
            diag[v] += b * b;
        }
    }

    diag
}

/// y -= diag ⊙ x  (elementwise)
#[inline(always)]
pub fn remove_self_effect<F: Real>(y: &mut [F], diag: &[F], x_nodes: &[F]) {
    debug_assert_eq!(y.len(), diag.len());
    debug_assert_eq!(y.len(), x_nodes.len());
    for i in 0..y.len() {
        y[i] -= diag[i] * x_nodes[i];
    }
}

/// y = B W B^T x  (implicit, sparse)
pub fn implicit_clique_step<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    x_nodes: &[F],
    y_nodes_out: &mut [F],
    buffer_edges: &mut [F],
    precomputed_diag: Option<&[F]>,
    cfg: CliqueStepCfg,
)
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    // 1) gather
    gather_edges_from_nodes(hg, x_nodes, buffer_edges, cfg.use_abs);

    // 2) scatter
    scatter_nodes_from_edges(hg, buffer_edges, y_nodes_out, cfg.use_abs);

    // 3) optional diagonal removal
    if !cfg.include_self {
        let diag = precomputed_diag.expect("Precomputed diagonal required if include_self is false");
        remove_self_effect(y_nodes_out, diag, x_nodes);
    }
}



pub fn build_explicit_a<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>, cfg: CliqueStepCfg) -> TensorCoo<F>
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real
{
    let n = hg.num_nodes();
    let m = hg.num_edges();

    // A = B B^T
    let mut coo = TensorCoo::with_meta(1, n, n);
    // Pre-pass
    let approx_nnz = calc_approx_nnz(hg, m);
    coo.reserve(approx_nnz);
    // Main pass
    for e in 0..m {
        let s = hg.edge_offsets[e];
        let eend = hg.edge_offsets[e + 1];

        // kis edge-szelet (v, bve)
        let mut nodes: Vec<(usize, F)> = Vec::with_capacity(eend - s);
        for p in s..eend {
            let v = hg.flat_edge_nodes[p].0;
            let mut b: F = inc_to_real(hg, p, e);
            b *= signed_incidence::<F>(hg.flat_edge_sign[p]);
            if cfg.use_abs { b = b.abs(); }
            nodes.push((v, b));
        }

        // outer product: A[u,v] += b(u,e) * b(v,e)
        for &(u, bu) in &nodes {
            for &(v, bv) in &nodes {
                if !cfg.include_self && u == v { continue; }
                coo.push(0, u, v, bu * bv);
            }
        }
    }
    coo
}

pub fn print_dense_real<F: Real>(mat: &[Vec<F>], title: &str) {
    let n = mat.len();
    let m = if n > 0 { mat[0].len() } else { 0 };
    println!("{title} ({}x{}):", n, m);
    for i in 0..n {
        for j in 0..m {
            print!("{:7.2} ", mat[i][j].as_f64());
        }
        println!();
    }
}