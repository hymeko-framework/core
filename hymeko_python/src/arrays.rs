//! Batch incidence export (mission §6). Arc-major sparse SoA read directly from
//! `ir.arcs` — a single bulk copy per array into an immutable `PyBytes`, exposed as
//! a typed `memoryview`. **No per-incidence Python object** is allocated on this
//! path.
//!
//! Representation choice: (A) `list`/`tuple` allocates one Python int per element
//! and is the slow baseline; (C) true zero-copy over the Rust `Vec` needs a
//! buffer-protocol object with parent-retention and is deferred (no undocumented
//! `unsafe`). This is (B): one bulk `memcpy` into `PyBytes`; the `memoryview` over
//! immutable bytes is safe and needs no parent-retention. Overhead vs the Rust-only
//! build is measured in the benchmark.
//!
//! Weight policy (explicit, documented): `weight_scalars[k]` is the product of the
//! `ValueR::Num` components of incidence `k`'s weight list (a nested `List` is
//! flattened one level); no weights, or all-non-numeric, yields `1.0`. This mirrors
//! the core `scalar_weight` policy without depending on the (unpromoted) incidence
//! module.

use hymeko::ir::ir::{SignedRefR, ValueR};
use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::pipeline::ModelInner;

fn scalar_weight(sref: &SignedRefR) -> f64 {
    match &sref.atom().weights {
        None => 1.0,
        Some(ws) => {
            let mut prod = 1.0;
            let mut any = false;
            for w in ws {
                match w {
                    ValueR::Num(x) => {
                        prod *= *x;
                        any = true;
                    }
                    ValueR::List(inner) => {
                        for v in inner {
                            if let ValueR::Num(x) = v {
                                prod *= *x;
                                any = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
            if any { prod } else { 1.0 }
        }
    }
}

/// Arc-major sparse-SoA incidence arrays. Each getter returns a typed, read-only
/// `memoryview` over an immutable bulk-copied buffer (built once at construction).
#[pyclass(frozen, module = "hymeko")]
pub struct IncidenceArrays {
    #[pyo3(get)]
    n_arcs: usize,
    #[pyo3(get)]
    nnz: usize,
    arc_offsets: Py<PyBytes>,    // u32, len n_arcs+1
    targets: Py<PyBytes>,        // u32, len nnz
    signs: Py<PyBytes>,          // i8, len nnz
    slots: Py<PyBytes>,          // u32, len nnz
    weight_scalars: Py<PyBytes>, // f64, len nnz
}

impl IncidenceArrays {
    pub(crate) fn build(py: Python<'_>, inner: &ModelInner) -> PyResult<Self> {
        let ir = inner.ir();
        let n_arcs = ir.arc_count();
        let nnz: usize = ir.arcs.iter().map(|a| a.refs.len()).sum();

        // A SINGLE pass over the (pointer-chasing) `SignedRefR` structs fills five
        // byte buffers; each is then bulk-copied once into an immutable `PyBytes`.
        // Measured (§6.5#18): the cost is dominated by the ref-struct traversal, not
        // the copy — five separate `PyBytes::new_with` passes (one per array, no
        // intermediate `Vec`) measured *slower* (5× traversal) than one pass + one
        // memcpy per array, so the single pass is kept.
        let mut offs: Vec<u8> = Vec::with_capacity((n_arcs + 1) * 4);
        let mut targets: Vec<u8> = Vec::with_capacity(nnz * 4);
        let mut signs: Vec<u8> = Vec::with_capacity(nnz);
        let mut slots: Vec<u8> = Vec::with_capacity(nnz * 4);
        let mut weights: Vec<u8> = Vec::with_capacity(nnz * 8);

        let mut acc: u32 = 0;
        offs.extend_from_slice(&acc.to_le_bytes());
        for arc in &ir.arcs {
            for (slot, sref) in arc.refs.iter().enumerate() {
                targets.extend_from_slice(&(sref.target().raw() as u32).to_le_bytes());
                signs.push(sref.sign() as u8);
                slots.extend_from_slice(&(slot as u32).to_le_bytes());
                weights.extend_from_slice(&scalar_weight(sref).to_le_bytes());
            }
            acc += arc.refs.len() as u32;
            offs.extend_from_slice(&acc.to_le_bytes());
        }

        Ok(Self {
            n_arcs,
            nnz,
            arc_offsets: PyBytes::new(py, &offs).unbind(),
            targets: PyBytes::new(py, &targets).unbind(),
            signs: PyBytes::new(py, &signs).unbind(),
            slots: PyBytes::new(py, &slots).unbind(),
            weight_scalars: PyBytes::new(py, &weights).unbind(),
        })
    }
}

fn typed_mv(py: Python<'_>, bytes: &Py<PyBytes>, fmt: &str) -> PyResult<Py<PyAny>> {
    let memoryview = py.import("builtins")?.getattr("memoryview")?;
    let mv = memoryview.call1((bytes.bind(py),))?;
    Ok(mv.call_method1("cast", (fmt,))?.unbind())
}

#[pymethods]
impl IncidenceArrays {
    /// `u32[n_arcs + 1]` CSR offsets: arc `i`'s incidences are `[offsets[i], offsets[i+1])`.
    #[getter]
    fn arc_offsets(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        typed_mv(py, &self.arc_offsets, "I")
    }
    /// `u32[nnz]` incidence targets (`DeclId` raws), arc-major.
    #[getter]
    fn targets(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        typed_mv(py, &self.targets, "I")
    }
    /// `i8[nnz]` signs (`+1` / `-1` / `0`), arc-major.
    #[getter]
    fn signs(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        typed_mv(py, &self.signs, "b")
    }
    /// `u32[nnz]` ref slots (the exact incidence coordinate within its arc).
    #[getter]
    fn slots(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        typed_mv(py, &self.slots, "I")
    }
    /// `f64[nnz]` scalar weights (product of `Num` components; `1.0` if none).
    #[getter]
    fn weight_scalars(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        typed_mv(py, &self.weight_scalars, "d")
    }
    fn __repr__(&self) -> String {
        format!("IncidenceArrays(n_arcs={}, nnz={})", self.n_arcs, self.nnz)
    }
}

#[cfg(test)]
mod tests {
    use super::scalar_weight;
    use hymeko::common::ids::{DeclId, SymId};
    use hymeko::ir::ir::{AnnoR, RefAtomR, SignedRefR, ValueR};

    fn plus(weights: Option<Vec<ValueR>>) -> SignedRefR {
        SignedRefR::Plus(RefAtomR {
            target: DeclId::new(0),
            anno: AnnoR::default(),
            weights,
        })
    }

    #[test]
    fn scalar_weight_policy() {
        assert_eq!(scalar_weight(&plus(None)), 1.0);
        assert_eq!(scalar_weight(&plus(Some(vec![]))), 1.0);
        assert_eq!(
            scalar_weight(&plus(Some(vec![ValueR::Num(2.0), ValueR::Num(3.0)]))),
            6.0
        );
        // nested list flattened one level
        assert_eq!(
            scalar_weight(&plus(Some(vec![ValueR::List(vec![
                ValueR::Num(2.0),
                ValueR::Num(4.0)
            ])]))),
            8.0
        );
        // non-numeric ignored; all-non-numeric -> 1.0
        assert_eq!(
            scalar_weight(&plus(Some(vec![ValueR::Str(SymId::new(0))]))),
            1.0
        );
    }
}
