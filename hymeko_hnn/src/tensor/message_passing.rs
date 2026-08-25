//! Hypergraph message passing: gather (nodes → edges), scatter (edges →
//! nodes), the clique diagonal, and the implicit `B W Bᵀ` step.
//!
//! # Traversal orientation and parallelism
//!
//! Every kernel here is embarrassingly parallel *provided it is walked in the
//! orientation that matches its output index*:
//!
//! * **gather** produces an edge-indexed result, so it walks the edge-first
//!   incidence CSR (`edge_offsets` / `flat_edge_*`). Edges are disjoint.
//! * **scatter** and **clique_diag** produce node-indexed results, so they
//!   walk the **node-first** CSR (`node_offsets` / `flat_node_*`). Nodes are
//!   disjoint.
//!
//! Scatter was previously written edge-major, which forced a read-modify-write
//! into a shared `y[v]` and would have needed atomics or per-thread partials to
//! parallelise. Walking the node-first CSR instead removes the sharing
//! entirely — no atomics, no locks, no reduction buffers.
//!
//! ## Why this does not change the numbers
//!
//! Both CSRs are built from the same incidence multiset, the node-first one
//! ordered by `(node, edge)`. The old edge-major loop visited `e = 0..m` in
//! order, so a given node `v` accumulated its contributions in strictly
//! increasing edge order. Walking `v`'s own slice of the node-first array
//! visits exactly those incidences, also in strictly increasing edge order.
//! The summation sequence per output element is therefore *identical*, and the
//! rewrite is bitwise reproducible — not merely equal within a tolerance.
//! Partitioning over nodes cannot reassociate anything, because no two threads
//! ever touch the same accumulator. This is asserted by
//! `test_scatter_serial_matches_parallel_bitwise` rather than assumed.

use rayon::prelude::*;

use hymeko::tensor::common::{Real, signed_incidence};
use hymeko::tensor::representations::tensor_coo::TensorCoo;
use hymeko::tensor::tensor_val::{EdgeWeight, IncVal};

use crate::tensor::common::{PAR_INCIDENCE_THRESHOLD, calc_approx_nnz};
use crate::tensor::common_traversal::{inc_scalar_signed, inc_scalar_signed_node, inc_to_real};
use crate::traversal::hypergraphview::HyperGraphView;

#[derive(Clone, Copy, Debug)]
pub struct CliqueStepCfg {
    pub use_abs: bool,      // default: true
    pub include_self: bool, // default: false (kiveszi az önhurkot)
}

impl Default for CliqueStepCfg {
    fn default() -> Self {
        Self {
            use_abs: true,
            include_self: false,
        }
    }
}

/// Rows (edges or nodes) per Rayon task.
///
/// Several tasks per worker, so work-stealing can rebalance irregular
/// cardinalities instead of leaving every thread waiting on whichever one drew
/// the hub rows.
#[inline]
fn par_chunk(len: usize) -> usize {
    let tasks = rayon::current_num_threads().saturating_mul(8).max(1);
    len.div_ceil(tasks).max(1)
}

// ===========================================================================
// 1) Gather:  x_e = Bᵀ x        (edge-indexed output, edge-first traversal)
// ===========================================================================

/// `x_e = Bᵀ x` — dispatches to the serial or parallel path on workload size.
///
/// # Preconditions
/// `x_nodes.len() == hg.num_nodes()` and `x_edges_out.len() == hg.num_edges()`.
///
/// # Postconditions
/// Every element of `x_edges_out` is overwritten, never accumulated into.
/// Bitwise identical whichever path runs.
pub fn gather_edges_from_nodes<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    x_nodes: &[F],
    x_edges_out: &mut [F],
    use_abs: bool,
) where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    if hg.flat_edge_nodes.len() >= PAR_INCIDENCE_THRESHOLD {
        gather_edges_from_nodes_parallel(hg, x_nodes, x_edges_out, use_abs);
    } else {
        gather_edges_from_nodes_serial(hg, x_nodes, x_edges_out, use_abs);
    }
}

