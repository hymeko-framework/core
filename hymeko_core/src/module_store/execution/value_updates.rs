//! Checked leaf-value updates to a retained canonical compiled model.
//! No parser, AST lowering, declaration allocation or link rewriting occurs here.
use super::{
    CompiledModel, CompiledProgram, ExecutionSnapshot, ExecutionState, PreparedCompilation,
    PublicationError, StructureValidation,
};
use crate::{
    common::ids::DeclId,
    ir::{
        canonical_hash::CanonicalIdentity,
        ir::{DeclKind, LeafValueEdit, ValueR},
        semantic_digest::SemanticDelta,
    },
    resolution::{
        interner::Interner,
        symbols::{SymbolDelta, SymbolUpdateError},
    },
};
use std::{fmt, sync::Arc};

const MAX_UPDATES: usize = 4096;
const MAX_VALUE_NODES: usize = 65536;
const MAX_TEXT_BYTES: usize = 1024 * 1024;
const MAX_DEPTH: usize = 32;

/// Values use the base model's symbol/reference IDs. Text is interned by Core.
/// Clear removes the inline value but retains the declaration and its identity.
pub enum ValueUpdate<'a> {
    Set { declaration: DeclId, value: ValueR },
    Text { declaration: DeclId, value: &'a str },
    Clear { declaration: DeclId },
}
impl ValueUpdate<'_> {
    fn declaration(&self) -> DeclId {
        match self {
            Self::Set { declaration, .. }
            | Self::Text { declaration, .. }
            | Self::Clear { declaration } => *declaration,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueUpdateError {
    LimitExceeded,
    InvalidLeaf(DeclId),
    DuplicateTarget(DeclId),
    InvalidValue(DeclId),
    InvalidModel,
}
impl fmt::Display for ValueUpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IR value update rejected: {self:?}")
    }
}
impl std::error::Error for ValueUpdateError {}

/// Caller-owned, model-independent scratch for checked value updates.
///
/// The configured target, staging, changed-ID, symbol-occurrence and hash-delta
/// buffers never grow during an update. Their contents are cleared on success
/// or an ordinary error, retaining capacity but no model references or payloads.
/// Returned snapshots, nested-value traversal, compaction and validation can
/// still allocate; this is not an allocation-free or hard-real-time update API.
#[derive(Debug)]
pub struct ValueUpdateWorkspace {
    max_updates: usize,
    targets: Vec<(DeclId, usize)>,
    changed: Vec<DeclId>,
    values: Vec<LeafValueEdit>,
    symbols: SymbolDelta,
    hashes: SemanticDelta,
}

impl ValueUpdateWorkspace {
    /// Reserve scratch for at most `max_updates` edits and
    /// `max_symbol_occurrences` old-plus-new string occurrences in edited values.
    /// Repetitions count separately, including unchanged strings. Zero is valid.
    ///
    /// # Errors
    /// Returns `LimitExceeded` if `max_updates` exceeds 4096, a capacity cannot
    /// be represented, or allocation fails. Existing value/text/depth limits
    /// also apply when this workspace is used.
    ///
    /// ```
    /// use hymeko::module_store::execution::ValueUpdateWorkspace;
    /// let _workspace = ValueUpdateWorkspace::with_capacity(64, 4096)?;
    /// # Ok::<(), hymeko::module_store::execution::ValueUpdateError>(())
    /// ```
    pub fn with_capacity(
        max_updates: usize,
        max_symbol_occurrences: usize,
    ) -> Result<Self, ValueUpdateError> {
        if max_updates > MAX_UPDATES {
            return Err(ValueUpdateError::LimitExceeded);
        }
        let mut workspace = Self::temporary();
        workspace.max_updates = max_updates;
        workspace
            .targets
            .try_reserve_exact(max_updates)
            .map_err(|_| ValueUpdateError::LimitExceeded)?;
        workspace
            .changed
            .try_reserve_exact(max_updates)
            .map_err(|_| ValueUpdateError::LimitExceeded)?;
        workspace
            .values
            .try_reserve_exact(max_updates)
            .map_err(|_| ValueUpdateError::LimitExceeded)?;
        workspace.symbols = SymbolDelta::with_capacity(max_symbol_occurrences)
            .map_err(|_| ValueUpdateError::LimitExceeded)?;
        workspace.hashes = SemanticDelta::with_capacity(max_updates)
            .map_err(|_| ValueUpdateError::LimitExceeded)?;
        Ok(workspace)
    }

    fn temporary() -> Self {
        Self {
            max_updates: MAX_UPDATES,
            targets: Vec::new(),
            changed: Vec::new(),
            values: Vec::new(),
            symbols: SymbolDelta::temporary(),
            hashes: SemanticDelta::default(),
        }
    }

    fn clear(&mut self) {
        self.targets.clear();
        self.changed.clear();
        self.values.clear();
        self.symbols.clear();
        self.hashes.clear();
    }

