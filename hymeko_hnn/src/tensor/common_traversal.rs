use hymeko::tensor::common::{signed_incidence, Real};
use hymeko::tensor::tensor_val::{EdgeWeight, IncVal};
use crate::traversal::hypergraphview::HyperGraphView;

#[inline(always)]
pub fn inc_to_real<V, EW, F>(hg: &HyperGraphView<V, EW, F>, p: usize, e: usize) -> F
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    // apply edge weight to the incidence value, then project to scalar
    hg.edge_weight[e].apply_to(hg.flat_edge_w[p].clone()).as_scalar()
}

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
    let mut b = hg.edge_weight[e]
        .apply_to(hg.flat_edge_w[p].clone())
        .as_scalar();

    b *= signed_incidence::<F>(hg.flat_edge_sign[p]);
    if use_abs { b = b.abs(); }
    b
}