/// Single-threaded gather. Contract as [`gather_edges_from_nodes`].
pub fn gather_edges_from_nodes_serial<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    x_nodes: &[F],
    x_edges_out: &mut [F],
    use_abs: bool,
) where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    debug_assert_eq!(x_nodes.len(), hg.num_nodes());
    debug_assert_eq!(x_edges_out.len(), hg.num_edges());

    for (e, slot) in x_edges_out.iter_mut().enumerate() {
        *slot = edge_accumulate(hg, e, x_nodes, use_abs);
    }
}

/// Rayon gather, partitioned by output edge. Contract as
/// [`gather_edges_from_nodes`].
pub fn gather_edges_from_nodes_parallel<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    x_nodes: &[F],
    x_edges_out: &mut [F],
    use_abs: bool,
) where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    debug_assert_eq!(x_nodes.len(), hg.num_nodes());
    debug_assert_eq!(x_edges_out.len(), hg.num_edges());
    if x_edges_out.is_empty() {
        return;
    }

    let chunk = par_chunk(x_edges_out.len());
    x_edges_out
        .par_chunks_mut(chunk)
        .enumerate()
        .for_each(|(chunk_idx, block)| {
            let base = chunk_idx * chunk;
            for (offset, slot) in block.iter_mut().enumerate() {
                *slot = edge_accumulate(hg, base + offset, x_nodes, use_abs);
            }
        });
}

/// `x_e[e] = Σ_{v ∈ e} b(v,e) · x[v]`, walking the edge-first CSR.
///
/// The single definition of gather's inner loop; both paths call it.
#[inline(always)]
fn edge_accumulate<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    e: usize,
    x_nodes: &[F],
    use_abs: bool,
) -> F
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    let s = hg.edge_offsets[e];
    let eend = hg.edge_offsets[e + 1];

    let mut acc = F::zero();
    for p in s..eend {
        let v = hg.flat_edge_nodes[p].0;
        let b = inc_scalar_signed(hg, p, e, use_abs);
        acc += b * x_nodes[v];
    }
    acc
}

// ===========================================================================
// 2) Scatter:  y = B x_e        (node-indexed output, node-first traversal)
// ===========================================================================

/// `y = B x_e` — dispatches to the serial or parallel path on workload size.
///
/// # Preconditions
/// `x_edges.len() == hg.num_edges()` and `y_nodes_out.len() == hg.num_nodes()`.
///
/// # Postconditions
/// Every element of `y_nodes_out` is overwritten, never accumulated into — the
/// explicit zeroing pass the edge-major version needed is gone because each
/// node's value is now computed in one place. Bitwise identical whichever path
/// runs, and bitwise identical to the pre-rewrite edge-major result (see the
/// module docs for the ordering argument).
pub fn scatter_nodes_from_edges<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    x_edges: &[F],
    y_nodes_out: &mut [F],
    use_abs: bool,
) where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    if hg.flat_node_edges.len() >= PAR_INCIDENCE_THRESHOLD {
        scatter_nodes_from_edges_parallel(hg, x_edges, y_nodes_out, use_abs);
    } else {
        scatter_nodes_from_edges_edge_major(hg, x_edges, y_nodes_out, use_abs);
    }
}

/// Single-threaded, **edge-major** scatter — the fastest serial form.
///
/// Contract as [`scatter_nodes_from_edges`], and bitwise identical to the
/// node-major variants.
///
/// Kept, and used as the small-input path, on measurement rather than taste.
/// Node-major traversal performs two random loads per incidence
/// (`edge_weight[e]` and `x_edges[e]`) where edge-major hoists `x_edges[e]`
/// once per edge and performs one random write. Node-major wins at scale
/// purely because it is the shape that parallelises without atomics; run
/// serially it is strictly more work per incidence. The first post-rewrite
/// measurement showed exactly that — a ~14 % regression on `mp_scatter/small`,
/// the one cell below the parallel threshold, while `large` improved 6.6x.
/// Dispatching small inputs back to this form removes the regression without
/// giving up the parallel win.
pub fn scatter_nodes_from_edges_edge_major<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    x_edges: &[F],
    y_nodes_out: &mut [F],
    use_abs: bool,
) where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    debug_assert_eq!(x_edges.len(), hg.num_edges());
    debug_assert_eq!(y_nodes_out.len(), hg.num_nodes());

    for y in y_nodes_out.iter_mut() {
        *y = F::zero();
    }

    for e in 0..hg.num_edges() {
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

/// Single-threaded scatter. Contract as [`scatter_nodes_from_edges`].
pub fn scatter_nodes_from_edges_serial<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    x_edges: &[F],
    y_nodes_out: &mut [F],
    use_abs: bool,
) where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    debug_assert_eq!(x_edges.len(), hg.num_edges());
    debug_assert_eq!(y_nodes_out.len(), hg.num_nodes());

    for (v, slot) in y_nodes_out.iter_mut().enumerate() {
        *slot = node_accumulate(hg, v, x_edges, use_abs);
    }
}

