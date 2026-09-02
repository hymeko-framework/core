//! Typed, model-scoped ids (mission §4). Each of `DeclId` / `NodeId` / `EdgeId` /
//! `HyperArcId` is a distinct Python type that carries a **model-instance token**:
//! they are not interchangeable across domains (distinct types) or across models
//! (the token). The raw `usize` is not a public Python contract — there is no
//! value getter; it appears only in `repr` for diagnostics. Pickle/wire stability
//! is not promised. Migration to generational canonical ids replaces the
//! `(raw, token)` pair with a `(generation, index, token)` triple behind the same
//! opaque type — no public-surface change (see `docs/python-architecture.md`).

use pyo3::prelude::*;

use crate::errors::{cross_model, out_of_range};

macro_rules! define_id {
    ($rust:ident, $label:literal) => {
        #[pyclass(frozen, eq, hash, name = $label, module = "hymeko")]
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $rust {
            pub(crate) raw: usize,
            pub(crate) token: u64,
        }

        #[pymethods]
        impl $rust {
            fn __repr__(&self) -> String {
                format!(concat!($label, "({})"), self.raw)
            }
        }

        impl $rust {
            pub(crate) fn new(raw: usize, token: u64) -> Self {
                Self { raw, token }
            }

            /// Validate the id belongs to `token`'s model and indexes `< count`.
            pub(crate) fn checked(&self, token: u64, count: usize) -> PyResult<usize> {
                if self.token != token {
                    return Err(cross_model($label));
                }
                if self.raw >= count {
                    return Err(out_of_range($label, self.raw, count));
                }
                Ok(self.raw)
            }
        }
    };
}

define_id!(DeclId, "DeclId");
define_id!(NodeId, "NodeId");
define_id!(EdgeId, "EdgeId");
define_id!(HyperArcId, "HyperArcId");

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<DeclId>()?;
    m.add_class::<NodeId>()?;
    m.add_class::<EdgeId>()?;
    m.add_class::<HyperArcId>()?;
    Ok(())
}
