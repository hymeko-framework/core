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
pub use value_updates::{ValueUpdate, ValueUpdateError, ValueUpdateWorkspace};

use super::module_store::{CompiledProgram, HymekoParser, ModuleLoadError, ModuleStore};
use super::source_provider::SourceProvider;
use crate::ir::canonical_hash::{Canon, CanonHashCfg, CanonicalIdentity};
use crate::ir::coherence::{
    CoherenceLimits, validate_hierarchy_index_coherence, validate_model_contract,
};
use crate::ir::incidence::{IncidenceError, IncidenceIndexData, IncidenceView};
use crate::ir::semantic_digest::{SemanticDigestState, SemanticModelDigest};
use crate::resolution::interner::Interner;
use crate::resolution::resolve::Index;
use crate::resolution::symbols::{SymbolError, SymbolUsage};
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};
use std::{collections::VecDeque, fmt};

/// Private evidence bound to an immutable program/interner pair. Only the
/// checked value-edit boundary may inherit it, and only while symbol IDs in
/// structural fields remain stable.
/// It retains no earlier program or string table.
#[derive(Clone, Copy)]
struct StructureValidation;

impl StructureValidation {
    fn check(
        ir: &crate::ir::ir::Ir,
        index: &Index,
        interner: &Interner,
    ) -> Result<Self, ValueUpdateError> {
        #[cfg(test)]
        STRUCTURE_CHECKS.with(|count| count.set(count.get() + 1));
        validate_hierarchy_index_coherence(ir, index, interner, CoherenceLimits::default())
            .map_err(|_| ValueUpdateError::InvalidModel)?;
        validate_model_contract(ir, interner).map_err(|_| ValueUpdateError::InvalidModel)?;
        Ok(Self)
    }
}

