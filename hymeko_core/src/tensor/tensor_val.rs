//! Conversion of canonical IR reference weights to numerical values.
use crate::ir::ir::{SignedRefR, ValueR};
use hymeko_tensor::{common::Real, tensor_val::IncVal};

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