    fn first_duplicate(&mut self, updates: &[ValueUpdate<'_>]) -> Option<usize> {
        self.targets.extend(
            updates
                .iter()
                .enumerate()
                .map(|(position, update)| (update.declaration(), position)),
        );
        self.targets
            .sort_unstable_by_key(|&(id, position)| (id.raw(), position));
        // Report the first duplicate in INPUT order, not the smallest repeated
        // declaration. Earlier invalid targets/values still take precedence.
        self.targets
            .windows(2)
            .filter(|pair| pair[0].0 == pair[1].0)
            .map(|pair| pair[1].1)
            .min()
    }
}

impl CompiledModel {
    /// Prepare a new immutable model by changing existing leaf values.
    ///
    /// The source model is never mutated, including on failure. Declaration IDs,
    /// hierarchy, import meanings, bindings and facets remain unchanged.
    /// Symbol IDs belong to one snapshot and may be renumbered when unreachable
    /// text is removed; the new model's index and interner are updated together.
    /// IDs supplied in values belong to this base model; foreign-model IDs cannot
    /// be distinguished by their numeric representation. Derived expression values
    /// are not re-evaluated: this edits the already lowered value, not authored AST.
    /// Bitwise-unchanged values retain their payloads and declaration storage.
    /// Every requested target still undergoes validation and symbol accounting.
    /// Structural identity is unchanged. The retained semantic digest rehashes
    /// only changed declarations; topology and concrete-record contributions are
    /// shared with the base. The sorted-hash merge remains linear; the final fold
    /// reuses BLAKE3 subtrees outside the changed sorted byte interval, preserving
    /// the canonical digest bytes. Rank shifts may still invalidate a long range.
    /// The compiled program retains a complete identity, so this public operation
    /// materializes the internal lazy fold before returning. No derived cache
    /// enters the serialized IR.
    ///
    /// Limits: 4096 edits, 65536 aggregate value nodes, depth 32, 1 MiB new text.
    /// This copies the candidate and retains only symbols it actually references.
    /// Repeated updates do not implicitly retain historical text values. Explicit
    /// old snapshot readers retain their independent, unchanged symbol tables.
    /// The first update counts the base model's symbol occurrences once. Later
    /// updates adjust only changed values; unchanged symbol multisets share the
    /// counts. Resolution metadata is shared until symbol IDs need remapping.
    /// Hierarchy/model validation is cached on the immutable base. Value-only
    /// edits inherit that evidence; remapping structural symbol fields revalidates
    /// the candidate. No structural-hash comparison substitutes for validation.
    pub fn with_value_updates(
        &self,
        updates: &[ValueUpdate<'_>],
    ) -> Result<Self, ValueUpdateError> {
        self.with_value_updates_in(updates, &mut ValueUpdateWorkspace::temporary())
    }

    /// Apply the same checked update operation using reusable bounded scratch.
    ///
    /// All contracts of [`Self::with_value_updates`] apply. Workspace exhaustion
    /// returns `LimitExceeded`, without changing the base model or growing the
    /// configured buffers. The workspace is reusable after success or error,
    /// including with a different model; no snapshot borrows it.
    pub fn with_value_updates_in(
        &self,
        updates: &[ValueUpdate<'_>],
        workspace: &mut ValueUpdateWorkspace,
    ) -> Result<Self, ValueUpdateError> {
        workspace.clear();
        let result = self.apply_value_updates(updates, workspace);
        workspace.clear();
        result
    }

    fn apply_value_updates(
        &self,
        updates: &[ValueUpdate<'_>],
        workspace: &mut ValueUpdateWorkspace,
    ) -> Result<Self, ValueUpdateError> {
        if updates.len() > MAX_UPDATES || updates.len() > workspace.max_updates {
            return Err(ValueUpdateError::LimitExceeded);
        }
        let first_duplicate = workspace.first_duplicate(updates);
        let mut nodes = 0;
        let mut text_bytes = 0usize;
        let symbol_count = self.interner.iter().count();
        for (position, update) in updates.iter().enumerate() {
            let target = update.declaration();
            self.program
                .ir
                .decl_node(target)
                .filter(|decl| decl.kind == DeclKind::Node && decl.first_child.is_none())
                .ok_or(ValueUpdateError::InvalidLeaf(target))?;
            if first_duplicate == Some(position) {
                return Err(ValueUpdateError::DuplicateTarget(target));
            }
            match update {
                ValueUpdate::Set { value, .. } => {
                    self.check_update_value(value, target, 0, &mut nodes, symbol_count)?
                }
                ValueUpdate::Text { value, .. } => {
                    nodes += 1;
                    text_bytes = text_bytes
                        .checked_add(value.len())
                        .ok_or(ValueUpdateError::LimitExceeded)?;
                    if text_bytes > MAX_TEXT_BYTES || nodes > MAX_VALUE_NODES {
                        return Err(ValueUpdateError::LimitExceeded);
                    }
                }
                ValueUpdate::Clear { .. } => {}
            }
        }
        // Compaction retains only live strings. Only previously unseen text
        // needs a detached table; other edits borrow the base symbol IDs.
        let mut extended_interner: Option<Interner> = None;
        let mut ir = self.program.ir.clone();
        // Duplicate precedence has already been resolved. Reuse this bounded
        // buffer to retain actual dirty targets after the setter drains values.
        workspace.targets.clear();
        for (position, update) in updates.iter().enumerate() {
            let edit = self.stage_value_update(update, &mut extended_interner);
            if matches!(edit, LeafValueEdit::Replace { .. }) {
                workspace.targets.push((update.declaration(), position));
            }
            workspace.values.push(edit);
        }
        ir.set_leaf_values(&mut workspace.values)
            .map_err(|_| ValueUpdateError::InvalidModel)?;
        let mut index = self.program.idx.clone();
        let mut imports = self.program.imports.clone();
        workspace
            .changed
            .extend(updates.iter().map(ValueUpdate::declaration));
        let (compacted, symbol_usage) = self
            .symbol_usage()
            .map_err(|_| ValueUpdateError::InvalidModel)?
            .with_values(
                (&self.program.ir, &mut ir),
                &mut index,
                &mut imports,
                extended_interner.as_ref().unwrap_or(&self.interner),
                &workspace.changed,
                &mut workspace.symbols,
            )
            .map_err(|error| match error {
                SymbolUpdateError::Invalid => ValueUpdateError::InvalidModel,
                SymbolUpdateError::CapacityExceeded => ValueUpdateError::LimitExceeded,
            })?;
        let interner = compacted.interner;
        let validation = if compacted.structural_symbols_changed {
            StructureValidation::check(&ir, &index, &interner)?
        } else {
            // The checked setter changes only leaf annotation values. Both
            // validators ignore values; all fields they inspect and the old
            // symbol meanings remain unchanged in this branch.
            self.structure_validation()?
        };
        // Symbol bookkeeping deliberately visited ALL targets: unchanged strings
        // still consume the documented occurrence budget. Hash only replacements.
        workspace.changed.clear();
        workspace
            .changed
            .extend(workspace.targets.iter().map(|&(id, _)| id));
        // Values do not participate in the structural hash. No hierarchy, facet,
        // binding or concrete record changes, so their digest contributions remain valid.
        let semantic = self.semantic_state().with_values(
            &ir,
            &interner,
            &workspace.changed,
            compacted.path_symbols_changed,
            &mut workspace.hashes,
        );
        // The kept identity is the updated IR's: its incremental semantic digest and the
        // unchanged structural hash.
        let canon =
            CanonicalIdentity::from_parts(semantic.digest(), self.program.canon.structure());
        Ok(Self {
            program: Arc::new(CompiledProgram {
                root: self.program.root.clone(),
                idx: index,
                ir: ir.into_shared(),
                imports,
                canon,
            }),
            interner,
            semantic: std::sync::OnceLock::from(Arc::new(semantic)),
            symbol_usage: std::sync::OnceLock::from(Ok(symbol_usage)),
            validation: std::sync::OnceLock::from(Ok(validation)),
            // `set_leaf_values` edits leaf annotations and compaction renumbers symbols; no
            // declaration, arc or slot moves, so a derived incidence index stays exact.
            incidence: self
                .incidence
                .get()
                .map_or_else(std::sync::OnceLock::new, |built| {
                    std::sync::OnceLock::from(built.clone())
                }),
        })
    }

    fn stage_value_update(
        &self,
        update: &ValueUpdate<'_>,
        extended_interner: &mut Option<Interner>,
    ) -> LeafValueEdit {
        let declaration = update.declaration();
        // The complete input-validation pass checked every declaration before
        // staging starts; the source model cannot change during this call.
        let current = self.program.ir[self.program.ir.decl_nodes[declaration.raw()].anno]
            .value
            .as_ref();
        let existing_text = match update {
            ValueUpdate::Text { value, .. } => self.interner.get_id(value),
            _ => None,
        };
        let unchanged = match (update, current) {
            (ValueUpdate::Set { value, .. }, Some(old)) => old.bitwise_eq(value),
            (ValueUpdate::Text { .. }, Some(ValueR::Str(symbol))) => existing_text == Some(*symbol),
            (ValueUpdate::Clear { .. }, None) => true,
            _ => false,
        };
        if unchanged {
            return LeafValueEdit::Keep(declaration);
        }
        let value = match update {
            ValueUpdate::Set { value, .. } => Some(value.clone()),
            ValueUpdate::Text { value, .. } => {
                let symbol = existing_text.unwrap_or_else(|| {
                    extended_interner
                        .get_or_insert_with(|| self.interner.fork())
                        .intern(value)
                });
                Some(ValueR::Str(symbol))
            }
            ValueUpdate::Clear { .. } => None,
        };
        LeafValueEdit::Replace { declaration, value }
    }

    fn check_update_value(
        &self,
        value: &ValueR,
        target: DeclId,
        depth: usize,
        nodes: &mut usize,
        symbols: usize,
    ) -> Result<(), ValueUpdateError> {
        *nodes += 1;
        if depth > MAX_DEPTH || *nodes > MAX_VALUE_NODES {
            return Err(ValueUpdateError::LimitExceeded);
        }
        match value {
            ValueR::Num(value) if !value.is_finite() => Err(ValueUpdateError::InvalidValue(target)),
            ValueR::Str(symbol) if symbol.raw() >= symbols => {
                Err(ValueUpdateError::InvalidValue(target))
            }
            ValueR::Ref(declaration) if !self.program.ir.contains_decl(*declaration) => {
                Err(ValueUpdateError::InvalidValue(target))
            }
            ValueR::List(values) => {
                for value in values {
                    self.check_update_value(value, target, depth + 1, nodes, symbols)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

impl ExecutionState {
    /// Stage an already Core-validated model against an explicitly acquired base.
    ///
    /// Acquire base before computing the candidate. A stale or foreign snapshot
    /// is rejected here; commit checks the same base again. No parser is invoked.
    /// Publication continues through the existing commit linearization point.
    pub fn prepare_model_at(
        &self,
        base: &Arc<ExecutionSnapshot>,
        model: CompiledModel,
    ) -> Result<PreparedCompilation, PublicationError> {
        let current = self.snapshot()?;
        if !Arc::ptr_eq(base, &current) {
            return Err(PublicationError::StaleBase {
                expected: base.revision(),
                actual: current.revision(),
            });
        }
        Ok(PreparedCompilation {
            owner: Arc::clone(&self.owner),
            base: Arc::clone(base),
            model,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        common::ids::SymId,
        ir::canonical_hash::Canon,
        module_store::{module_store::HymekoParser, source_provider::MemProvider},
        util::real_parser::RealParser,
    };
    use std::{
        path::Path,
        sync::atomic::{AtomicUsize, Ordering},
    };

    const SOURCE: &str = "description {} world { state 0; label \"initial\"; position {} link -> state; @observation { (+state, -label); } }";
    fn model() -> CompiledModel {
        CompiledModel::compile(
            MemProvider::default().with_file("model.hymeko", SOURCE),
            RealParser,
            Path::new("model.hymeko"),
        )
        .unwrap()
    }
    fn id(model: &CompiledModel, name: &str) -> DeclId {
        model
            .program()
            .ir
            .decls()
            .find(|(_, decl)| model.interner().resolve(decl.name) == name)
            .unwrap()
            .0
    }
    fn value<'a>(model: &'a CompiledModel, name: &str) -> Option<&'a ValueR> {
        model
            .program()
            .ir
            .decl_annotation(id(model, name))
            .unwrap()
            .value
            .as_ref()
    }

    #[test]
    fn unchanged_updates_preserve_flat_and_paged_records_and_publication_order() {
        use crate::ir::semantic_digest::take_hash_work;
        for page_size in [None, Some(2)] {
            let base = model().with_value_updates(&[]).unwrap();
            let base = match page_size {
                Some(size) => base.with_declaration_pages(size).unwrap(),
                None => base,
            };
            base.incidences().unwrap();
            let updates = [
                ValueUpdate::Set {
                    declaration: id(&base, "state"),
                    value: ValueR::Num(0.0),
                },
                ValueUpdate::Text {
                    declaration: id(&base, "label"),
                    value: "initial",
                },
                ValueUpdate::Clear {
                    declaration: id(&base, "position"),
                },
            ];
            let mut workspace = ValueUpdateWorkspace::with_capacity(3, 2).unwrap();
            let buffers = scratch_state(&workspace);
            take_hash_work();
            let next = base
                .with_value_updates_in(&updates, &mut workspace)
                .unwrap();
            for index in 0..base.program.ir.decl_count() {
                assert!(
                    std::ptr::eq(
                        &base.program.ir.decl_nodes[index],
                        &next.program.ir.decl_nodes[index]
                    ),
                    "unchanged record {index} must remain shared, layout {page_size:?}"
                );
            }
            assert_eq!(take_hash_work(), [0; 4]);
            assert_eq!(scratch_state(&workspace), buffers);
            assert_eq!(base.program.canon, next.program.canon);
            assert_eq!(
                crate::writers::cbor_writer::encode_compiled(
                    Path::new(""),
                    base.program(),
                    base.interner()
                )
                .unwrap(),
                crate::writers::cbor_writer::encode_compiled(
                    Path::new(""),
                    next.program(),
                    next.interner()
                )
                .unwrap()
            );
            let state = ExecutionState::new(base);
            let old = state.snapshot().unwrap();
            let prepared = state.prepare_model_at(&old, next).unwrap();
            let published = state.commit(prepared).unwrap();
            assert_eq!(published.revision(), 1);
            assert_eq!(old.revision(), 0);
            assert_eq!(
                old.model().semantic_digest(),
                published.model().semantic_digest()
            );
        }
    }

    #[test]
    fn mixed_updates_keep_nested_payloads_and_hash_only_the_changed_page() {
        use crate::ir::semantic_digest::take_hash_work;
        let initial = model().with_value_updates(&[]).unwrap();
        let target = id(&initial, "state");
        let position = id(&initial, "position");
        let label = id(&initial, "label");
        let nested = ValueR::List(vec![
            ValueR::Num(2.0),
            ValueR::List(vec![
                ValueR::Str(initial.interner.get_id("initial").unwrap()),
                ValueR::Ref(target),
            ]),
        ]);
        let base = initial
            .with_value_updates(&[ValueUpdate::Set {
                declaration: position,
                value: nested.clone(),
            }])
            .unwrap()
            .with_declaration_pages(1)
            .unwrap();
        let updates = [
            ValueUpdate::Set {
                declaration: position,
                value: nested,
            },
            ValueUpdate::Text {
                declaration: label,
                value: "initial",
            },
            ValueUpdate::Set {
                declaration: target,
                value: ValueR::Num(1.0),
            },
        ];
        let mut workspace = ValueUpdateWorkspace::with_capacity(3, 4).unwrap();
        let buffers = scratch_state(&workspace);
        take_hash_work();
        let next = base
            .with_value_updates_in(&updates, &mut workspace)
            .unwrap();
        assert_eq!(take_hash_work(), [1, 0, 0, 0]);
        assert_eq!(scratch_state(&workspace), buffers);
        for index in 0..base.program.ir.decl_count() {
            assert!(
                std::ptr::eq(
                    &base.program.ir.decl_nodes[index],
                    &next.program.ir.decl_nodes[index]
                ),
                "annotation edits must retain the declaration page: record {index}"
            );
        }
        for raw in 0..base.program.ir.annotations.len() {
            let changed = base.program.ir.decl_nodes[target.raw()].anno.raw() == raw;
            assert_eq!(
                std::ptr::eq(
                    &base.program.ir.annotations[raw],
                    &next.program.ir.annotations[raw]
                ),
                !changed,
                "only the edited annotation page may detach: payload {raw}"
            );
        }
        let (Some(ValueR::List(old)), Some(ValueR::List(new))) =
            (value(&base, "position"), value(&next, "position"))
        else {
            panic!("nested list lost")
        };
        assert_eq!(old.as_ptr(), new.as_ptr());
        assert_eq!(value(&base, "state"), Some(&ValueR::Num(0.0)));
        assert_eq!(value(&next, "state"), Some(&ValueR::Num(1.0)));
        assert_eq!(
            next.semantic_digest(),
            Canon::new(&next.program.ir, &next.interner).hash()
        );
    }

    #[test]
    fn unchanged_target_binding_is_checked_before_symbol_capacity() {
        for broken in [false, true] {
            let mut base = admitted_model();
            let target = id(&base, "label");
            if broken {
                let (node, _) = base.program.ir.node_for_decl(target).unwrap();
                Arc::get_mut(&mut base.program).unwrap().ir.nodes[node.raw()].decl = DeclId::NONE;
            }
            let mut workspace = ValueUpdateWorkspace::with_capacity(1, 0).unwrap();
            let buffers = scratch_state(&workspace);
            let result = base.with_value_updates_in(
                &[ValueUpdate::Text {
                    declaration: target,
                    value: "initial",
                }],
                &mut workspace,
            );
            assert_eq!(
                result.err(),
                Some(if broken {
                    ValueUpdateError::InvalidModel
                } else {
                    ValueUpdateError::LimitExceeded
                })
            );
            assert_eq!(scratch_state(&workspace), buffers);
        }
    }

    #[test]
    fn unchanged_values_still_remove_unreachable_text() {
        let mut base = model().with_value_updates(&[]).unwrap();
        base.interner.intern("unreachable-after-admission");
        base.symbol_usage = std::sync::OnceLock::new();
        let next = base
            .with_value_updates(&[ValueUpdate::Set {
                declaration: id(&base, "state"),
                value: ValueR::Num(0.0),
            }])
            .unwrap();
        assert!(
            base.interner
                .get_id("unreachable-after-admission")
                .is_some()
        );
        assert!(
            next.interner
                .get_id("unreachable-after-admission")
                .is_none()
        );
        assert_eq!(base.program.canon, next.program.canon);
        assert_eq!(
            next.semantic_digest(),
            Canon::new(&next.program.ir, &next.interner).hash()
        );
    }

    #[test]
    fn paged_models_preserve_updates_wire_and_derived_state() {
        let base = model().with_value_updates(&[]).unwrap();
        let digest = base.semantic_digest();
        base.incidences().unwrap();
        let previous = Arc::downgrade(&base.program);
        assert_eq!(
            base.with_declaration_pages(3).err(),
            Some(crate::ir::table::IrPageSizeError(3))
        );
        let paged = base.with_declaration_pages(2).unwrap();
        assert!(Arc::ptr_eq(
            base.semantic.get().unwrap(),
            paged.semantic.get().unwrap()
        ));
        assert!(paged.validation.get().is_some());
        assert!(paged.incidence.get().is_some());
        assert_eq!(paged.semantic_digest(), digest);
        let encode = |m: &CompiledModel| {
            crate::writers::cbor_writer::encode_compiled(Path::new(""), m.program(), m.interner())
                .unwrap()
        };
        assert_eq!(encode(&paged), encode(&base));
        let changes = [
            ValueUpdate::Set {
                declaration: id(&base, "state"),
                value: ValueR::Num(37.0),
            },
            ValueUpdate::Text {
                declaration: id(&base, "label"),
                value: "paged update",
            },
        ];
        let flat_next = base.with_value_updates(&changes).unwrap();
        let paged_next = paged.with_value_updates(&changes).unwrap();
        assert_eq!(encode(&flat_next), encode(&paged_next));
        assert_eq!(
            paged_next.semantic_digest(),
            Canon::new(&paged_next.program.ir, &paged_next.interner).hash()
        );
        assert_eq!(paged.semantic_digest(), digest);
        drop(base);
        assert!(previous.upgrade().is_none());
        assert_eq!(value(&paged, "state"), Some(&ValueR::Num(0.0)));
        assert_eq!(value(&paged_next, "state"), Some(&ValueR::Num(37.0)));
    }

    fn admitted_model() -> CompiledModel {
        // Put a value symbol before declaration names so its removal exercises
        // a structural-ID remap through real compiler/codec entry points.
        let mut store = crate::module_store::module_store::ModuleStore::new(
            MemProvider::default().with_file("model.hymeko", SOURCE),
            RealParser,
        );
        store.it.intern("anonymous-placeholder");
        store.it.intern("initial");
        let program = store.compile(Path::new("model.hymeko")).unwrap();
        let bytes =
            crate::writers::cbor_writer::encode_compiled(Path::new(""), &program, &store.it)
                .unwrap();
        CompiledModel::from_decoded_cbor(
            crate::writers::cbor_writer::decode(&bytes, &Default::default()).unwrap(),
        )
    }

    #[test]
    fn structure_evidence_is_cached_for_branches_and_rechecked_on_remap() {
        use super::super::take_structure_checks;
        use crate::ir::coherence::{
            CoherenceLimits, validate_hierarchy_index_coherence, validate_model_contract,
        };
        let base = admitted_model();
        let target = id(&base, "state");
        take_structure_checks();
        let first = base
            .with_value_updates(&[ValueUpdate::Set {
                declaration: target,
                value: ValueR::Num(1.0),
            }])
            .unwrap();
        assert_eq!(take_structure_checks(), 1);
        let branch = base
            .with_value_updates(&[ValueUpdate::Set {
                declaration: target,
                value: ValueR::Num(2.0),
            }])
            .unwrap();
        assert_eq!(take_structure_checks(), 0);
        let text = first
            .with_value_updates(&[ValueUpdate::Text {
                declaration: target,
                value: "tail-proof-text",
            }])
            .unwrap();
        assert_eq!(take_structure_checks(), 0);
        let trimmed = text
            .with_value_updates(&[ValueUpdate::Clear {
                declaration: target,
            }])
            .unwrap();
        assert_eq!(take_structure_checks(), 0);
        let remapped = text
            .with_value_updates(&[ValueUpdate::Clear {
                declaration: id(&text, "label"),
            }])
            .unwrap();
        assert_eq!(take_structure_checks(), 1);
        let following = remapped
            .with_value_updates(&[ValueUpdate::Set {
                declaration: target,
                value: ValueR::Num(3.0),
            }])
            .unwrap();
        assert_eq!(take_structure_checks(), 0);
        for result in [
            &base, &first, &branch, &text, &trimmed, &remapped, &following,
        ] {
            validate_hierarchy_index_coherence(
                &result.program.ir,
                &result.program.idx,
                &result.interner,
                CoherenceLimits::default(),
            )
            .unwrap();
            validate_model_contract(&result.program.ir, &result.interner).unwrap();
            assert_eq!(
                result.semantic_digest(),
                Canon::new(&result.program.ir, &result.interner).hash()
            );
        }
    }

    #[test]
    fn annotation_remaps_keep_topology_paths_and_validation_shared() {
        use super::super::take_structure_checks;
        let base = model().with_value_updates(&[]).unwrap();
        let target = id(&base, "state");
        let mut current = base
            .with_value_updates(&[ValueUpdate::Text {
                declaration: target,
                value: "dynamic-0",
            }])
            .unwrap();
        take_structure_checks();
        for step in 1..24 {
            let text = format!("dynamic-{step}");
            let next = current
                .with_value_updates(&[ValueUpdate::Text {
                    declaration: target,
                    value: &text,
                }])
                .unwrap();
            assert_eq!(take_structure_checks(), 0);
            assert!(Arc::ptr_eq(&current.program.idx, &next.program.idx));
            assert!(Arc::ptr_eq(&current.program.imports, &next.program.imports));
            assert_eq!(
                current.program.ir.nodes.as_ptr(),
                next.program.ir.nodes.as_ptr()
            );
            assert_eq!(
                current.program.ir.edges.as_ptr(),
                next.program.ir.edges.as_ptr()
            );
            assert_eq!(
                current.program.ir.arcs.as_ptr(),
                next.program.ir.arcs.as_ptr()
            );
            assert!(
                current
                    .semantic_state()
                    .shares_layout(next.semantic_state())
            );
            assert_eq!(
                next.semantic_digest(),
                Canon::new(&next.program.ir, &next.interner).hash()
            );
            assert!(
                next.interner
                    .get_id(&format!("dynamic-{}", step - 1))
                    .is_none()
            );
            current = next;
        }
    }

    #[test]
    fn invalid_structure_evidence_is_local_and_preserves_input_error_order() {
        use super::super::take_structure_checks;
        use crate::ir::ir::{DType, Extent, ModelFacet, TensorContract, TensorRole};
        for facet_error in [false, true] {
            let valid = admitted_model();
            let mut invalid = admitted_model();
            let target = id(&invalid, "state");
            let program = Arc::get_mut(&mut invalid.program).unwrap();
            if facet_error {
                program.ir.set_model_facet(
                    target,
                    Some(ModelFacet::Tensor(TensorContract {
                        role: TensorRole::Input,
                        shape: vec![Extent::Static(1)],
                        dtype: DType::F64,
                    })),
                );
            } else {
                program.ir.decl_nodes[target.raw()].parent = DeclId::NONE;
            }
            assert_eq!(
                valid.program.canon.structure(),
                invalid.program.canon.structure()
            );
            take_structure_checks();
            let updates = [ValueUpdate::Set {
                declaration: target,
                value: ValueR::Num(9.0),
            }];
            assert_eq!(
                invalid.with_value_updates(&updates).err(),
                Some(ValueUpdateError::InvalidModel)
            );
            assert_eq!(take_structure_checks(), 1);
            assert_eq!(
                invalid.with_value_updates(&updates).err(),
                Some(ValueUpdateError::InvalidModel)
            );
            assert_eq!(take_structure_checks(), 0);
            assert_eq!(
                invalid
                    .with_value_updates(&[ValueUpdate::Set {
                        declaration: target,
                        value: ValueR::Num(0.0),
                    }])
                    .err(),
                Some(ValueUpdateError::InvalidModel),
                "unchanged values must not bypass structural validation"
            );
            assert_eq!(
                invalid
                    .with_value_updates(&[ValueUpdate::Set {
                        declaration: target,
                        value: ValueR::Num(f64::NAN)
                    }])
                    .err(),
                Some(ValueUpdateError::InvalidValue(target))
            );
            assert!(valid.with_value_updates(&updates).is_ok());
            assert_eq!(
                take_structure_checks(),
                1,
                "a matching hash is not a validation proof"
            );
        }
    }

    #[test]
    fn inherited_structure_evidence_does_not_retain_parent_program() {
        let base = admitted_model();
        let previous = Arc::downgrade(&base.program);
        let next = base
            .with_value_updates(&[ValueUpdate::Set {
                declaration: id(&base, "state"),
                value: ValueR::Num(1.0),
            }])
            .unwrap();
        drop(base);
        assert!(previous.upgrade().is_none());
        assert!(next.validation.get().is_some_and(Result::is_ok));
        assert_eq!(
            next.semantic_digest(),
            Canon::new(&next.program.ir, &next.interner).hash()
        );
    }

    fn buffer_storage<T>(values: &Vec<T>) -> (usize, usize, usize) {
        (values.as_ptr() as usize, values.capacity(), values.len())
    }

    fn scratch_state(workspace: &ValueUpdateWorkspace) -> [(usize, usize, usize); 6] {
        let (symbol_ptr, symbol_capacity, symbol_len) = workspace.symbols.storage();
        let [removed, added] = workspace.hashes.storage();
        [
            buffer_storage(&workspace.targets),
            buffer_storage(&workspace.changed),
            buffer_storage(&workspace.values),
            (symbol_ptr, symbol_capacity, symbol_len),
            (removed.0 as usize, removed.1, removed.2),
            (added.0 as usize, added.1, added.2),
        ]
    }

    #[test]
    fn workspace_capacity_zero_overflow_and_exact_symbol_occurrences() {
        assert_eq!(
            ValueUpdateWorkspace::with_capacity(MAX_UPDATES + 1, 0).err(),
            Some(ValueUpdateError::LimitExceeded)
        );
        assert_eq!(
            ValueUpdateWorkspace::with_capacity(0, usize::MAX).err(),
            Some(ValueUpdateError::LimitExceeded)
        );
        let base = model();
        let mut zero = ValueUpdateWorkspace::with_capacity(0, 0).unwrap();
        assert_eq!(
            base.with_value_updates_in(&[], &mut zero)
                .unwrap()
                .semantic_digest(),
            base.semantic_digest()
        );
        let target = id(&base, "state");
        assert_eq!(
            base.with_value_updates_in(
                &[ValueUpdate::Clear {
                    declaration: target
                }],
                &mut zero
            )
            .err(),
            Some(ValueUpdateError::LimitExceeded)
        );

        let symbol = base.interner.get_id("initial").unwrap();
        let edits = [ValueUpdate::Set {
            declaration: target,
            value: ValueR::List(vec![ValueR::Str(symbol), ValueR::Str(symbol)]),
        }];
        let mut short = ValueUpdateWorkspace::with_capacity(1, 1).unwrap();
        let storage = scratch_state(&short);
        assert_eq!(
            base.with_value_updates_in(&edits, &mut short).err(),
            Some(ValueUpdateError::LimitExceeded)
        );
        assert_eq!(scratch_state(&short), storage);
        assert_eq!(value(&base, "state"), Some(&ValueR::Num(0.0)));
        let mut exact = ValueUpdateWorkspace::with_capacity(1, 2).unwrap();
        let storage = scratch_state(&exact);
        let next = base.with_value_updates_in(&edits, &mut exact).unwrap();
        assert_eq!(scratch_state(&exact), storage);
        // Removing the two old occurrences needs the same capacity, even though
        // the new value contains no symbols. Repetitions are not distinct IDs.
        let clear = [ValueUpdate::Clear {
            declaration: target,
        }];
        assert_eq!(
            next.with_value_updates_in(&clear, &mut short).err(),
            Some(ValueUpdateError::LimitExceeded)
        );
        assert!(next.with_value_updates_in(&clear, &mut exact).is_ok());
        assert_eq!(scratch_state(&exact), storage);
        // An unchanged string still contributes one old and one new occurrence.
        let same = [ValueUpdate::Text {
            declaration: id(&base, "label"),
            value: "initial",
        }];
        assert_eq!(
            base.with_value_updates_in(&same, &mut short).err(),
            Some(ValueUpdateError::LimitExceeded)
        );
        assert!(base.with_value_updates_in(&same, &mut exact).is_ok());
    }

    #[test]
    fn workspace_reuses_all_buffers_across_models_and_keeps_snapshots_independent() {
        let base = model();
        let other = CompiledModel::compile(
            MemProvider::default().with_file(
                "other.hymeko",
                "description {} other { number 3; label \"different\"; }",
            ),
            RealParser,
            Path::new("other.hymeko"),
        )
        .unwrap();
        let mut workspace = ValueUpdateWorkspace::with_capacity(2, 8).unwrap();
        let storage = scratch_state(&workspace);
        let mut readers = Vec::new();
        for (source, name) in [(&base, "state"), (&other, "number"), (&base, "state")] {
            let changes = [
                ValueUpdate::Set {
                    declaration: id(source, name),
                    value: ValueR::Num(42.0),
                },
                ValueUpdate::Text {
                    declaration: id(source, "label"),
                    value: "replacement",
                },
            ];
            let next = source
                .with_value_updates_in(&changes, &mut workspace)
                .unwrap();
            let ordinary = source.with_value_updates(&changes).unwrap();
            assert_eq!(
                next.semantic_digest(),
                Canon::new(&next.program.ir, &next.interner).hash()
            );
            assert_eq!(next.semantic_digest(), ordinary.semantic_digest());
            assert_eq!(
                serde_json::to_vec(&next.program.ir).unwrap(),
                serde_json::to_vec(&ordinary.program.ir).unwrap()
            );
            assert_eq!(
                next.interner.iter().collect::<Vec<_>>(),
                ordinary.interner.iter().collect::<Vec<_>>()
            );
            assert_eq!(scratch_state(&workspace), storage);
            readers.push((next, name));
        }
        drop(workspace);
        for (reader, name) in readers {
            assert_eq!(value(&reader, name), Some(&ValueR::Num(42.0)));
        }
        assert_eq!(value(&base, "state"), Some(&ValueR::Num(0.0)));
        assert_eq!(value(&other, "number"), Some(&ValueR::Num(3.0)));
    }

    #[test]
    fn workspace_cleans_after_early_and_late_errors_then_can_succeed() {
        let base = model();
        let mut workspace = ValueUpdateWorkspace::with_capacity(2, 8).unwrap();
        let storage = scratch_state(&workspace);
        let before = serde_json::to_vec(&base.program.ir).unwrap();
        assert_eq!(
            base.with_value_updates_in(
                &[ValueUpdate::Set {
                    declaration: id(&base, "state"),
                    value: ValueR::Num(f64::NAN),
                }],
                &mut workspace
            )
            .err(),
            Some(ValueUpdateError::InvalidValue(id(&base, "state")))
        );
        assert_eq!(scratch_state(&workspace), storage);

        // This private corruption is impossible through CompiledModel's public
        // immutable facade. It drives the ordinary post-staging error path.
        let mut broken = model();
        let target = id(&broken, "state");
        Arc::get_mut(&mut broken.program).unwrap().ir.decl_nodes[target.raw()].parent =
            DeclId::NONE;
        let corrupted_before = serde_json::to_vec(&broken.program.ir).unwrap();
        assert_eq!(
            broken
                .with_value_updates_in(
                    &[ValueUpdate::Text {
                        declaration: target,
                        value: "must-not-be-retained",
                    }],
                    &mut workspace
                )
                .err(),
            Some(ValueUpdateError::InvalidModel)
        );
        assert_eq!(scratch_state(&workspace), storage);
        assert_eq!(
            serde_json::to_vec(&broken.program.ir).unwrap(),
            corrupted_before
        );
        assert!(broken.interner.get_id("must-not-be-retained").is_none());
        assert!(
            base.with_value_updates_in(
                &[ValueUpdate::Clear {
                    declaration: id(&base, "label")
                }],
                &mut workspace
            )
            .is_ok()
        );
        assert_eq!(scratch_state(&workspace), storage);
        assert_eq!(serde_json::to_vec(&base.program.ir).unwrap(), before);
    }

    #[test]
    fn workspace_preserves_original_input_order_error_precedence() {
        // Frozen validation-order oracle from fceee42: target, HashSet duplicate,
        // then value validation. Only target bookkeeping changed in production.
        fn original(
            base: &CompiledModel,
            updates: &[ValueUpdate<'_>],
        ) -> Result<(), ValueUpdateError> {
            let mut seen = std::collections::HashSet::new();
            let mut nodes = 0;
            for update in updates {
                let target = update.declaration();
                base.program
                    .ir
                    .decl_node(target)
                    .filter(|decl| decl.kind == DeclKind::Node && decl.first_child.is_none())
                    .ok_or(ValueUpdateError::InvalidLeaf(target))?;
                if !seen.insert(target) {
                    return Err(ValueUpdateError::DuplicateTarget(target));
                }
                if let ValueUpdate::Set { value, .. } = update {
                    base.check_update_value(
                        value,
                        target,
                        0,
                        &mut nodes,
                        base.interner.iter().count(),
                    )?;
                }
            }
            Ok(())
        }
        let base = model();
        let mut workspace = ValueUpdateWorkspace::with_capacity(4, 32).unwrap();
        let storage = scratch_state(&workspace);
        for n in 0..=4_u32 {
            for mut encoding in 0..6_usize.pow(n) {
                let updates: Vec<_> = (0..n)
                    .map(|_| {
                        let choice = encoding % 6;
                        encoding /= 6;
                        match choice {
                            0 => ValueUpdate::Clear {
                                declaration: id(&base, "state"),
                            },
                            1 => ValueUpdate::Clear {
                                declaration: id(&base, "label"),
                            },
                            2 => ValueUpdate::Clear {
                                declaration: DeclId::NONE,
                            },
                            3 => ValueUpdate::Clear {
                                declaration: id(&base, "world"),
                            },
                            4 => ValueUpdate::Set {
                                declaration: id(&base, "state"),
                                value: ValueR::Num(f64::NAN),
                            },
                            _ => ValueUpdate::Set {
                                declaration: id(&base, "position"),
                                value: ValueR::Str(SymId::NONE),
                            },
                        }
                    })
                    .collect();
                assert_eq!(
                    base.with_value_updates_in(&updates, &mut workspace).err(),
                    original(&base, &updates).err()
                );
                assert_eq!(scratch_state(&workspace), storage);
            }
        }
    }

    #[test]
    fn value_edits_share_metadata_until_live_symbol_ids_move() {
        let base = model().with_value_updates(&[]).unwrap();
        let state = id(&base, "state");
        let numeric = base
            .with_value_updates(&[ValueUpdate::Set {
                declaration: state,
                value: ValueR::Num(2.0),
            }])
            .unwrap();
        assert!(Arc::ptr_eq(&base.program.idx, &numeric.program.idx));
        assert!(Arc::ptr_eq(&base.program.imports, &numeric.program.imports));
        assert_eq!(
            base.symbol_usage().unwrap(),
            numeric.symbol_usage().unwrap()
        );
        let with_text = numeric
            .with_value_updates(&[ValueUpdate::Text {
                declaration: state,
                value: "new-tail-symbol",
            }])
            .unwrap();
        assert!(Arc::ptr_eq(&numeric.program.idx, &with_text.program.idx));
        let cleared = with_text
            .with_value_updates(&[ValueUpdate::Clear { declaration: state }])
            .unwrap();
        assert!(Arc::ptr_eq(&with_text.program.idx, &cleared.program.idx));
        assert!(cleared.interner().get_id("new-tail-symbol").is_none());
        assert!(with_text.interner().get_id("new-tail-symbol").is_some());
        // The removed value precedes the newly added tail string, but not any
        // names. IDs move only in annotations; resolution metadata stays shared.
        let remapped = with_text
            .with_value_updates(&[ValueUpdate::Clear {
                declaration: id(&with_text, "label"),
            }])
            .unwrap();
        assert!(Arc::ptr_eq(&with_text.program.idx, &remapped.program.idx));
        assert_ne!(
            with_text.interner.get_id("new-tail-symbol"),
            remapped.interner.get_id("new-tail-symbol")
        );
        assert!(Arc::ptr_eq(
            &with_text.program.imports,
            &remapped.program.imports
        ));
        assert_eq!(
            remapped.symbol_usage().unwrap(),
            &crate::resolution::symbols::SymbolUsage::new(
                &remapped.program.ir,
                &remapped.program.idx,
                &remapped.program.imports,
                &remapped.interner,
            )
            .unwrap()
        );
        assert_eq!(
            remapped.semantic_digest(),
            Canon::new(&remapped.program.ir, &remapped.interner).hash()
        );
        assert_eq!(value(&base, "state"), Some(&ValueR::Num(0.0)));
    }

    #[test]
    fn value_edits_share_unchanged_tables_and_isolate_later_mutation() {
        let base = model().with_value_updates(&[]).unwrap();
        let digest = base.semantic_digest();
        let next = base
            .with_value_updates(&[ValueUpdate::Set {
                declaration: id(&base, "state"),
                value: ValueR::Num(7.0),
            }])
            .unwrap();
        let old = &base.program().ir;
        let new = &next.program().ir;
        assert!(!old.nodes.is_empty() && !old.edges.is_empty() && !old.arcs.is_empty());
        assert_eq!(old.decl_nodes.as_ptr(), new.decl_nodes.as_ptr());
        assert_ne!(old.annotations.as_ptr(), new.annotations.as_ptr());
        assert_eq!(old.nodes.as_ptr(), new.nodes.as_ptr());
        assert_eq!(old.edges.as_ptr(), new.edges.as_ptr());
        assert_eq!(old.arcs.as_ptr(), new.arcs.as_ptr());
        assert_eq!(old.decl_to_record.as_ptr(), new.decl_to_record.as_ptr());
        assert_eq!(old.decl_file.as_ptr(), new.decl_file.as_ptr());
        assert_eq!(value(&base, "state"), Some(&ValueR::Num(0.0)));
        assert_eq!(value(&next, "state"), Some(&ValueR::Num(7.0)));
        assert_eq!(
            Canon::new(new, next.interner()).hash(),
            next.semantic_digest()
        );
        let count = old.arcs[0].refs.len();
        assert!(count > 0);
        let mut edited = new.clone();
        assert_eq!(edited.decl_nodes.as_ptr(), new.decl_nodes.as_ptr());
        edited.arcs[0].refs.clear();
        assert_eq!(old.arcs[0].refs.len(), count);
        assert_eq!(new.arcs[0].refs.len(), count);
        assert_eq!(Canon::new(old, base.interner()).hash(), digest);
    }

    #[test]
    fn text_edits_reuse_base_and_new_symbols_within_one_batch() {
        let base = model();
        let next = base
            .with_value_updates(&[
                ValueUpdate::Text {
                    declaration: id(&base, "label"),
                    value: "world",
                },
                ValueUpdate::Text {
                    declaration: id(&base, "state"),
                    value: "new shared text",
                },
                ValueUpdate::Text {
                    declaration: id(&base, "position"),
                    value: "new shared text",
                },
            ])
            .unwrap();
        for (name, text) in [
            ("label", "world"),
            ("state", "new shared text"),
            ("position", "new shared text"),
        ] {
            let Some(ValueR::Str(symbol)) = value(&next, name) else {
                panic!("expected text")
            };
            assert_eq!(next.interner().resolve(*symbol), text);
        }
        assert_eq!(
            next.interner()
                .iter()
                .filter(|(_, s)| *s == "new shared text")
                .count(),
            1
        );
        assert!(next.interner().get_id("initial").is_none());
        assert!(base.interner().get_id("initial").is_some());
        assert!(base.interner().get_id("new shared text").is_none());
        assert_eq!(
            next.semantic_digest(),
            Canon::new(&next.program().ir, next.interner()).hash()
        );
        assert_eq!(
            next.program().canon.structure(),
            base.program().canon.structure()
        );
    }

    #[test]
    fn symbol_lifetime_successive_text_updates_keep_only_current_values() {
        let original = model();
        let target = id(&original, "label");
        let initial_count = original.interner().iter().count();
        let mut current = original.with_value_updates(&[]).unwrap();
        for step in 0..1000 {
            let text = format!("timestamp-{step:019}");
            current = current
                .with_value_updates(&[ValueUpdate::Text {
                    declaration: target,
                    value: &text,
                }])
                .unwrap();
            assert!(
                current.interner().iter().count() <= initial_count + 1,
                "unreachable old values must not accumulate"
            );
            let Some(ValueR::Str(symbol)) = value(&current, "label") else {
                panic!("label");
            };
            assert_eq!(current.interner().resolve(*symbol), text);
            assert_eq!(
                current.semantic_digest(),
                Canon::new(&current.program().ir, current.interner()).hash()
            );
        }
        let Some(ValueR::Str(old)) = value(&original, "label") else {
            panic!("old label");
        };
        assert_eq!(
            original.interner().resolve(*old),
            "initial",
            "explicit old snapshot stays valid"
        );
    }

    #[test]
    fn symbol_lifetime_nested_base_symbols_survive_until_last_reference_is_cleared() {
        let base = model();
        let label = id(&base, "label");
        let state = id(&base, "state");
        let initial = base.interner().get_id("initial").unwrap();
        let next = base
            .with_value_updates(&[
                ValueUpdate::Text {
                    declaration: label,
                    value: "replacement",
                },
                ValueUpdate::Set {
                    declaration: state,
                    value: ValueR::List(vec![
                        ValueR::List(vec![ValueR::Str(initial)]),
                        ValueR::Ref(label),
                    ]),
                },
            ])
            .unwrap();
        assert!(next.interner().get_id("initial").is_some());
        let cleared = next
            .with_value_updates(&[ValueUpdate::Clear { declaration: state }])
            .unwrap();
        assert!(cleared.interner().get_id("initial").is_none());
        assert!(
            next.interner().get_id("initial").is_some(),
            "retained previous graph stays valid"
        );
        assert_eq!(
            cleared.semantic_digest(),
            Canon::new(&cleared.program().ir, cleared.interner()).hash()
        );
    }

    #[test]
    fn numeric_text_list_and_clear_preserve_identity_and_old_reader() {
        let original = model();
        let state = id(&original, "state");
        let label = id(&original, "label");
        let position = id(&original, "position");
        let digest = Canon::new(&original.program().ir, original.interner()).hash();
        let updated = original
            .with_value_updates(&[
                ValueUpdate::Set {
                    declaration: state,
                    value: ValueR::Num(2.0),
                },
                ValueUpdate::Text {
                    declaration: label,
                    value: "new observation",
                },
                ValueUpdate::Set {
                    declaration: position,
                    value: ValueR::List(vec![ValueR::Num(-0.000000001), ValueR::Num(1.0)]),
                },
            ])
            .unwrap();
        assert!(matches!(value(&original,"state"),Some(ValueR::Num(n)) if *n == 0.0));
        assert!(matches!(value(&updated,"state"),Some(ValueR::Num(n)) if *n == 2.0));
        let Some(ValueR::Str(symbol)) = value(&updated, "label") else {
            panic!("label");
        };
        assert_eq!(updated.interner().resolve(*symbol), "new observation");
        assert_eq!(id(&updated, "position"), position);
        assert_eq!(
            original.program().ir.decl_count(),
            updated.program().ir.decl_count()
        );
        assert_eq!(
            original.program().ir.node_count(),
            updated.program().ir.node_count()
        );
        let recomputed = Canon::new(&updated.program().ir, updated.interner()).hash();
        assert_ne!(digest, recomputed);
        // The kept identity is the updated IR's, not the original's.
        assert_eq!(updated.program().canon.hash(), recomputed);
        assert_eq!(
            digest,
            Canon::new(&original.program().ir, original.interner()).hash()
        );
        let cleared = updated
            .with_value_updates(&[ValueUpdate::Clear {
                declaration: position,
            }])
            .unwrap();
        assert!(value(&cleared, "position").is_none());
        assert!(value(&updated, "position").is_some());
    }

    #[test]
    fn incremental_digest_rehashes_only_edits_and_matches_full_recomputation() {
        use crate::ir::{
            canonical_hash::{CanonHashCfg, CanonicalPaths},
            semantic_digest::take_hash_work,
        };
        let base = model();
        let target = id(&base, "state");
        let label = id(&base, "label");
        let reference = id(&base, "position");
        take_hash_work();
        let baseline = base.semantic_digest();
        assert_eq!(
            take_hash_work(),
            [
                base.program.ir.decl_count(),
                base.program.ir.node_count(),
                base.program.ir.edge_count(),
                base.program.ir.arc_count()
            ]
        );
        assert_eq!(base.semantic_digest(), baseline);
        assert_eq!(take_hash_work(), [0; 4]);
        let mut current = base.with_value_updates(&[]).unwrap();
        assert_eq!(current.semantic_digest(), baseline);
        assert_eq!(take_hash_work(), [0; 4]);
        // Include signed zero, reference identity, nested lists, strings, clearing,
        // repeated identical edits and branching from an unchanged old reader.
        for step in 0..32 {
            let numeric = match step % 4 {
                0 => ValueR::Num(-0.0),
                1 => ValueR::Ref(reference),
                2 => ValueR::List(vec![
                    ValueR::Num(step as f64),
                    ValueR::List(vec![ValueR::Ref(label)]),
                ]),
                _ => ValueR::Num(0.0),
            };
            let text = format!("observation-{step}");
            let updates = [
                ValueUpdate::Set {
                    declaration: target,
                    value: numeric,
                },
                if step % 3 == 0 {
                    ValueUpdate::Clear { declaration: label }
                } else {
                    ValueUpdate::Text {
                        declaration: label,
                        value: &text,
                    }
                },
            ];
            let next = current.with_value_updates(&updates).unwrap();
            assert_eq!(take_hash_work(), [2, 0, 0, 0]);
            let digest = next.semantic_digest();
            assert_eq!(next.semantic_digest(), digest);
            assert_eq!(take_hash_work(), [0; 4]);
            assert_eq!(digest, Canon::new(&next.program.ir, next.interner()).hash());
            assert_eq!(
                next.program.canon.structure().0,
                Canon::new(&next.program.ir, next.interner())
                    .structure(CanonHashCfg::default())
                    .0
            );
            assert_eq!(base.semantic_digest(), baseline);
            take_hash_work();
            current = next;
        }
        let branch = base
            .with_value_updates(&[ValueUpdate::Set {
                declaration: target,
                value: ValueR::Num(-0.0),
            }])
            .unwrap();
        assert_eq!(take_hash_work(), [1, 0, 0, 0]);
        assert_ne!(branch.semantic_digest(), baseline);
        assert_eq!(
            branch.semantic_digest(),
            Canon::new(&branch.program.ir, branch.interner()).hash()
        );
        take_hash_work();
        let unchanged = branch
            .with_value_updates(&[ValueUpdate::Set {
                declaration: target,
                value: ValueR::Num(-0.0),
            }])
            .unwrap();
        assert_eq!(take_hash_work(), [0; 4]);
        assert_eq!(unchanged.semantic_digest(), branch.semantic_digest());
    }

    #[test]
    fn bad_updates_leave_base_unchanged() {
        let base = model();
        let target = id(&base, "state");
        for updates in [
            vec![ValueUpdate::Set {
                declaration: DeclId::NONE,
                value: ValueR::Num(1.),
            }],
            vec![ValueUpdate::Clear {
                declaration: id(&base, "world"),
            }],
            vec![ValueUpdate::Clear {
                declaration: id(&base, "observation"),
            }],
            vec![
                ValueUpdate::Clear {
                    declaration: target,
                },
                ValueUpdate::Clear {
                    declaration: target,
                },
            ],
            vec![ValueUpdate::Set {
                declaration: target,
                value: ValueR::Num(f64::NAN),
            }],
            vec![ValueUpdate::Set {
                declaration: target,
                value: ValueR::Str(SymId::new(usize::MAX)),
            }],
            vec![ValueUpdate::Set {
                declaration: target,
                value: ValueR::Ref(DeclId::NONE),
            }],
        ] {
            assert!(base.with_value_updates(&updates).is_err());
            assert!(matches!(value(&base,"state"),Some(ValueR::Num(n)) if *n == 0.));
        }
        let mut nested = ValueR::Num(0.);
        for _ in 0..MAX_DEPTH + 2 {
            nested = ValueR::List(vec![nested]);
        }
        assert_eq!(
            base.with_value_updates(&[ValueUpdate::Set {
                declaration: target,
                value: nested
            }])
            .err(),
            Some(ValueUpdateError::LimitExceeded)
        );
        let text = "x".repeat(MAX_TEXT_BYTES + 1);
        assert_eq!(
            base.with_value_updates(&[ValueUpdate::Text {
                declaration: target,
                value: &text
            }])
            .err(),
            Some(ValueUpdateError::LimitExceeded)
        );
        assert_eq!(
            base.with_value_updates(
                &(0..MAX_UPDATES + 1)
                    .map(|_| ValueUpdate::Clear {
                        declaration: target
                    })
                    .collect::<Vec<_>>()
            )
            .err(),
            Some(ValueUpdateError::LimitExceeded)
        );
        assert_eq!(
            base.with_value_updates(&[
                ValueUpdate::Set {
                    declaration: target,
                    value: ValueR::List(vec![ValueR::Num(0.); MAX_VALUE_NODES - 1])
                },
                ValueUpdate::Text {
                    declaration: id(&base, "label"),
                    value: "one-more-node"
                },
            ])
            .err(),
            Some(ValueUpdateError::LimitExceeded)
        );
    }

    #[test]
    fn template_updates_do_not_parse_or_accumulate_dynamic_symbols() {
        struct Counted<'a>(&'a AtomicUsize);
        impl HymekoParser for Counted<'_> {
            fn parse<'a>(&self, source: &'a str) -> Result<parser::ast::AstStr<'a>, String> {
                self.0.fetch_add(1, Ordering::SeqCst);
                RealParser.parse(source)
            }
        }
        let calls = AtomicUsize::new(0);
        let template = CompiledModel::compile(
            MemProvider::default().with_file("model.hymeko", SOURCE),
            Counted(&calls),
            Path::new("model.hymeko"),
        )
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let symbols = template
            .with_value_updates(&[])
            .unwrap()
            .interner()
            .iter()
            .count();
        for n in 0..16 {
            let next = template
                .with_value_updates(&[ValueUpdate::Text {
                    declaration: id(&template, "label"),
                    value: &format!("measurement-{n}"),
                }])
                .unwrap();
            assert_eq!(next.interner().iter().count(), symbols);
            assert!(next.interner().get_id("initial").is_none());
            assert_eq!(id(&next, "state"), id(&template, "state"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn prepared_ir_reuses_publication_and_rejects_stale_bases() {
        let state = ExecutionState::new(model());
        let base = state.snapshot().unwrap();
        let target = id(base.model(), "state");
        let candidate = base
            .model()
            .with_value_updates(&[ValueUpdate::Set {
                declaration: target,
                value: ValueR::Num(2.),
            }])
            .unwrap();
        let prepared = state.prepare_model_at(&base, candidate).unwrap();
        let competing = state
            .prepare_model_at(&base, base.model().with_value_updates(&[]).unwrap())
            .unwrap();
        let next = state.commit(prepared).unwrap();
        assert_eq!(next.revision(), 1);
        assert!(matches!(value(base.model(),"state"),Some(ValueR::Num(n)) if *n == 0.));
        assert!(matches!(value(next.model(),"state"),Some(ValueR::Num(n)) if *n == 2.));
        assert!(matches!(
            state.commit(competing).err(),
            Some(PublicationError::StaleBase { .. })
        ));
        assert!(matches!(
            state.prepare_model_at(&base, model()).err(),
            Some(PublicationError::StaleBase { .. })
        ));
        let foreign = ExecutionState::new(model()).snapshot().unwrap();
        assert!(state.prepare_model_at(&foreign, model()).is_err());
    }
}