#[cfg(test)]
std::thread_local! {
    static STRUCTURE_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn take_structure_checks() -> usize {
    STRUCTURE_CHECKS.with(|count| count.replace(0))
}

/// An inseparable, read-only compiled program and symbol table.
///
/// Construction goes through the canonical compiler, validated snapshots or checked value updates.
/// No mutable program/interner
/// access or arbitrary pairing constructor is exposed.
pub struct CompiledModel {
    program: Arc<CompiledProgram>,
    interner: Interner,
    semantic: OnceLock<Arc<SemanticDigestState>>,
    symbol_usage: OnceLock<Result<SymbolUsage, SymbolError>>,
    validation: OnceLock<Result<StructureValidation, ValueUpdateError>>,
    /// The reverse-incidence index of `program.ir`, derived on first use and never
    /// serialized. Checked value updates change leaf values only, never arcs or
    /// declarations, so the models they derive share it.
    incidence: OnceLock<Result<Arc<IncidenceIndexData>, IncidenceError>>,
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
            semantic: OnceLock::new(),
            symbol_usage: OnceLock::new(),
            validation: OnceLock::new(),
            incidence: OnceLock::new(),
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
                idx: Arc::new(payload.index),
                ir: payload.ir.into_shared(),
                imports: Arc::new(payload.imports),
                canon,
            }),
            interner,
            semantic: OnceLock::new(),
            symbol_usage: OnceLock::new(),
            validation: OnceLock::new(),
            incidence: OnceLock::new(),
        }
    }

    fn structure_validation(&self) -> Result<StructureValidation, ValueUpdateError> {
        *self.validation.get_or_init(|| {
            StructureValidation::check(&self.program.ir, &self.program.idx, &self.interner)
        })
    }

    fn symbol_usage(&self) -> Result<&SymbolUsage, SymbolError> {
        self.symbol_usage
            .get_or_init(|| {
                SymbolUsage::new(
                    &self.program.ir,
                    &self.program.idx,
                    &self.program.imports,
                    &self.interner,
                )
            })
            .as_ref()
            .map_err(|&error| error)
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

    /// Create an equivalent model with copy-on-write declaration pages.
    ///
    /// Default compiled storage remains contiguous. This opt-in layout favors
    /// small edits: indexed writes copy directory handles and affected pages.
    /// Explicit Vec/slice access may materialize O(N) contiguous records, while
    /// ordinary accessors and iteration stream pages. Hashes, IDs, cached derived
    /// state and old readers are preserved. Conversion can copy all declarations.
    ///
    /// # Errors
    /// `page_size` must be a nonzero power of two, measured in records.
    pub fn with_declaration_pages(
        &self,
        page_size: usize,
    ) -> Result<Self, crate::ir::table::IrPageSizeError> {
        if !page_size.is_power_of_two() {
            return Err(crate::ir::table::IrPageSizeError(page_size));
        }
        let mut ir = self.program.ir.clone();
        ir.decl_nodes.share_pages(page_size)?;
        ir.annotations.share_pages(page_size)?;
        Ok(Self {
            program: Arc::new(CompiledProgram {
                root: self.program.root.clone(),
                idx: Arc::clone(&self.program.idx),
                ir,
                imports: Arc::clone(&self.program.imports),
                canon: CanonicalIdentity::from_parts(
                    self.program.canon.hash(),
                    self.program.canon.structure(),
                ),
            }),
            interner: self.interner.fork(),
            semantic: self
                .semantic
                .get()
                .cloned()
                .map_or_else(OnceLock::new, OnceLock::from),
            symbol_usage: self
                .symbol_usage
                .get()
                .cloned()
                .map_or_else(OnceLock::new, OnceLock::from),
            validation: self
                .validation
                .get()
                .copied()
                .map_or_else(OnceLock::new, OnceLock::from),
            incidence: self
                .incidence
                .get()
                .cloned()
                .map_or_else(OnceLock::new, OnceLock::from),
        })
    }

    /// Value-sensitive identity of this immutable snapshot. The first call builds
    /// derived contributions; repeated calls reuse them. Checked value updates
    /// rehash only edited declarations with the same encoder as the full digest.
    #[must_use]
    pub fn semantic_digest(&self) -> SemanticModelDigest {
        self.semantic_state().digest()
    }

    fn semantic_state(&self) -> &SemanticDigestState {
        self.semantic
            .get_or_init(|| Arc::new(SemanticDigestState::new(&self.program.ir, &self.interner)))
    }

    /// The program's incidences, over the reverse-incidence index this model derives
    /// once. Repeated calls, and the models checked value updates derive from this one,
    /// reuse that index: no copy of the IR, no rebuild. The view is zero-allocation.
    ///
    /// # Errors
    /// The IR's [`IncidenceError`], which is kept as well.
    // APPROVED-CORE-EDIT: model-incidence-cache (2026-09-30, user chat).
    pub fn incidences(&self) -> Result<IncidenceView<'_, '_>, IncidenceError> {
        let data = self
            .incidence
            .get_or_init(|| IncidenceIndexData::build(&self.program.ir).map(Arc::new))
            .as_ref()
            .map_err(|error| *error)?;
        Ok(IncidenceView::pair(&self.program.ir, data))
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

/// Caller-owned roots awaiting destruction on a caller-selected worker.
///
/// Capacity counts snapshots, not bytes. Live readers keep their roots queued.
/// `reclaim` bounds inspections, but destroying one model can still take time
/// proportional to its contents. Independent IR/metadata copies have their own
/// lifetimes. Dropping this queue performs ordinary teardown, not deferred GC.
pub struct SnapshotRetirement {
    roots: VecDeque<Arc<ExecutionSnapshot>>,
    limit: usize,
}

impl SnapshotRetirement {
    /// Reserve all queue slots. Zero capacity refuses retained publications.
    /// Allocation/size overflow is reported without constructing a queue.
    pub fn with_capacity(limit: usize) -> Result<Self, std::collections::TryReserveError> {
        let mut roots = VecDeque::new();
        roots.try_reserve_exact(limit)?;
        Ok(Self { roots, limit })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.roots.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Configured root limit, independent of allocator rounding.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.limit
    }

    /// Inspect at most `max_checks` queued roots, returning the number reclaimed.
    /// Run this on the thread where model destruction is acceptable. Blocked
    /// roots rotate to the back, so a live reader cannot starve later roots.
    pub fn reclaim(&mut self, max_checks: usize) -> usize {
        let checks = max_checks.min(self.roots.len());
        let mut reclaimed = 0;
        for _ in 0..checks {
            let root = self
                .roots
                .pop_front()
                .expect("bounded by initial queue length");
            match Arc::try_unwrap(root) {
                Ok(snapshot) => {
                    drop(snapshot);
                    reclaimed += 1;
                }
                Err(root) => self.roots.push_back(root),
            }
        }
        reclaimed
    }
}

/// A retained publication did not consume its candidate. The caller may retry
/// a full queue or send the recovered candidate to a worker for destruction.
pub enum RetainedPublicationError {
    RetirementFull(PreparedCompilation),
    Rejected {
        error: PublicationError,
        candidate: PreparedCompilation,
    },
}

impl RetainedPublicationError {
    #[must_use]
    pub fn into_candidate(self) -> PreparedCompilation {
        match self {
            Self::RetirementFull(candidate) | Self::Rejected { candidate, .. } => candidate,
        }
    }
}

impl fmt::Debug for RetainedPublicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RetirementFull(_) => f.write_str("RetirementFull"),
            Self::Rejected { error, .. } => f.debug_tuple("Rejected").field(error).finish(),
        }
    }
}