/// Rayon scatter, partitioned by output node. No atomics, no locks.
/// Contract as [`scatter_nodes_from_edges`].
pub fn scatter_nodes_from_edges_parallel<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    x_edges: &[F],
    y_nodes_out: &mut [F],
    use_abs: bool,
) where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    debug_assert_eq!(x_edges.len(), hg.num_edges());
    debug_assert_eq!(y_nodes_out.len(), hg.num_nodes());
    if y_nodes_out.is_empty() {
        return;
    }

    let chunk = par_chunk(y_nodes_out.len());
    y_nodes_out
        .par_chunks_mut(chunk)
        .enumerate()
        .for_each(|(chunk_idx, block)| {
            let base = chunk_idx * chunk;
            for (offset, slot) in block.iter_mut().enumerate() {
                *slot = node_accumulate(hg, base + offset, x_edges, use_abs);
            }
        });
}

/// `y[v] = Σ_{e ∋ v} b(v,e) · x_e[e]`, walking the node-first CSR.
///
/// The single definition of scatter's inner loop; the serial path, the
/// parallel path, and the fused clique step all call it.
#[inline(always)]
fn node_accumulate<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    v: usize,
    x_edges: &[F],
    use_abs: bool,
) -> F
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    let s = hg.node_offsets[v];
    let nend = hg.node_offsets[v + 1];

    let mut acc = F::zero();
    for p in s..nend {
        let e = hg.flat_node_edges[p].0;
        let b = inc_scalar_signed_node(hg, p, e, use_abs);
        acc += b * x_edges[e];
    }
    acc
}

// ===========================================================================
// 3) Clique diagonal:  diag[v] = Σ_e b(v,e)²
// ===========================================================================

/// `diag[v] = Σ_e b_{v,e}²`.
///
/// Convenience wrapper: allocates, then fills.
///
/// It deliberately does **not** delegate to [`clique_diag_into`]. That
/// function's postcondition is "fully overwritten", which obliges its
/// edge-major branch to clear the buffer first — but `vec![F::zero(); n]` has
/// already cleared it. Routing through it therefore zeroed the vector twice per
/// call, which measured as a 29 % regression on `mp_clique_diag/small` (a
/// defect introduced by this optimisation round and caught by re-measurement,
/// not by review). Dispatching here instead keeps the single clearing pass.
pub fn clique_diag<V, EW, F>(hg: &HyperGraphView<V, EW, F>, use_abs: bool) -> Vec<F>
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    let mut diag = vec![F::zero(); hg.num_nodes()];
    if diag.is_empty() {
        return diag;
    }

    if hg.flat_node_edges.len() >= PAR_INCIDENCE_THRESHOLD {
        clique_diag_node_major(hg, use_abs, &mut diag);
    } else {
        // `vec!` already zeroed the buffer; accumulate straight into it.
        accumulate_square_edge_major(hg, use_abs, &mut diag);
    }
    diag
}

