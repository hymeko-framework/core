//! Concurrent preparation and linearized, process-local IR publication.
//!
//! Workers use the existing compiler independently. Readers retain an immutable
//! snapshot containing both the compiled program and its matching interner. The
//! publication lock protects only acquisition/base checking/replacement, never
//! compilation or consumer computation. Scheduling and backpressure belong to
//! consumers: each preparation and retained snapshot consumes model-sized memory.
//!
//! Publication replaces complete models, prepared from source or checked leaf values.
//! This is not general topology-delta application, automatic merging,
//! distributed consensus, or a parallel rewrite of the compiler. Publication
//! revisions are local counters, unrelated to `ir_stream::SnapshotVersion`.
//! Concurrent candidates from one base compete: one commits; others are stale.
//! Completion order is not submission order; there is no latest-request-wins rule.

mod value_updates;
pub use value_updates::{ValueUpdate, ValueUpdateError};

use super::module_store::{CompiledProgram, HymekoParser, ModuleLoadError, ModuleStore};
use super::source_provider::SourceProvider;
use crate::ir::canonical_hash::{Canon, CanonHashCfg, CanonicalIdentity};
use crate::resolution::interner::Interner;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, RwLock};

/// An inseparable, read-only compiled program and symbol table.
///
/// Construction goes through the canonical compiler, validated snapshots or checked value updates.
/// No mutable program/interner
/// access or arbitrary pairing constructor is exposed.
pub struct CompiledModel {
    program: Arc<CompiledProgram>,
    interner: Interner,
}

impl CompiledModel {
    /// Compile in a fresh isolated store, then move its interner with the result.
    ///
    /// The provider must supply a coherent set of source bytes for this call;
    /// a live filesystem is not itself an atomic source snapshot. A frozen
    /// `MemProvider` is suitable. A fresh store avoids root-only cache reuse
    /// across updates, but cannot invalidate caches internal to a passed provider.
    /// Parser/provider callbacks run on the calling worker with no publication lock.
    /// Errors are those of the existing compilation boundary.
    pub fn compile<P: SourceProvider, R: HymekoParser>(
        provider: P,
        parser: R,
        root: &Path,
    ) -> Result<Self, ModuleLoadError> {
        let mut store = ModuleStore::new(provider, parser);
        let program = store.compile(root)?;
        Ok(Self {
            program,
            interner: store.it,
        })
    }

    /// Install a Core-validated canonical snapshot without parsing or lowering.
    /// Consumes the checked payload and its matching interner together; declaration
    /// IDs, imports, index and structural hash retain the decoder's identities; the
    /// semantic digest, which the wire does not carry, is computed here once. The module
    /// root stays workspace-relative and grants no source-provider access.
    // APPROVED-CORE-EDIT: ir-snapshot-install (2026-09-15, user chat).
    #[must_use]
    pub fn from_decoded_cbor(snapshot: crate::writers::cbor_writer::DecodedCborPayload) -> Self {
        let (payload, interner) = snapshot.into_parts();
        let canon =
            CanonicalIdentity::of(&Canon::new(&payload.ir, &interner), CanonHashCfg::default());
        // The decoder recomputed the structural hash from this IR before accepting it.
        debug_assert_eq!(canon.structure().0, payload.canon_hash);
        Self {
            program: Arc::new(CompiledProgram {
                root: super::module_store::ModuleKey(payload.root_path),
                idx: payload.index,
                ir: payload.ir,
                imports: payload.imports,
                canon,
            }),
            interner,
        }
    }

    /// The canonical compiler's result, borrowed for this model's lifetime.
    #[must_use]
    pub fn program(&self) -> &CompiledProgram {
        &self.program
    }

    /// The symbol table belonging to this exact program.
    #[must_use]
    pub fn interner(&self) -> &Interner {
        &self.interner
    }
}

/// A published model and local monotonic revision, held alive by readers' Arcs.
pub struct ExecutionSnapshot {
    revision: u64,
    model: CompiledModel,
}

