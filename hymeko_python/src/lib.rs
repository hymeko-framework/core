//! `hymeko_python` — PyO3 bindings exposing the real HyMeKo parser and the
//! `hymeko_core` canonical `Ir` to Python as `import hymeko`.
//!
//! Read-only value views over the canonical `Ir`; no second IR, no editor IR, no
//! JSON intermediate, no dense graph, no CLI, no mutation/transaction API. The
//! `Model` owns the canonical `Ir` + the `Interner` needed to resolve names (via
//! `Arc<ModelInner>`); every entity handed to Python is a read-only view. See
//! `docs/python-architecture.md`.

mod arrays;
mod entities;
mod errors;
mod ids;
mod model;
mod pipeline;

use pyo3::prelude::*;

/// The `hymeko` Python module (the `#[pymodule]` fn name is the import name).
#[pymodule]
fn hymeko(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    errors::register(m)?;
    ids::register(m)?;
    entities::register(m)?;
    model::register(m)?;
    Ok(())
}
