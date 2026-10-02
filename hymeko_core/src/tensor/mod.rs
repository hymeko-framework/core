//! Adapters between the canonical IR and derived tensor views.
//! Numeric storage, scalar contracts and kernels are owned by `hymeko_tensor`.

use crate::common::ids::{EdgeId, NodeId};
use hymeko_tensor::{common::Real, tensor_val::IncVal};
use std::marker::PhantomData;

pub mod tensor_val;

#[derive(Clone, Copy, Debug)]
pub struct TensorInc<F: Real, V: IncVal<F>> {
    pub e: EdgeId,
    pub n: NodeId,
    pub s: i8,
    pub w: V,
    /// Scalar marker for typed incidence values consumed by derived views.
    pub _pd: PhantomData<F>,
}

impl<F: Real, V: IncVal<F>> TensorInc<F, V> {
    /// Convenience constructor so callers don't need to materialise a
    /// `PhantomData<F>` value at every call-site.
    #[inline]
    pub fn new(e: EdgeId, n: NodeId, s: i8, w: V) -> Self {
        Self {
            e,
            n,
            s,
            w,
            _pd: PhantomData,
        }
    }
}
