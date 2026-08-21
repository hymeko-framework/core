use hymeko::common::ids::EdgeId;
use hymeko::tensor::common::Real;
use crate::tensor::common_traversal::inc_to_real;
use hymeko::tensor::representations::tensor_csr::{build_row_ptr, TensorCsr};
use hymeko::tensor::tensor_val::{EdgeWeight, IncVal};
use crate::traversal::hypergraphview::HyperGraphView;

// ==========================================
// STAR EXPANSION (|V| + |E|) x (|V| + |E|)
// ==========================================

pub fn star_expansion_csr<V, EW, F>(hg: &HyperGraphView<V, EW, F>) -> TensorCsr<F>
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    let dim = hg.num_nodes() + hg.num_edges();
    let counts = compute_star_counts(hg);
    let (row_ptr, nnz) = build_row_ptr(&counts);

    populate_star_csr(hg, dim, nnz, row_ptr)
}

fn compute_star_counts<V, EW, F>(hg: &HyperGraphView<V, EW, F>) -> Vec<usize>
where
    V: IncVal<F>, EW: EdgeWeight<V, F>, F: Real
{
    let dim = hg.num_nodes() + hg.num_edges();
    let mut counts = vec![0; dim];

    for e in 0..hg.num_edges() {
        let (s, eend) = hg.edge_span(EdgeId::new(e));
        let e_v = hg.num_nodes() + e;

        for p in s..eend {
            let n_v = hg.flat_edge_nodes[p].0 as usize;
            match hg.flat_edge_sign[p] {
                1 => counts[n_v] += 1,
                -1 => counts[e_v] += 1,
                _ => { counts[n_v] += 1; counts[e_v] += 1; }
            }
        }
    }
    counts
}

fn populate_star_csr<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>, dim: usize, nnz: usize, row_ptr: Vec<usize>
) -> TensorCsr<F>
where
    V: IncVal<F>, EW: EdgeWeight<V, F>, F: Real
{
    let mut csr = TensorCsr::with_capacity(dim, dim, nnz);
    csr.row_ptr = row_ptr.clone();
    let mut offset = row_ptr;

    for e in 0..hg.num_edges() {
        let (s, eend) = hg.edge_span(EdgeId::new(e));
        let e_v = hg.num_nodes() + e;

        for p in s..eend {
            let n_v = hg.flat_edge_nodes[p].0 as usize;
            let w = inc_to_real(hg, p, e);

            let mut insert = |row: usize, col: usize, val: F| {
                let idx = offset[row];
                csr.col_ind[idx] = col;
                csr.val[idx] = val;
                offset[row] += 1;
            };

            match hg.flat_edge_sign[p] {
                1 => insert(n_v, e_v, w),
                -1 => insert(e_v, n_v, w),
                _ => { insert(n_v, e_v, w); insert(e_v, n_v, w); }
            }
        }
    }
    csr
}


// ==========================================
// CLIQUE EXPANSION (Projected 2D: |V| x |V|)
// ==========================================

pub fn clique_expansion_csr<V, EW, F>(hg: &HyperGraphView<V, EW, F>) -> TensorCsr<F>
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    let dim = hg.num_nodes();
    let counts = compute_clique_counts(hg, dim);
    let (row_ptr, nnz) = build_row_ptr(&counts);

    populate_clique_csr(hg, dim, nnz, row_ptr)
}

fn compute_clique_counts<V, EW, F>(hg: &HyperGraphView<V, EW, F>, num_nodes: usize) -> Vec<usize>
where
    V: IncVal<F>, EW: EdgeWeight<V, F>, F: Real
{
    let mut counts = vec![0; num_nodes];

    for e in 0..hg.num_edges() {
        let (s, eend) = hg.edge_span(EdgeId::new(e));
        let nodes_in_edge = eend - s;

        for i in 0..nodes_in_edge {
            let u_idx = s + i;
            let u = hg.flat_edge_nodes[u_idx].0;
            let su = hg.flat_edge_sign[u_idx];

            for j in (i + 1)..nodes_in_edge {
                let v_idx = s + j;
                let sv = hg.flat_edge_sign[v_idx];

                match (su, sv) {
                    (1, -1) => counts[u] += 1,      // u -> v
                    (-1, 1) => counts[hg.flat_edge_nodes[v_idx].0] += 1, // v -> u
                    _ => {
                        counts[u] += 1;
                        counts[hg.flat_edge_nodes[v_idx].0] += 1;
                    }
                }
            }
        }
    }
    counts
}

fn populate_clique_csr<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>, dim: usize, nnz: usize, row_ptr: Vec<usize>
) -> TensorCsr<F>
where
    V: IncVal<F>, EW: EdgeWeight<V, F>, F: Real
{
    let mut csr = TensorCsr::with_capacity(dim, dim, nnz);
    csr.row_ptr = row_ptr.clone();
    let mut offset = row_ptr;

    for e in 0..hg.num_edges() {
        let (s, eend) = hg.edge_span(EdgeId::new(e));
        let nodes_in_edge = eend - s;

        // Gather local nodes to avoid repetitive memory lookups
        let mut local_nodes = Vec::with_capacity(nodes_in_edge);
        for p in s..eend {
            local_nodes.push((
                hg.flat_edge_nodes[p].0,
                hg.flat_edge_sign[p],
                inc_to_real(hg, p, e),
            ));
        }

        for i in 0..local_nodes.len() {
            let (u, su, wu) = local_nodes[i];
            for j in (i + 1)..local_nodes.len() {
                let (v, sv, wv) = local_nodes[j];
                let w = wu * wv;

                let mut insert = |row: usize, col: usize, val: F| {
                    let idx = offset[row];
                    csr.col_ind[idx] = col;
                    csr.val[idx] = val;
                    offset[row] += 1;
                };

                match (su, sv) {
                    (1, -1) => insert(u, v, w),
                    (-1, 1) => insert(v, u, w),
                    _ => { insert(u, v, w); insert(v, u, w); }
                }
            }
        }
    }
    csr
}