impl fmt::Display for RetainedPublicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RetirementFull(_) => f.write_str("snapshot retirement queue is full"),
            Self::Rejected { error, .. } => error.fmt(f),
        }
    }
}

impl std::error::Error for RetainedPublicationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RetirementFull(_) => None,
            Self::Rejected { error, .. } => Some(error),
        }
    }
}

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
        self.publish(prepared, drop).map_err(|(error, _)| error)
    }

    /// Publish while retaining the replaced root for explicit worker reclamation.
    ///
    /// Queue capacity is checked first. Full or rejected attempts return their
    /// candidate and do not change the current snapshot or retirement queue.
    /// The queue never grows during publication; call `reclaim` separately.
    #[allow(
        clippy::result_large_err,
        reason = "returning the owned candidate inline avoids an allocation on backpressure"
    )]
    pub fn commit_retaining(
        &self,
        prepared: PreparedCompilation,
        retirement: &mut SnapshotRetirement,
    ) -> Result<Arc<ExecutionSnapshot>, RetainedPublicationError> {
        if retirement.roots.len() == retirement.limit {
            return Err(RetainedPublicationError::RetirementFull(prepared));
        }
        self.publish(prepared, |previous| retirement.roots.push_back(previous))
            .map_err(|(error, candidate)| RetainedPublicationError::Rejected { error, candidate })
    }

    #[allow(
        clippy::result_large_err,
        reason = "candidate recovery must not add an error-path allocation"
    )]
    fn publish(
        &self,
        prepared: PreparedCompilation,
        replaced: impl FnOnce(Arc<ExecutionSnapshot>),
    ) -> Result<Arc<ExecutionSnapshot>, (PublicationError, PreparedCompilation)> {
        if !Arc::ptr_eq(&self.owner, &prepared.owner) {
            return Err((PublicationError::ForeignState, prepared));
        }
        let PreparedCompilation { owner, base, model } = prepared;
        let revision = base.revision.checked_add(1);
        // Allocation is outside the publication lock. The placeholder revision
        // cannot be published if checked_add failed; rejection order stays intact.
        let next = Arc::new(ExecutionSnapshot {
            revision: revision.unwrap_or(base.revision),
            model,
        });
        let result = self
            .current
            .write()
            .map_err(|_| PublicationError::Poisoned)
            .and_then(|mut current| {
                if !Arc::ptr_eq(&current, &base) {
                    return Err(PublicationError::StaleBase {
                        expected: base.revision,
                        actual: current.revision,
                    });
                }
                revision.ok_or(PublicationError::RevisionExhausted)?;
                Ok(std::mem::replace(&mut *current, Arc::clone(&next)))
            });
        // Every write guard is gone before replacement handling or recovery.
        match result {
            Ok(previous) => {
                replaced(previous);
                Ok(next)
            }
            Err(error) => {
                let model = Arc::try_unwrap(next)
                    .ok()
                    .expect("a rejected candidate was never shared")
                    .model;
                Err((error, PreparedCompilation { owner, base, model }))
            }
        }
    }
}

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::{
        ir::ir::ValueR, ir_stream::DecodeLimits, module_store::source_provider::MemProvider,
        writers::cbor_writer,
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
        assert_eq!(installed.program().imports.as_ref(), &imported);
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
            old.model().program().ir.decl_annotation(id).unwrap().value,
            Some(ValueR::Num(1.))
        );
        assert_eq!(
            state
                .snapshot()
                .unwrap()
                .model()
                .program()
                .ir
                .decl_annotation(id)
                .unwrap()
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