/// Allocation-free clique diagonal.
///
/// # Preconditions
/// `diag_out.len() == hg.num_nodes()`.
///
/// # Postconditions
/// Every element overwritten. Bitwise identical to the edge-major original,
/// by the same ordering argument as scatter.
pub fn clique_diag_into<V, EW, F>(hg: &HyperGraphView<V, EW, F>, use_abs: bool, diag_out: &mut [F])
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    debug_assert_eq!(diag_out.len(), hg.num_nodes());
    if diag_out.is_empty() {
        return;
    }

    if hg.flat_node_edges.len() >= PAR_INCIDENCE_THRESHOLD {
        clique_diag_node_major(hg, use_abs, diag_out);
    } else {
        // Edge-major below the threshold, for the reason documented on
        // `scatter_nodes_from_edges_edge_major`: node-major is the
        // parallelisable shape, not the faster serial one. It accumulates, so
        // the buffer must be cleared first to honour this function's
        // fully-overwritten postcondition.
        for slot in diag_out.iter_mut() {
            *slot = F::zero();
        }
        accumulate_square_edge_major(hg, use_abs, diag_out);
    }
}

/// Node-major, Rayon-parallel diagonal. Assigns every slot, so no clearing needed.
fn clique_diag_node_major<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    use_abs: bool,
    diag_out: &mut [F],
) where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    let chunk = par_chunk(diag_out.len());
    diag_out
        .par_chunks_mut(chunk)
        .enumerate()
        .for_each(|(chunk_idx, block)| {
            let base = chunk_idx * chunk;
            for (offset, slot) in block.iter_mut().enumerate() {
                *slot = node_square_sum(hg, base + offset, use_abs);
            }
        });
}

/// Edge-major `out[v] += b(v,e)²`.
///
/// # Preconditions
/// `out` must already be zeroed — this **accumulates**, it does not overwrite.
/// Keeping the clearing pass out of here is what lets [`clique_diag`] rely on
/// the zeroing `vec!` already performed instead of doing it twice.
#[inline]
fn accumulate_square_edge_major<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    use_abs: bool,
    out: &mut [F],
) where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    for e in 0..hg.num_edges() {
        let s = hg.edge_offsets[e];
        let eend = hg.edge_offsets[e + 1];
        for p in s..eend {
            let v = hg.flat_edge_nodes[p].0;
            let b = inc_scalar_signed(hg, p, e, use_abs);
            out[v] += b * b;
        }
    }
}

/// `Σ_{e ∋ v} b(v,e)²`, walking the node-first CSR.
#[inline(always)]
fn node_square_sum<V, EW, F>(hg: &HyperGraphView<V, EW, F>, v: usize, use_abs: bool) -> F
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    let s = hg.node_offsets[v];
    let nend = hg.node_offsets[v + 1];

    let mut acc = F::zero();
    for p in s..nend {
        let e = hg.flat_node_edges[p].0;
        let b = inc_scalar_signed_node(hg, p, e, use_abs);
        acc += b * b;
    }
    acc
}

// ===========================================================================
// 4) Implicit clique step:  y = B W Bᵀ x
// ===========================================================================

/// `y -= diag ⊙ x` (elementwise).
///
/// Retained as a public operation in its own right. [`implicit_clique_step`]
/// no longer calls it — the subtraction is fused into the scatter, which saves
/// a second full pass over `y` — but the semantics are identical and the
/// function stays part of the API.
#[inline(always)]
pub fn remove_self_effect<F: Real>(y: &mut [F], diag: &[F], x_nodes: &[F]) {
    debug_assert_eq!(y.len(), diag.len());
    debug_assert_eq!(y.len(), x_nodes.len());
    for i in 0..y.len() {
        y[i] -= diag[i] * x_nodes[i];
    }
}