impl ExecutionSnapshot {
    /// Local publication order. Compare only within the same execution state.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Immutable IR and symbol context used by all computations on this snapshot.
    #[must_use]
    pub fn model(&self) -> &CompiledModel {
        &self.model
    }
}

/// An opaque compilation tied to the exact base and execution state that prepared it.
/// Dropping it aborts preparation without changing the published state.
pub struct PreparedCompilation {
    owner: Arc<()>,
    base: Arc<ExecutionSnapshot>,
    model: CompiledModel,
}

/// A failed publication changes neither the current revision nor the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationError {
    /// The candidate belongs to a different execution state, even if revisions match.
    ForeignState,
    /// Another candidate committed since this candidate captured its base.
    StaleBase { expected: u64, actual: u64 },
    /// Publication refuses to wrap its local revision counter.
    RevisionExhausted,
    /// A writer panicked while holding the publication lock; no recovery is assumed.
    Poisoned,
}

impl fmt::Display for PublicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IR publication failed: {self:?}")
    }
}

impl std::error::Error for PublicationError {}

/// Failure to acquire the base or compile a candidate. Nothing was published.
#[derive(Debug)]
pub enum PrepareError {
    Publication(PublicationError),
    Compile(ModuleLoadError),
}

impl fmt::Display for PrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Publication(error) => error.fmt(f),
            Self::Compile(error) => write!(f, "IR preparation failed: {error:?}"),
        }
    }
}

impl std::error::Error for PrepareError {}

/// Process-local publication authority for concurrent workers and readers.
///
/// This uses a standard RwLock, not a lock-free or fairness-guaranteed primitive.
/// A consumer retains one acquired snapshot for an entire computation and tags
/// results with that snapshot's revision. Acquisition alone does not promise that
/// a delayed result is still current when it reaches another consumer.
pub struct ExecutionState {
    owner: Arc<()>,
    current: RwLock<Arc<ExecutionSnapshot>>,
}

impl ExecutionState {
    /// Start a new independent publication authority at revision zero.
    #[must_use]
    pub fn new(model: CompiledModel) -> Self {
        Self {
            owner: Arc::new(()),
            current: RwLock::new(Arc::new(ExecutionSnapshot { revision: 0, model })),
        }
    }

    /// Acquire a stable snapshot. The read lock is released before returning.
    /// Returns `Poisoned` if the publication lock was poisoned.
    pub fn snapshot(&self) -> Result<Arc<ExecutionSnapshot>, PublicationError> {
        self.current
            .read()
            .map(|current| Arc::clone(&current))
            .map_err(|_| PublicationError::Poisoned)
    }

    /// Capture the base, then compile independently of all readers and writers.
    ///
    /// Call concurrently from a bounded worker pool. Successful compilation does
    /// not guarantee later commit: the base may have become stale. No hidden retry
    /// or rebase occurs. See `CompiledModel::compile` for source consistency.
    pub fn prepare<P: SourceProvider, R: HymekoParser>(
        &self,
        provider: P,
        parser: R,
        root: &Path,
    ) -> Result<PreparedCompilation, PrepareError> {
        let base = self.snapshot().map_err(PrepareError::Publication)?;
        let model =
            CompiledModel::compile(provider, parser, root).map_err(PrepareError::Compile)?;
        Ok(PreparedCompilation {
            owner: Arc::clone(&self.owner),
            base,
            model,
        })
    }

