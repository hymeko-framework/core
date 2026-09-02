//! The real parse → resolve → lower → validate pipeline, and the owned bundle the
//! Python `Model` holds. Uses the production `ModuleStore::compile` path (imports +
//! `const` handled; never panics). The heavy work runs with the GIL released
//! (`allow_threads`); errors are returned as a `Send` type and mapped to typed
//! Python exceptions with the GIL re-held (mission §5, §6 GIL).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use hymeko::common::ids::{DeclId, SymId};
use hymeko::common::pathkey::PathKey;
use hymeko::ir::coherence::{CoherenceLimits, validate_hierarchy_index_coherence};
use hymeko::ir::ir::Ir;
use hymeko::module_store::module_store::{CompiledProgram, ModuleLoadError, ModuleStore};
use hymeko::module_store::source_provider::{MemProvider, StdFsProvider};
use hymeko::resolution::interner::Interner;
use hymeko::util::real_parser::RealParser;
use pyo3::prelude::*;

use crate::errors::{HyMeKoIoError, HyMeKoValidationError, parse_stage_error};

/// Process-unique model-instance token generator. This is the one narrow global
/// permitted by CLAUDE.md §6.5#11 (a thread-safe monotonic id generator — no
/// runtime-config state, no hidden coupling, order-independent). It backs the
/// cross-model id guard.
static NEXT_MODEL_TOKEN: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_token() -> u64 {
    NEXT_MODEL_TOKEN.fetch_add(1, Ordering::Relaxed)
}

/// The owned canonical bundle behind a Python `Model`: the `Interner` (for names),
/// the `Arc<CompiledProgram>` (owns `Ir` + `Index` + precomputed canonical hash),
/// a precomputed `DeclId → canonical path`, and the model-instance token. The
/// `ModuleStore` is dropped after build — only the `Ir`/`Interner`/`Index` are kept
/// (mission §1). Shared via `Arc<ModelInner>` by the `Model` and all its views.
pub(crate) struct ModelInner {
    pub(crate) token: u64,
    interner: Interner,
    compiled: Arc<CompiledProgram>,
    did_to_path: Vec<Option<PathKey>>,
}

impl ModelInner {
    pub(crate) fn ir(&self) -> &Ir {
        &self.compiled.ir
    }

    /// Resolve a `SymId` to its interned text (`SymId::new(0)` on anonymous decls
    /// resolves to the empty string).
    pub(crate) fn name(&self, s: SymId) -> &str {
        self.interner.resolve(s)
    }

    /// The dotted canonical path of a declaration (`a.b.c`), or `None` if the decl
    /// has no indexed path.
    pub(crate) fn canonical_path(&self, d: DeclId) -> Option<String> {
        let pk = self.did_to_path.get(d.raw())?.as_ref()?;
        Some(
            pk.0.iter()
                .map(|&s| self.interner.resolve(s))
                .collect::<Vec<_>>()
                .join("."),
        )
    }

    /// The 32-byte canonical program hash as lower-hex (bit-identical to the Rust
    /// `canonical_program_hash` oracle — it is `CompiledProgram.canon_hash`).
    pub(crate) fn canonical_hash_hex(&self) -> String {
        self.compiled
            .canon_hash
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// A `Send` build error (no Python object), converted to a typed `PyErr` once the
/// GIL is re-held.
enum BuildError {
    ModuleLoad(ModuleLoadError),
    Validation(String),
}

impl BuildError {
    fn into_pyerr(self) -> PyErr {
        match self {
            BuildError::ModuleLoad(ModuleLoadError::Io(diag)) => HyMeKoIoError::new_err(format!(
                "{}: {} ({})",
                diag.op,
                diag.path.display(),
                diag.err
            )),
            BuildError::ModuleLoad(ModuleLoadError::Cycle { stack }) => {
                HyMeKoIoError::new_err(format!(
                    "import cycle: {}",
                    stack
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(" -> ")
                ))
            }
            BuildError::ModuleLoad(ModuleLoadError::Parse(msg)) => parse_stage_error(&msg),
            BuildError::Validation(report) => {
                HyMeKoValidationError::new_err(format!("coherence findings: {report}"))
            }
        }
    }
}

fn finish(
    py: Python<'_>,
    build: impl FnOnce() -> Result<(Arc<CompiledProgram>, Interner), BuildError> + Send,
) -> PyResult<ModelInner> {
    let (compiled, interner) = py
        .allow_threads(|| {
            let (compiled, interner) = build()?;
            validate_hierarchy_index_coherence(
                &compiled.ir,
                &compiled.idx,
                &interner,
                CoherenceLimits::default(),
            )
            .map_err(|r| BuildError::Validation(format!("{r:?}")))?;
            Ok((compiled, interner))
        })
        .map_err(BuildError::into_pyerr)?;

    let did_to_path = build_did_to_path(&compiled);
    Ok(ModelInner {
        token: next_token(),
        interner,
        compiled,
        did_to_path,
    })
}

/// Parse in-memory source (single unit; imports would need a provider that can
/// serve them, so `parse` uses a one-file `MemProvider`).
pub(crate) fn build_from_source(
    py: Python<'_>,
    source: &str,
    filename: &str,
) -> PyResult<ModelInner> {
    let source = source.to_owned();
    let filename = PathBuf::from(filename);
    finish(py, move || {
        let fs = MemProvider::default().with_file(filename.clone(), source);
        let mut store = ModuleStore::new(fs, RealParser);
        let compiled = store.compile(&filename).map_err(BuildError::ModuleLoad)?;
        let interner = std::mem::take(&mut store.it);
        Ok((compiled, interner))
    })
}

/// Load and compile a `.hymeko` file, resolving `@"other.hymeko"` includes against
/// the real filesystem.
pub(crate) fn build_from_file(py: Python<'_>, path: &Path) -> PyResult<ModelInner> {
    let path = path.to_owned();
    finish(py, move || {
        let mut store = ModuleStore::new(StdFsProvider::new(), RealParser);
        let compiled = store.compile(&path).map_err(BuildError::ModuleLoad)?;
        let interner = std::mem::take(&mut store.it);
        Ok((compiled, interner))
    })
}

fn build_did_to_path(compiled: &CompiledProgram) -> Vec<Option<PathKey>> {
    let mut v: Vec<Option<PathKey>> = vec![None; compiled.ir.decl_count()];
    for (pk, &d) in compiled.idx.by_path.iter() {
        let i = d.raw();
        if i < v.len() && v[i].is_none() {
            v[i] = Some(pk.clone());
        }
    }
    v
}
