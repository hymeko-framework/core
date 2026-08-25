use crate::ir::ir::{SignedRefR, ValueR};
use crate::tensor::aggregation::{AggCfg, agg_weight};
use crate::tensor::common::Real;

pub trait IncVal<F>: Clone + Send + Sync
where
    F: Real,
{
    fn zero() -> Self;
    /// default value when no explicit weights exist
    fn one() -> Self;

    /// multiply by scalar (arc weight, edge weight, etc.)
    fn scale(&self, k: F) -> Self;

    /// aggregate two values (same (e,n) key)
    fn agg(cfg: &AggCfg, a: &Self, b: &Self) -> Self;

    /// for degree / normalization: return a nonnegative scalar measure
    fn degree_mass(&self) -> F;

    /// Convert to f32 for final output (e.g., for GNN input)
    fn as_scalar(&self) -> F;
}

// Skalár eset: V = F
impl<F: Real> IncVal<F> for F {
    #[inline]
    fn zero() -> Self {
        F::zero()
    }
    #[inline]
    fn one() -> Self {
        F::one()
    }
    #[inline]
    fn scale(&self, k: F) -> Self {
        *self * k
    }
    #[inline]
    fn agg(cfg: &AggCfg, a: &Self, b: &Self) -> Self {
        agg_weight::<F>(cfg, *a, *b) // ezt is generikussá kell tenni (lásd lent)
    }
    #[inline]
    fn degree_mass(&self) -> F {
        self.abs()
    }
    #[inline]
    fn as_scalar(&self) -> F {
        *self
    }
}

// APPROVED-CORE-EDIT: extract-ref-weight-noalloc (2026-08-24, user chat).
//
// The previous implementation collected every numeric weight into a temporary
// `Vec<F>` purely to ask three questions about it: is it empty, does it have
// exactly one element, and what is its left-to-right sum. Because
// `Vec::new()` allocates on first push, that was **one heap allocation per
// incidence** on the `HyperGraphView::from_ir` path — measured at exactly
// 1.000 allocations/incidence (1 642 278 allocations for 1 642 260 incidences
// at the large workload), and 36-75 % of total construction time.
//
// The three questions are answered by a running count, the first value, and a
// running sum, so the `Vec` is not needed at all. The rewrite is a pure
// strength reduction: same values, same order, no allocation.

/// Project a reference's optional weight list to a single scalar.
///
/// # Postconditions
/// * `None`, an empty list, or a list containing no `ValueR::Num` at the two
///   inspected levels yields `F::one()` — **not** `F::zero()`.
/// * Exactly one numeric value is returned **verbatim**, not folded. This
///   matters: `F::zero() + x` is not bitwise `x` when `x` is `-0.0`, so
///   collapsing this case into the general fold would be an observable change.
/// * Two or more numeric values yield `zero + v0 + v1 + ...`, folded
///   left-to-right in list order. Floating-point addition is neither
///   associative nor commutative, so the order is part of the contract.
/// * `ValueR::List` is descended **exactly one level**; a `List` nested inside
///   a `List` contributes nothing. Non-numeric variants are skipped.
///
/// # Invariants
/// Allocation-free. Every branch above is pinned by
/// `tests/test_tensor_representations/test_weight_extraction.rs`, which was
/// written against the previous implementation and passes unchanged.
#[inline(always)]
pub fn extract_ref_weight_scalar<F: Real>(weights: &Option<Vec<ValueR>>) -> F {
    let Some(ws) = weights else {
        return F::one();
    };

    // `count` distinguishes the none / single / many cases the old `Vec` length
    // used to answer; `first` preserves the verbatim single-value result;
    // `sum` reproduces the old `fold(zero, +)` exactly, including starting at
    // zero rather than at the first element.
    let mut count: usize = 0;
    let mut first = F::zero();
    let mut sum = F::zero();

    let mut take = |x: f64| {
        let value = F::from_other(x);
        count += 1;
        if count == 1 {
            first = value;
        }
        sum += value;
    };

    for v in ws {
        match v {
            ValueR::Num(x) => take(*x),
            ValueR::List(xs) => {
                for vv in xs {
                    if let ValueR::Num(x) = vv {
                        take(*x);
                    }
                }
            }
            _ => {}
        }
    }

    match count {
        0 => F::one(),
        1 => first,
        _ => sum,
    }
}

pub trait RefValueExtractor<F: Real, V: IncVal<F>> {
    fn value_of(&self, r: &SignedRefR) -> V;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScalarWeightExtractor;

impl<F: Real> RefValueExtractor<F, F> for ScalarWeightExtractor {
    #[inline(always)]
    fn value_of(&self, r: &SignedRefR) -> F {
        match r {
            SignedRefR::Plus(a) | SignedRefR::Minus(a) | SignedRefR::Neutral(a) => {
                extract_ref_weight_scalar(&a.weights)
            }
        }
    }
}

pub trait EdgeWeight<V, F>: Clone + Send + Sync
where
    V: IncVal<F>,
    F: Real,
{
    fn one() -> Self; // “1”
    fn scale(&self, x: V) -> V; // ha kell: edge-súlyozás

    fn apply_to(&self, x: V) -> V;
}

#[derive(Clone, Copy, Default)]
pub struct EdgeWScalar<F: Real>(pub F);

impl<F: Real> EdgeWeight<F, F> for EdgeWScalar<F> {
    fn one() -> Self {
        Self(F::one())
    }
    fn scale(&self, x: F) -> F {
        self.0 * x
    }

    #[inline(always)]
    fn apply_to(&self, x: F) -> F {
        self.0 * x
    }
}

pub trait Dot<Rhs> {
    type Out;
    fn dot(self, rhs: Rhs) -> Self::Out;
}

// f32 · f32 -> f32
impl Dot<f32> for f32 {
    type Out = f32;
    #[inline(always)]
    fn dot(self, rhs: f32) -> f32 {
        self * rhs
    }
}