    /// Atomically replace the current snapshot iff the candidate's exact base is current.
    ///
    /// The pointer replacement under the write lock is the linearization point.
    /// Foreign/stale candidates and counter overflow are rejected before replacement.
    /// The candidate is consumed on success or failure; callers explicitly prepare
    /// again if appropriate. Destruction of replaced/candidate models happens after
    /// lock release. Compilation errors and callback panics cannot poison this lock.
    pub fn commit(
        &self,
        prepared: PreparedCompilation,
    ) -> Result<Arc<ExecutionSnapshot>, PublicationError> {
        if !Arc::ptr_eq(&self.owner, &prepared.owner) {
            return Err(PublicationError::ForeignState);
        }
        let mut current = self
            .current
            .write()
            .map_err(|_| PublicationError::Poisoned)?;
        if !Arc::ptr_eq(&current, &prepared.base) {
            return Err(PublicationError::StaleBase {
                expected: prepared.base.revision,
                actual: current.revision,
            });
        }
        let revision = current
            .revision
            .checked_add(1)
            .ok_or(PublicationError::RevisionExhausted)?;
        let next = Arc::new(ExecutionSnapshot {
            revision,
            model: prepared.model,
        });
        let previous = std::mem::replace(&mut *current, Arc::clone(&next));
        drop(current);
        drop(previous);
        Ok(next)
    }
}

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::{
        ir::ir::ValueR, ir_stream::DecodeLimits,
        module_store::source_provider::MemProvider, writers::cbor_writer,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn validated_snapshot_install_moves_identity_and_publishes_without_parser_calls() {
        let calls = AtomicUsize::new(0);
        let original=CompiledModel::compile(MemProvider::default()
            .with_file("types.hymeko", "description {} types { body {} }")
            .with_file("scene.hymeko", "description { @\"types.hymeko\"; using types as t; } room { marker: t.body { gain 1.0; } }"),
            tests::CallbackParser(|| { calls.fetch_add(1, Ordering::SeqCst); }), Path::new("scene.hymeko")).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let bytes =
            cbor_writer::encode_compiled(Path::new(""), original.program(), original.interner())
                .unwrap();
        let decoded = cbor_writer::decode(&bytes, &DecodeLimits::default()).unwrap();
        let nodes = decoded.ir().decl_nodes.as_ptr();
        let imported = decoded.payload().imports.clone();
        assert!(!imported.is_empty());
        let digest = Canon::new(decoded.ir(), decoded.interner()).hash();
        let installed = CompiledModel::from_decoded_cbor(decoded);
        assert_eq!(installed.program().ir.decl_nodes.as_ptr(), nodes);
        assert_eq!(installed.program().root.0, Path::new("scene.hymeko"));
        assert_eq!(installed.program().imports, imported);
        assert_eq!(installed.program().canon, original.program().canon);
        assert_eq!(
            Canon::new(&installed.program().ir, installed.interner()).hash(),
            digest
        );
        assert_eq!(
            cbor_writer::encode_compiled(Path::new(""), installed.program(), installed.interner())
                .unwrap(),
            bytes
        );
        let id = installed
            .program()
            .ir
            .decls()
            .find(|(_, d)| installed.interner().resolve(d.name) == "gain")
            .unwrap()
            .0;
        let state = ExecutionState::new(installed);
        let old = state.snapshot().unwrap();
        let next = state
            .prepare_model_at(
                &old,
                old.model()
                    .with_value_updates(&[ValueUpdate::Set {
                        declaration: id,
                        value: ValueR::Num(3.),
                    }])
                    .unwrap(),
            )
            .unwrap();
        let competing = state
            .prepare_model_at(
                &old,
                CompiledModel::from_decoded_cbor(
                    cbor_writer::decode(&bytes, &DecodeLimits::default()).unwrap(),
                ),
            )
            .unwrap();
        assert_eq!(state.commit(next).unwrap().revision(), 1);
        assert_eq!(
            state.commit(competing).err(),
            Some(PublicationError::StaleBase {
                expected: 0,
                actual: 1
            })
        );
        assert_eq!(
            old.model().program().ir.decl_node(id).unwrap().anno.value,
            Some(ValueR::Num(1.))
        );
        assert_eq!(
            state
                .snapshot()
                .unwrap()
                .model()
                .program()
                .ir
                .decl_node(id)
                .unwrap()
                .anno
                .value,
            Some(ValueR::Num(3.))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        for invalid in [&b"invalid"[..], &bytes[..bytes.len() / 2]] {
            assert!(cbor_writer::decode(invalid, &DecodeLimits::default()).is_err());
        }
        assert_eq!(state.snapshot().unwrap().revision(), 1);
    }
}
