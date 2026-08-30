//! Incidence → scalar projection, in both traversal orientations.
//!
//! A [`HyperGraphView`] materialises the same incidence multiset twice: once
//! ordered by `(edge, node)` and once by `(node, edge)`. Kernels that produce
//! an *edge*-indexed result (gather) walk the first; kernels that produce a
//! *node*-indexed result (scatter, degree, diagonal) walk the second, which is
//! what lets them run in parallel without atomics.
//!
//! Both orientations project an incidence to a scalar the same way, so the
//! arithmetic lives once in [`project_incidence`] and the orientation-specific
//! wrappers only choose which parallel arrays to index. Before this was
//! unified there were two byte-identical copies of the projection — one here
//! and one private to `message_passing` (CLAUDE.md §6.1).

use hymeko::tensor::common::{Real, signed_incidence};
use hymeko::tensor::tensor_val::{EdgeWeight, IncVal};

use crate::traversal::hypergraphview::HyperGraphView;

/// Project one incidence to a signed scalar `b`.
///
/// Applies the edge weight to the incidence value, projects to a scalar,
/// multiplies by the incidence sign, and optionally takes the absolute value.
///
/// # Invariants
/// Pure: depends only on its arguments, so the two orientation wrappers below
/// cannot drift apart numerically.
#[inline(always)]
fn project_incidence<V, EW, F>(edge_weight: &EW, value: &V, sign: i8, use_abs: bool) -> F
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    let mut b = edge_weight.apply_to(value.clone()).as_scalar();
    b *= signed_incidence::<F>(sign);
    if use_abs {
        b = b.abs();
    }
    b
}

/// Unsigned incidence value at edge-first flat position `p` of edge `e`.
///
/// # Preconditions
/// `p < hg.flat_edge_w.len()` and `e < hg.num_edges()`.
#[inline(always)]
pub fn inc_to_real<V, EW, F>(hg: &HyperGraphView<V, EW, F>, p: usize, e: usize) -> F
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    // apply edge weight to the incidence value, then project to scalar
    hg.edge_weight[e]
        .apply_to(hg.flat_edge_w[p].clone())
        .as_scalar()
}

/// Signed incidence at **edge-first** flat position `p` of edge `e`.
///
/// Indexes `flat_edge_w` / `flat_edge_sign`, i.e. the `(edge, node)`-ordered
/// arrays reached through `edge_offsets`.
///
/// # Preconditions
/// `hg.edge_offsets[e] <= p < hg.edge_offsets[e + 1]`.
#[inline(always)]
pub fn inc_scalar_signed<V, EW, F>(
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
    project_incidence(
        &hg.edge_weight[e],
        &hg.flat_edge_w[p],
        hg.flat_edge_sign[p],
        use_abs,
    )
}

/// Signed incidence at **node-first** flat position `p`, whose edge is `e`.
///
/// Indexes `flat_node_w` / `flat_node_sign`, i.e. the `(node, edge)`-ordered
/// arrays reached through `node_offsets`. `e` must be the edge stored at that
/// position — `hg.flat_node_edges[p].raw()` — because the per-edge weight is still
/// looked up globally.
///
/// # Preconditions
/// `hg.node_offsets[v] <= p < hg.node_offsets[v + 1]` for the node `v` being
/// visited, and `e == hg.flat_node_edges[p].raw()`.
///
/// # Postconditions
/// Returns exactly the value [`inc_scalar_signed`] returns for the same
/// logical incidence reached through the edge-first arrays. The two views hold
/// the same multiset, so this is an identity, not an approximation.
#[inline(always)]
pub fn inc_scalar_signed_node<V, EW, F>(
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
    debug_assert_eq!(
        hg.flat_node_edges[p].raw(),
        e,
        "node-first position {p} does not belong to edge {e}"
    );
    project_incidence(
        &hg.edge_weight[e],
        &hg.flat_node_w[p],
        hg.flat_node_sign[p],
        use_abs,
    )
}
