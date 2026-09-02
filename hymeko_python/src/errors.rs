//! Typed Python exceptions for every failure mode. Nothing crosses the FFI
//! boundary as a Rust panic: parse / resolve / lower / validate / IO / id errors
//! are all mapped to these classes (mission §5).

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

create_exception!(
    hymeko,
    HyMeKoError,
    PyException,
    "Base class for every HyMeKo error."
);
create_exception!(
    hymeko,
    HyMeKoSyntaxError,
    HyMeKoError,
    "The parser rejected the source."
);
create_exception!(
    hymeko,
    HyMeKoResolutionError,
    HyMeKoError,
    "Name resolution / const / index / using-alias failure."
);
create_exception!(
    hymeko,
    HyMeKoLoweringError,
    HyMeKoError,
    "Lowering the AST to the canonical IR failed."
);
create_exception!(
    hymeko,
    HyMeKoValidationError,
    HyMeKoError,
    "The lowered IR failed hierarchy/index coherence validation."
);
create_exception!(
    hymeko,
    HyMeKoIoError,
    HyMeKoError,
    "File read / module load / import-cycle failure."
);
create_exception!(
    hymeko,
    HyMeKoIdError,
    HyMeKoError,
    "An id was used against the wrong model, wrong domain, or was out of range."
);

/// The pipeline stage a `ModuleLoadError::Parse(msg)` string came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Syntax,
    Resolution,
    Lowering,
    Io,
}

/// Classify a `ModuleLoadError::Parse(msg)` by the exact prefix `compile` tags each
/// stage with (`module_store.rs`): `const:` / `index ` / `using alias` → resolution,
/// `lower failed` → lowering, `dep missing` / `root missing` → io, anything else is
/// the raw parser message → syntax. Pure (unit-tested).
pub(crate) fn classify_parse_stage(msg: &str) -> Stage {
    if msg.starts_with("const:") || msg.starts_with("index ") || msg.starts_with("using alias") {
        Stage::Resolution
    } else if msg.starts_with("lower failed") {
        Stage::Lowering
    } else if msg.starts_with("dep missing") || msg.starts_with("root missing") {
        Stage::Io
    } else {
        Stage::Syntax
    }
}

/// Map a `Parse(msg)` to its typed exception via [`classify_parse_stage`].
pub(crate) fn parse_stage_error(msg: &str) -> PyErr {
    let owned = msg.to_owned();
    match classify_parse_stage(msg) {
        Stage::Resolution => HyMeKoResolutionError::new_err(owned),
        Stage::Lowering => HyMeKoLoweringError::new_err(owned),
        Stage::Io => HyMeKoIoError::new_err(owned),
        Stage::Syntax => HyMeKoSyntaxError::new_err(owned),
    }
}

/// A cross-model id use.
pub(crate) fn cross_model(kind: &str) -> PyErr {
    HyMeKoIdError::new_err(format!(
        "{kind} belongs to a different model instance (ids are not portable between models)"
    ))
}

/// An id whose raw index is outside the model's domain.
pub(crate) fn out_of_range(kind: &str, raw: usize, count: usize) -> PyErr {
    HyMeKoIdError::new_err(format!("{kind} {raw} is out of range (model has {count})"))
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add("HyMeKoError", py.get_type::<HyMeKoError>())?;
    m.add("HyMeKoSyntaxError", py.get_type::<HyMeKoSyntaxError>())?;
    m.add(
        "HyMeKoResolutionError",
        py.get_type::<HyMeKoResolutionError>(),
    )?;
    m.add("HyMeKoLoweringError", py.get_type::<HyMeKoLoweringError>())?;
    m.add(
        "HyMeKoValidationError",
        py.get_type::<HyMeKoValidationError>(),
    )?;
    m.add("HyMeKoIoError", py.get_type::<HyMeKoIoError>())?;
    m.add("HyMeKoIdError", py.get_type::<HyMeKoIdError>())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Stage, classify_parse_stage};

    #[test]
    fn classifies_each_compile_stage_by_prefix() {
        assert_eq!(
            classify_parse_stage("UnrecognizedToken { .. }"),
            Stage::Syntax
        );
        assert_eq!(
            classify_parse_stage("const: undefined ref x"),
            Stage::Resolution
        );
        assert_eq!(
            classify_parse_stage("index root failed: UnresolvedRef"),
            Stage::Resolution
        );
        assert_eq!(
            classify_parse_stage("using alias failed: x"),
            Stage::Resolution
        );
        assert_eq!(
            classify_parse_stage("lower failed: MissingDecl"),
            Stage::Lowering
        );
        assert_eq!(classify_parse_stage("dep missing: a.hymeko"), Stage::Io);
        assert_eq!(classify_parse_stage("root missing after load"), Stage::Io);
    }
}
