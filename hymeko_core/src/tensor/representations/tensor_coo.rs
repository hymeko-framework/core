use crate::common::ids::{EdgeId, NodeId};
use crate::tensor::common::Real;
use crate::tensor::tensor_val::IncVal;
use std::marker::PhantomData;

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct CooEntry<F: Real> {
    pub k: usize,
    pub i: usize,
    pub j: usize,
    pub v: F,
}

pub struct CooSoa<F: Real> {
    pub num_slices: usize,
    pub dim_i: usize,
    pub dim_j: usize,
    pub k: Vec<usize>,
    pub i: Vec<usize>,
    pub j: Vec<usize>,
    pub v: Vec<F>,
}

/// Sparse 3D tensor in COO form (k, i, j, value).
/// Intended as an intermediate export format for JAX/PyTorch.
///
/// - `k` selects the slice (e.g., edge index / hyperedge id).
/// - `i`, `j` are row/col indices inside the slice.
/// - `v` is the weight/value (usually f32).
#[derive(Clone, Debug, Default)]
pub struct TensorCoo<F: Real> {
    pub num_slices: usize,
    pub dim_i: usize,
    pub dim_j: usize,
    pub entries: Vec<CooEntry<F>>,
}

#[derive(Clone, Copy, Debug)]
pub struct TensorBuildCfg {
    pub default_arc_weight: f32,
    pub default_edge_weight: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct TensorInc<F: Real, V: IncVal<F>> {
    pub e: EdgeId,
    pub n: NodeId,
    pub s: i8,
    pub w: V,
    /// Phantom `F` marker. Public so `hymeko_hnn` (which lives in a
    /// sibling crate since 2026-04-18) can construct the value — see
    /// `hymeko_hnn::traversal::hypergraphview::HyperGraphView::from_ir`.
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

impl Default for TensorBuildCfg {
    fn default() -> Self {
        Self {
            default_arc_weight: 1.0,
            default_edge_weight: 1.0,
        }
    }
}
impl<F: Real> TensorCoo<F> {
    pub fn new(num_slices: usize, dim_i: usize, dim_j: usize) -> Self {
        Self {
            num_slices,
            dim_i,
            dim_j,
            entries: Vec::new(),
        }
    }

    #[inline(always)]
    pub fn with_meta(num_slices: usize, dim_i: usize, dim_j: usize) -> Self {
        Self {
            num_slices,
            dim_i,
            dim_j,
            entries: Vec::new(),
        }
    }

    /// Push one non-zero entry (k, i, j, value).
    #[inline(always)]
    pub fn push(&mut self, k: usize, i: usize, j: usize, v: F) {
        self.entries.push(CooEntry { k, i, j, v });
    }

    #[inline(always)]
    pub fn reserve(&mut self, n: usize) {
        self.entries.reserve(n);
    }

    pub fn entry(&self, t: usize) -> &CooEntry<F> {
        &self.entries[t]
    }

    pub fn iter(&self) -> std::slice::Iter<'_, CooEntry<F>> {
        self.entries.iter()
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn into_soa(self) -> CooSoa<F> {
        let n = self.entries.len();
        let mut k = Vec::with_capacity(n);
        let mut i = Vec::with_capacity(n);
        let mut j = Vec::with_capacity(n);
        let mut v = Vec::with_capacity(n);

        for e in self.entries {
            k.push(e.k);
            i.push(e.i);
            j.push(e.j);
            v.push(e.v);
        }

        CooSoa {
            num_slices: self.num_slices,
            dim_i: self.dim_i,
            dim_j: self.dim_j,
            k,
            i,
            j,
            v,
        }
    }
}