/// `y = B W Bᵀ x` (implicit, sparse).
///
/// # Preconditions
/// `x_nodes.len() == y_nodes_out.len() == hg.num_nodes()`,
/// `buffer_edges.len() == hg.num_edges()`, and when `!cfg.include_self`,
/// `precomputed_diag` is `Some` with length `hg.num_nodes()`.
///
/// # Postconditions
/// `y_nodes_out` fully overwritten; `buffer_edges` left holding the
/// intermediate `Bᵀ x` (it is scratch, not an output, but is deterministic).
///
/// # Panics
/// If `!cfg.include_self` and `precomputed_diag` is `None`.
pub fn implicit_clique_step<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    x_nodes: &[F],
    y_nodes_out: &mut [F],
    buffer_edges: &mut [F],
    precomputed_diag: Option<&[F]>,
    cfg: CliqueStepCfg,
) where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    // 1) gather
    gather_edges_from_nodes(hg, x_nodes, buffer_edges, cfg.use_abs);

    // 2) scatter, with the diagonal removal fused in where requested.
    //
    // Fusing matters because the subtraction is node-indexed exactly like the
    // scatter output: doing it inside the same loop reuses `y[v]` while it is
    // still in a register, instead of writing the whole vector out and reading
    // it back for a second elementwise pass. The arithmetic is unchanged --
    // `acc - diag[v] * x[v]` computes what `y[v] = acc; y[v] -= diag[v] * x[v]`
    // computed, in the same order -- so the result stays bitwise identical.
    match (cfg.include_self, precomputed_diag) {
        (true, _) => {
            scatter_nodes_from_edges(hg, buffer_edges, y_nodes_out, cfg.use_abs);
        }
        (false, Some(diag)) => {
            scatter_and_remove_self(hg, buffer_edges, diag, x_nodes, y_nodes_out, cfg.use_abs);
        }
        (false, None) => {
            panic!("Precomputed diagonal required if include_self is false");
        }
    }
}

/// Scatter with the diagonal removal fused in: `y[v] = Σ_e b·x_e − diag[v]·x[v]`.
fn scatter_and_remove_self<V, EW, F>(
    hg: &HyperGraphView<V, EW, F>,
    x_edges: &[F],
    diag: &[F],
    x_nodes: &[F],
    y_nodes_out: &mut [F],
    use_abs: bool,
) where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    debug_assert_eq!(diag.len(), hg.num_nodes());
    debug_assert_eq!(x_nodes.len(), hg.num_nodes());
    debug_assert_eq!(y_nodes_out.len(), hg.num_nodes());
    if y_nodes_out.is_empty() {
        return;
    }

    if hg.flat_node_edges.len() >= PAR_INCIDENCE_THRESHOLD {
        let chunk = par_chunk(y_nodes_out.len());
        y_nodes_out
            .par_chunks_mut(chunk)
            .enumerate()
            .for_each(|(chunk_idx, block)| {
                let base = chunk_idx * chunk;
                for (offset, slot) in block.iter_mut().enumerate() {
                    let v = base + offset;
                    *slot = node_accumulate(hg, v, x_edges, use_abs) - diag[v] * x_nodes[v];
                }
            });
    } else {
        // Below the threshold the edge-major scatter plus a separate
        // elementwise pass beats node-major-with-fusion, for the reason
        // documented on `scatter_nodes_from_edges_edge_major`. Fusion only pays
        // once the traversal is node-major anyway.
        scatter_nodes_from_edges_edge_major(hg, x_edges, y_nodes_out, use_abs);
        remove_self_effect(y_nodes_out, diag, x_nodes);
    }
}

// ===========================================================================
// 5) Explicit clique expansion (COO)
// ===========================================================================

pub fn build_explicit_a<V, EW, F>(hg: &HyperGraphView<V, EW, F>, cfg: CliqueStepCfg) -> TensorCoo<F>
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    let n = hg.num_nodes();
    let m = hg.num_edges();

    // A = B B^T
    let mut coo = TensorCoo::with_meta(1, n, n);
    // Pre-pass
    let approx_nnz = calc_approx_nnz(hg, m);
    coo.reserve(approx_nnz);

    // Reused across edges: this used to be a fresh `Vec::with_capacity` per
    // hyperedge, i.e. one allocation per edge in the hot loop.
    let mut nodes: Vec<(usize, F)> = Vec::new();

    // Main pass
    for e in 0..m {
        let s = hg.edge_offsets[e];
        let eend = hg.edge_offsets[e + 1];

        // kis edge-szelet (v, bve)
        nodes.clear();
        nodes.reserve(eend - s);
        for p in s..eend {
            let v = hg.flat_edge_nodes[p].0;
            let mut b: F = inc_to_real(hg, p, e);
            b *= signed_incidence::<F>(hg.flat_edge_sign[p]);
            if cfg.use_abs {
                b = b.abs();
            }
            nodes.push((v, b));
        }

        // outer product: A[u,v] += b(u,e) * b(v,e)
        for &(u, bu) in &nodes {
            for &(v, bv) in &nodes {
                if !cfg.include_self && u == v {
                    continue;
                }
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
