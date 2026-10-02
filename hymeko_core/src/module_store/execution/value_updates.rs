//! Checked leaf-value updates to a retained canonical compiled model.
//! No parser, AST lowering, declaration allocation or link rewriting occurs here.
use super::{
    CompiledModel, CompiledProgram, ExecutionSnapshot, ExecutionState, PreparedCompilation,
    PublicationError,
};
use crate::{
    common::ids::DeclId,
    ir::{
        canonical_hash::CanonicalIdentity,
        coherence::{CoherenceLimits, validate_hierarchy_index_coherence, validate_model_contract},
        ir::{DeclKind, ValueR},
    },
    resolution::{interner::Interner, symbols::SymbolCompaction},
};
use std::{collections::HashSet, fmt, sync::Arc};

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
    /// Structural identity is unchanged. The retained semantic digest rehashes
    /// only edited declarations; topology and concrete-record contributions are
    /// shared with the base. Its final sorted-hash fold remains linear to preserve
    /// the canonical digest bytes. No derived cache enters the serialized IR.
    ///
    /// Limits: 4096 edits, 65536 aggregate value nodes, depth 32, 1 MiB new text.
    /// This copies the candidate and retains only symbols it actually references.
    /// Repeated updates do not implicitly retain historical text values. Explicit
    /// old snapshot readers retain their independent, unchanged symbol tables.
    pub fn with_value_updates(
        &self,
        updates: &[ValueUpdate<'_>],
    ) -> Result<Self, ValueUpdateError> {
        if updates.len() > MAX_UPDATES {
            return Err(ValueUpdateError::LimitExceeded);
        }
        let mut seen = HashSet::new();
        let mut nodes = 0;
        let mut text_bytes = 0usize;
        let symbol_count = self.interner.iter().count();
        for update in updates {
            let target = update.declaration();
            self.program
                .ir
                .decl_node(target)
                .filter(|decl| decl.kind == DeclKind::Node && decl.first_child.is_none())
                .ok_or(ValueUpdateError::InvalidLeaf(target))?;
            if !seen.insert(target) {
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
        let mut interner = Interner::new();
        for (id, text) in self.interner.iter() {
            let copied = interner.intern(text);
            debug_assert_eq!(id, copied);
        }
        let mut ir = self.program.ir.clone();
        let values = updates
            .iter()
            .map(|update| {
                let value = match update {
                    ValueUpdate::Set { value, .. } => Some(value.clone()),
                    ValueUpdate::Text { value, .. } => Some(ValueR::Str(interner.intern(value))),
                    ValueUpdate::Clear { .. } => None,
                };
                (update.declaration(), value)
            })
            .collect();
        ir.set_leaf_values(values)
            .map_err(|_| ValueUpdateError::InvalidModel)?;
        let mut index = self.program.idx.clone();
        let mut imports = self.program.imports.clone();
        let compacted = SymbolCompaction::apply(&mut ir, &mut index, &mut imports, &interner)
            .map_err(|_| ValueUpdateError::InvalidModel)?;
        interner = compacted.interner;
        validate_hierarchy_index_coherence(&ir, &index, &interner, CoherenceLimits::default())
            .map_err(|_| ValueUpdateError::InvalidModel)?;
        validate_model_contract(&ir, &interner).map_err(|_| ValueUpdateError::InvalidModel)?;
        // Values do not participate in the structural hash. No hierarchy, facet,
        // binding or concrete record changes, so their digest contributions remain valid.
        let changed: Vec<_> = updates.iter().map(ValueUpdate::declaration).collect();
        let semantic =
            self.semantic_state()
                .with_values(&ir, &interner, &changed, compacted.renumbered);
        // The kept identity is the updated IR's: its incremental semantic digest and the
        // unchanged structural hash.
        let canon =
            CanonicalIdentity::from_parts(semantic.digest(), self.program.canon.structure());
        Ok(Self {
            program: Arc::new(CompiledProgram {
                root: self.program.root.clone(),
                idx: index,
                ir,
                imports,
                canon,
            }),
            interner,
            semantic: std::sync::OnceLock::from(semantic),
        })
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
            .decl_node(id(model, name))
            .unwrap()
            .anno
            .value
            .as_ref()
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
            assert_eq!(
                digest,
                Canon::new(&next.program.ir, next.interner()).hash()
            );
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
        assert_eq!(take_hash_work(), [1, 0, 0, 0]);
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
