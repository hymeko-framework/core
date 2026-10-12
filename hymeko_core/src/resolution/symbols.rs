//! Model-local symbol lifetime. One typed field walk serves marking and rewriting.
//! Marking borrows shared IR tables; only dense rewriting may detach them.
use crate::{
    common::ids::{DeclId, SymId},
    ir::ir::{AnnoR, Extent, Ir, ModelFacet, SignedRefR, ValueR},
    module_store::module_store::ModuleKey,
    resolution::{interner::Interner, resolve::Index},
};
use rustc_hash::FxHashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub(crate) struct SymbolError {
    pub field: &'static str,
    pub symbol: usize,
}

trait SymbolReader {
    fn symbol(&mut self, id: &SymId, field: &'static str) -> Result<(), SymbolError>;
}

struct LiveSymbols<'a>(&'a mut [bool]);
impl SymbolReader for LiveSymbols<'_> {
    fn symbol(&mut self, id: &SymId, field: &'static str) -> Result<(), SymbolError> {
        let used = self.0.get_mut(id.raw()).ok_or(SymbolError {
            field,
            symbol: id.raw(),
        })?;
        *used = true;
        Ok(())
    }
}

/// Exact occurrences, including immutable topology, metadata and resolution paths.
/// Derived state belongs to one immutable program/interner pair, never the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SymbolUsage {
    counts: Arc<Vec<usize>>,
    unused: usize,
}

struct CountSymbols<'a>(&'a mut [usize]);
impl SymbolReader for CountSymbols<'_> {
    fn symbol(&mut self, id: &SymId, field: &'static str) -> Result<(), SymbolError> {
        let error = SymbolError {
            field,
            symbol: id.raw(),
        };
        let count = self.0.get_mut(id.raw()).ok_or(error)?;
        *count = count.checked_add(1).ok_or(error)?;
        Ok(())
    }
}

struct SymbolDelta {
    occurrences: Vec<(usize, bool)>,
    adding: bool,
    symbols: usize,
}
impl SymbolReader for SymbolDelta {
    fn symbol(&mut self, id: &SymId, field: &'static str) -> Result<(), SymbolError> {
        if id.raw() >= self.symbols {
            return Err(SymbolError {
                field,
                symbol: id.raw(),
            });
        }
        self.occurrences.push((id.raw(), self.adding));
        Ok(())
    }
}

impl SymbolUsage {
    pub(crate) fn new(
        ir: &Ir,
        index: &Index,
        imports: &[(SymId, ModuleKey)],
        source: &Interner,
    ) -> Result<Self, SymbolError> {
        let mut counts = vec![0; source.iter().count()];
        ProgramSymbols { ir, index, imports }.mark(&mut CountSymbols(&mut counts))?;
        Ok(Self {
            unused: counts.iter().filter(|&&count| count == 0).count(),
            counts: Arc::new(counts),
        })
    }

    /// Change counts only for the supplied leaf values. Topology, annotations
    /// other than values, index and imports must still match the base snapshot.
    pub(crate) fn with_values(
        &self,
        before: &Ir,
        after: &mut Ir,
        index: &mut Arc<Index>,
        imports: &mut Arc<Vec<(SymId, ModuleKey)>>,
        source: &Interner,
        changed: &[DeclId],
    ) -> Result<(SymbolCompaction, Self), SymbolError> {
        let symbols = source.iter().count();
        if symbols < self.counts.len() {
            return Err(SymbolError {
                field: "symbol usage table",
                symbol: self.counts.len(),
            });
        }
        let mut delta = SymbolDelta {
            occurrences: Vec::new(),
            adding: false,
            symbols,
        };
        for &declaration in changed {
            for (ir, adding) in [(before, false), (&*after, true)] {
                delta.adding = adding;
                if let Some(value) = &ir.decl_nodes[declaration.raw()].anno.value {
                    ProgramSymbols::read_value(value, &mut delta)?;
                }
            }
        }
        // Combine before mutating so swaps and equal symbol multisets neither
        // detach the counts nor transiently remove a still-referenced string.
        delta
            .occurrences
            .sort_unstable_by_key(|&(symbol, _)| symbol);
        let mut next = self.clone();
        if symbols > next.counts.len() {
            next.unused += symbols - next.counts.len();
            Arc::make_mut(&mut next.counts).resize(symbols, 0);
        }
        let mut cursor = 0;
        while cursor < delta.occurrences.len() {
            let symbol = delta.occurrences[cursor].0;
            let mut added = 0;
            let mut removed = 0;
            while cursor < delta.occurrences.len() && delta.occurrences[cursor].0 == symbol {
                if delta.occurrences[cursor].1 {
                    added += 1;
                } else {
                    removed += 1;
                }
                cursor += 1;
            }
            let old = next.counts[symbol];
            let new = old
                .checked_sub(removed)
                .and_then(|n| n.checked_add(added))
                .ok_or(SymbolError {
                    field: "symbol usage count",
                    symbol,
                })?;
            if old != new {
                Arc::make_mut(&mut next.counts)[symbol] = new;
                if old == 0 {
                    next.unused -= 1;
                }
                if new == 0 {
                    next.unused += 1;
                }
            }
        }
        if next.unused == 0 {
            return Ok((
                SymbolCompaction {
                    interner: source.fork(),
                    renumbered: false,
                },
                next,
            ));
        }
        let used: Vec<_> = next.counts.iter().map(|&count| count != 0).collect();
        let compacted = SymbolCompaction::retain(source, &used, |mapping| {
            ProgramSymbols::rewrite(
                after,
                Arc::make_mut(index),
                Arc::make_mut(imports).as_mut_slice(),
                &mut DenseSymbols(mapping),
            )
        })?;
        next.counts = Arc::new(
            next.counts
                .iter()
                .copied()
                .filter(|&count| count != 0)
                .collect(),
        );
        next.unused = 0;
        Ok((compacted, next))
    }
}

struct DenseSymbols<'a>(&'a [SymId]);
impl DenseSymbols<'_> {
    fn symbol(&mut self, id: &mut SymId, field: &'static str) -> Result<(), SymbolError> {
        *id = self
            .0
            .get(id.raw())
            .copied()
            .filter(|s| s.is_some())
            .ok_or(SymbolError {
                field,
                symbol: id.raw(),
            })?;
        Ok(())
    }
}

pub(crate) struct SymbolCompaction {
    pub interner: Interner,
    pub renumbered: bool,
}
impl SymbolCompaction {
    pub(crate) fn apply(
        ir: &mut Ir,
        index: &mut Index,
        imports: &mut [(SymId, ModuleKey)],
        source: &Interner,
    ) -> Result<Self, SymbolError> {
        let mut used = vec![false; source.iter().count()];
        let program = ProgramSymbols { ir, index, imports };
        program.mark(&mut LiveSymbols(&mut used))?;
        Self::retain(source, &used, |mapping| {
            ProgramSymbols::rewrite(ir, index, imports, &mut DenseSymbols(mapping))
        })
    }

    fn retain(
        source: &Interner,
        used: &[bool],
        rewrite: impl FnOnce(&[SymId]) -> Result<(), SymbolError>,
    ) -> Result<Self, SymbolError> {
        let (interner, mapping) = source.retained(used);
        let renumbered = mapping.is_some();
        if let Some(mapping) = mapping {
            rewrite(&mapping)?;
        }
        Ok(Self {
            interner,
            renumbered,
        })
    }
}

struct ProgramSymbols<'a> {
    ir: &'a Ir,
    index: &'a Index,
    imports: &'a [(SymId, ModuleKey)],
}

// One field schema, instantiated with shared or mutable borrows. Merely asking
// for mutable iteration would detach a shared table, even when no ID changes.
// The visitor signatures enforce that the marking expansion cannot write.
macro_rules! symbol_walk {
    ($walk:ident, $path:ident, $anno:ident, $refs:ident, $value:ident,
     $visitor:ty, $iter:ident $(, $mutable:tt)?) => {
        impl ProgramSymbols<'_> {
            fn $walk(ir: & $($mutable)? Ir, visitor: &mut $visitor) -> Result<(), SymbolError> {
                for declaration in & $($mutable)? ir.decl_nodes {
                    visitor.symbol(& $($mutable)? declaration.name, "declaration name")?;
                    Self::$anno(& $($mutable)? declaration.anno, visitor)?;
                }
                for (_, prefix) in & $($mutable)? ir.namespace_roots {
                    Self::$path(prefix, "namespace prefix symbol", visitor)?;
                }
                for refs in ir.nodes.$iter().map(|n| & $($mutable)? n.bases)
                    .chain(ir.edges.$iter().map(|e| & $($mutable)? e.bases)) {
                    Self::$refs(refs, visitor)?;
                }
                for arc in & $($mutable)? ir.arcs {
                    Self::$anno(& $($mutable)? arc.anno, visitor)?;
                    Self::$refs(& $($mutable)? arc.refs, visitor)?;
                }
                for facet in ir.decl_model.$iter().flatten() {
                    match facet {
                        ModelFacet::Tensor(tensor) => {
                            for dimension in & $($mutable)? tensor.shape {
                                match dimension {
                                    Extent::Symbolic(id) => visitor.symbol(id, "tensor symbolic extent")?,
                                    Extent::Static(_) => {}
                                }
                            }
                        }
                        ModelFacet::Operator(operator) =>
                            visitor.symbol(& $($mutable)? operator.identity, "operator identity")?,
                    }
                }
                Ok(())
            }

            fn $path(path: & $($mutable)? [SymId], field: &'static str,
                     visitor: &mut $visitor) -> Result<(), SymbolError> {
                for id in path { visitor.symbol(id, field)?; }
                Ok(())
            }

            fn $anno(anno: & $($mutable)? AnnoR, visitor: &mut $visitor) -> Result<(), SymbolError> {
                Self::$path(& $($mutable)? anno.tags, "annotation tag", visitor)?;
                if let Some(value) = & $($mutable)? anno.value { Self::$value(value, visitor)?; }
                Ok(())
            }

            fn $refs(refs: & $($mutable)? [SignedRefR], visitor: &mut $visitor) -> Result<(), SymbolError> {
                for reference in refs {
                    let (SignedRefR::Plus(atom) | SignedRefR::Minus(atom) | SignedRefR::Neutral(atom)) = reference;
                    Self::$anno(& $($mutable)? atom.anno, visitor)?;
                    if let Some(weights) = & $($mutable)? atom.weights {
                        for value in weights { Self::$value(value, visitor)?; }
                    }
                }
                Ok(())
            }

            fn $value(root: & $($mutable)? ValueR, visitor: &mut $visitor) -> Result<(), SymbolError> {
                if matches!(root, ValueR::Num(_) | ValueR::Ref(_)) { return Ok(()); }
                let mut pending = vec![root];
                while let Some(value) = pending.pop() {
                    match value {
                        ValueR::Str(id) => visitor.symbol(id, "string value")?,
                        ValueR::List(values) => pending.extend(values),
                        ValueR::Num(_) | ValueR::Ref(_) => {}
                    }
                }
                Ok(())
            }
        }
    };
}

symbol_walk!(
    read_ir,
    read_path,
    read_anno,
    read_refs,
    read_value,
    impl SymbolReader,
    iter
);
symbol_walk!(
    write_ir,
    write_path,
    write_anno,
    write_refs,
    write_value,
    DenseSymbols<'_>,
    iter_mut,
    mut
);

impl ProgramSymbols<'_> {
    fn mark(&self, visitor: &mut impl SymbolReader) -> Result<(), SymbolError> {
        Self::read_ir(self.ir, visitor)?;
        for path in self.index.by_path.keys() {
            Self::read_path(&path.0, "path-index symbol", visitor)?;
        }
        for aliases in std::iter::once(&self.index.aliases)
            .chain(self.index.module_aliases.iter().map(|(_, aliases)| aliases))
        {
            for (alias, path) in aliases {
                visitor.symbol(alias, "alias")?;
                Self::read_path(path, "alias path symbol", visitor)?;
            }
        }
        for (namespace, _) in self.imports.iter() {
            visitor.symbol(namespace, "import namespace")?;
        }
        Ok(())
    }

    fn rewrite(
        ir: &mut Ir,
        index: &mut Index,
        imports: &mut [(SymId, ModuleKey)],
        visitor: &mut DenseSymbols<'_>,
    ) -> Result<(), SymbolError> {
        Self::write_ir(ir, visitor)?;
        // A dense rewrite changes keys, so it must rebuild their maps.
        let paths = std::mem::take(&mut index.by_path);
        index.by_path.reserve(paths.len());
        for (mut path, declaration) in paths {
            Self::write_path(&mut path.0, "path-index symbol", visitor)?;
            index.by_path.insert(path, declaration);
        }
        Self::rewrite_aliases(&mut index.aliases, visitor)?;
        for (_, aliases) in &mut index.module_aliases {
            Self::rewrite_aliases(aliases, visitor)?;
        }
        for (namespace, _) in imports {
            visitor.symbol(namespace, "import namespace")?;
        }
        Ok(())
    }

    fn rewrite_aliases(
        aliases: &mut FxHashMap<SymId, Vec<SymId>>,
        visitor: &mut DenseSymbols<'_>,
    ) -> Result<(), SymbolError> {
        let old = std::mem::take(aliases);
        aliases.reserve(old.len());
        for (mut alias, mut path) in old {
            visitor.symbol(&mut alias, "alias")?;
            Self::write_path(&mut path, "alias path symbol", visitor)?;
            aliases.insert(alias, path);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ir::{
            canonical_hash::{Canon, CanonHashCfg, build_path_index},
            ir::{DType, OperatorContract, PortDirection, RefAtomR, TensorContract, TensorRole},
        },
        ir_stream::DecodeLimits,
        module_store::{execution::CompiledModel, source_provider::MemProvider},
        util::real_parser::RealParser,
        writers::cbor_writer,
    };
    use std::path::Path;

    struct Fixture {
        ir: Ir,
        index: Index,
        strings: Interner,
        imports: Vec<(SymId, ModuleKey)>,
    }
    impl Fixture {
        fn new() -> Self {
            let model = CompiledModel::compile(
                MemProvider::default().with_file(
                    "model.hymeko",
                    "description {} root { label \"active\"; target {} @edge { (+target); } }",
                ),
                RealParser,
                Path::new("model.hymeko"),
            )
            .unwrap();
            let mut ir = model.program().ir.clone();
            let mut strings = Interner::new();
            for (_, text) in model.interner().iter() {
                strings.intern(text);
            }
            let mut symbol = |text: &str| {
                strings.intern(&format!("unused-before-{text}"));
                strings.intern(text)
            };
            let [
                namespace,
                name,
                tag,
                scalar,
                nested,
                dimension,
                operator,
                alias,
                local,
            ] = [
                "namespace",
                "renamed-root",
                "tag",
                "scalar",
                "nested",
                "dimension",
                "operator",
                "alias",
                "local-alias",
            ]
            .map(&mut symbol);
            let root = ir.nodes[0].decl;
            let target = ir.nodes.last().unwrap().decl;
            ir.decl_nodes[root.raw()].name = name;
            ir.record_namespace_root(root, &[namespace]);
            for declaration in &mut ir.decl_nodes {
                declaration.anno.tags.push(tag);
            }
            let atom = || RefAtomR {
                target,
                direction: PortDirection::Out,
                anno: AnnoR {
                    tags: vec![tag],
                    value: Some(ValueR::Str(scalar)),
                },
                weights: Some(vec![ValueR::List(vec![
                    ValueR::Str(nested),
                    ValueR::Ref(root),
                ])]),
            };
            ir.nodes[0].bases.push(SignedRefR::Plus(atom()));
            ir.edges[0].bases.push(SignedRefR::Minus(atom()));
            ir.arcs[0].anno = atom().anno;
            let mut input = atom();
            input.target = root;
            input.direction = PortDirection::In;
            ir.arcs[0].refs = vec![
                SignedRefR::Plus(input),
                SignedRefR::Minus(atom()),
                SignedRefR::Neutral(atom()),
            ];
            ir.set_model_facet(
                root,
                Some(ModelFacet::Tensor(TensorContract {
                    role: TensorRole::Input,
                    shape: vec![Extent::Symbolic(dimension)],
                    dtype: DType::F64,
                })),
            );
            ir.set_model_facet(
                ir.edges[0].decl,
                Some(ModelFacet::Operator(OperatorContract {
                    identity: operator,
                })),
            );
            ir.set_model_facet(
                target,
                Some(ModelFacet::Tensor(TensorContract {
                    role: TensorRole::Output,
                    shape: vec![Extent::Static(1)],
                    dtype: DType::F64,
                })),
            );
            crate::ir::coherence::validate_model_contract(&ir, &strings).unwrap();
            let mut index = Index::new();
            index.by_path = build_path_index(&ir)
                .unwrap()
                .iter()
                .map(|(p, d)| (p.clone(), d))
                .collect();
            index.aliases.insert(alias, vec![namespace, name]);
            index.module_aliases.push((
                0..ir.decl_count(),
                [(local, vec![namespace, name])].into_iter().collect(),
            ));
            Self {
                ir,
                index,
                strings,
                imports: vec![(namespace, ModuleKey("modules/library.hymeko".into()))],
            }
        }
    }

    #[test]
    fn incremental_usage_matches_full_compaction_with_aliases_and_metadata() {
        let mut f = Fixture::new();
        let a = f.ir.nodes[1].decl;
        let b = f.ir.nodes[2].decl;
        let mut index = Arc::new(f.index);
        let mut imports = Arc::new(f.imports);
        let mut usage = SymbolUsage::new(&f.ir, &index, &imports, &f.strings).unwrap();
        for step in 0..24 {
            let mut source = f.strings.fork();
            let fresh = source.intern(&format!("new-value-{step}"));
            // A dead suffix must be removed even if no retained ID needs moving.
            source.intern("unused-suffix");
            let tag = source.get_id("tag").unwrap();
            let mut edited = f.ir.clone();
            edited.decl_nodes[a.raw()].anno.value = Some(ValueR::List(vec![
                ValueR::Str(tag),
                ValueR::Str(fresh),
                ValueR::List(vec![ValueR::Str(fresh), ValueR::Num(step as f64)]),
            ]));
            edited.decl_nodes[b.raw()].anno.value = if step % 2 == 0 {
                Some(ValueR::Str(fresh))
            } else {
                None
            };
            let mut reference_ir = edited.clone();
            let mut reference_index = index.as_ref().clone();
            let mut reference_imports = imports.as_ref().clone();
            let reference = SymbolCompaction::apply(
                &mut reference_ir,
                &mut reference_index,
                &mut reference_imports,
                &source,
            )
            .unwrap();
            let old_index = Arc::clone(&index);
            let old_imports = Arc::clone(&imports);
            let old_usage = usage.clone();
            let (actual, next) = usage
                .with_values(
                    &f.ir,
                    &mut edited,
                    &mut index,
                    &mut imports,
                    &source,
                    &[a, b],
                )
                .unwrap();
            assert_eq!(
                serde_json::to_vec(&edited).unwrap(),
                serde_json::to_vec(&reference_ir).unwrap()
            );
            assert_eq!(index.by_path, reference_index.by_path);
            assert_eq!(index.aliases, reference_index.aliases);
            assert_eq!(index.module_aliases, reference_index.module_aliases);
            assert_eq!(imports.as_ref(), &reference_imports);
            assert_eq!(actual.renumbered, reference.renumbered);
            assert_eq!(
                actual.interner.iter().collect::<Vec<_>>(),
                reference.interner.iter().collect::<Vec<_>>()
            );
            assert_eq!(
                next,
                SymbolUsage::new(&edited, &index, &imports, &actual.interner).unwrap()
            );
            assert_eq!(usage, old_usage);
            assert_eq!(
                usage,
                SymbolUsage::new(&f.ir, &old_index, &old_imports, &f.strings).unwrap()
            );
            assert_eq!(Arc::ptr_eq(&old_index, &index), !actual.renumbered);
            assert_eq!(Arc::ptr_eq(&old_imports, &imports), !actual.renumbered);
            assert!(actual.interner.get_id("unused-suffix").is_none());
            f.ir = edited;
            f.strings = actual.interner;
            usage = next;
        }
    }

    #[test]
    fn numeric_and_equal_symbol_multisets_keep_shared_counts() {
        let mut f = Fixture::new();
        let compact =
            SymbolCompaction::apply(&mut f.ir, &mut f.index, &mut f.imports, &f.strings).unwrap();
        f.strings = compact.interner;
        let target = f.ir.nodes[2].decl;
        let tag = f.strings.get_id("tag").unwrap();
        for (before, after) in [
            (Some(ValueR::Num(0.0)), Some(ValueR::Num(1.0))),
            (
                Some(ValueR::List(vec![
                    ValueR::Str(tag),
                    ValueR::Num(0.0),
                    ValueR::Str(tag),
                ])),
                Some(ValueR::List(vec![
                    ValueR::Str(tag),
                    ValueR::List(vec![ValueR::Str(tag)]),
                    ValueR::Num(2.0),
                ])),
            ),
        ] {
            f.ir.decl_nodes[target.raw()].anno.value = before;
            let mut edited = f.ir.clone();
            edited.decl_nodes[target.raw()].anno.value = after;
            let mut index = Arc::new(f.index.clone());
            let mut imports = Arc::new(f.imports.clone());
            let old_index = Arc::clone(&index);
            let old_imports = Arc::clone(&imports);
            let usage = SymbolUsage::new(&f.ir, &index, &imports, &f.strings).unwrap();
            let (compacted, next) = usage
                .with_values(
                    &f.ir,
                    &mut edited,
                    &mut index,
                    &mut imports,
                    &f.strings,
                    &[target],
                )
                .unwrap();
            assert!(Arc::ptr_eq(&usage.counts, &next.counts));
            assert!(Arc::ptr_eq(&old_index, &index));
            assert!(Arc::ptr_eq(&old_imports, &imports));
            assert!(!compacted.renumbered);
            assert_eq!(
                next,
                SymbolUsage::new(&edited, &index, &imports, &compacted.interner).unwrap()
            );
        }
    }

    #[test]
    fn usage_rejects_bad_symbols_and_count_underflow_without_changing_base() {
        let f = Fixture::new();
        let target = f.ir.nodes[1].decl;
        let usage = SymbolUsage::new(&f.ir, &f.index, &f.imports, &f.strings).unwrap();
        let base = usage.clone();
        let mut index = Arc::new(f.index);
        let mut imports = Arc::new(f.imports);
        let mut edited = f.ir.clone();
        edited.decl_nodes[target.raw()].anno.value = Some(ValueR::Str(SymId::NONE));
        assert!(SymbolUsage::new(&edited, &index, &imports, &f.strings).is_err());
        assert!(
            usage
                .with_values(
                    &f.ir,
                    &mut edited,
                    &mut index,
                    &mut imports,
                    &f.strings,
                    &[target]
                )
                .is_err()
        );
        edited.decl_nodes[target.raw()].anno.value = None;
        // Internal misuse: removing the same single-use value twice must fail
        // closed instead of wrapping a counter and retaining a dead symbol.
        assert!(
            usage
                .with_values(
                    &f.ir,
                    &mut edited,
                    &mut index,
                    &mut imports,
                    &f.strings,
                    &[target, target]
                )
                .is_err()
        );
        assert_eq!(usage, base);
        assert!(Arc::ptr_eq(&usage.counts, &base.counts));
    }

    #[test]
    fn marking_keeps_path_and_alias_maps_in_place() {
        let mut f = Fixture::new();
        f.ir = f.ir.into_shared();
        let retained = f.ir.clone();
        let symbol = SymId::NONE;
        let (alias, path) = f.index.aliases.iter().next().unwrap();
        let aliases_before = (*alias, path.clone(), alias as *const SymId as usize);
        let (path, declaration) = f.index.by_path.iter().next().unwrap();
        let path_before = (path.clone(), *declaration, path as *const _ as usize);
        let mut used = vec![false; f.strings.iter().count()];
        ProgramSymbols {
            ir: &mut f.ir,
            index: &mut f.index,
            imports: &mut f.imports,
        }
        .mark(&mut LiveSymbols(&mut used))
        .unwrap();
        assert_eq!(f.ir.decl_nodes.as_ptr(), retained.decl_nodes.as_ptr());
        assert_eq!(f.ir.nodes.as_ptr(), retained.nodes.as_ptr());
        assert_eq!(f.ir.edges.as_ptr(), retained.edges.as_ptr());
        assert_eq!(f.ir.arcs.as_ptr(), retained.arcs.as_ptr());
        let (path, declaration) = f.index.by_path.get_key_value(&path_before.0).unwrap();
        assert_eq!(*declaration, path_before.1);
        assert_eq!(
            path as *const _ as usize, path_before.2,
            "marking must not rebuild the path map"
        );
        let (alias, path) = f.index.aliases.get_key_value(&aliases_before.0).unwrap();
        assert_eq!(path, &aliases_before.1);
        assert_eq!(
            alias as *const SymId as usize, aliases_before.2,
            "marking must not rebuild the alias map"
        );
        assert!(
            LiveSymbols(&mut used)
                .symbol(&symbol, "invalid probe")
                .is_err()
        );
    }

    #[test]
    fn symbol_lifetime_covers_all_typed_symbol_positions_and_is_idempotent() {
        let mut f = Fixture::new();
        f.ir = f.ir.into_shared();
        let retained = f.ir.clone();
        let semantic = Canon::new(&f.ir, &f.strings).hash();
        let structural = Canon::new(&f.ir, &f.strings)
            .structure(CanonHashCfg::default())
            .0;
        let declaration_names =
            f.ir.decl_nodes
                .iter()
                .map(|d| f.strings.resolve(d.name).to_owned())
                .collect::<Vec<_>>();
        let compact =
            SymbolCompaction::apply(&mut f.ir, &mut f.index, &mut f.imports, &f.strings).unwrap();
        assert!(compact.renumbered);
        assert_eq!(Canon::new(&retained, &f.strings).hash(), semantic);
        assert!(
            compact
                .interner
                .iter()
                .all(|(_, s)| !s.starts_with("unused-before-"))
        );
        assert_eq!(Canon::new(&f.ir, &compact.interner).hash(), semantic);
        assert_eq!(
            Canon::new(&f.ir, &compact.interner)
                .structure(CanonHashCfg::default())
                .0,
            structural
        );
        assert_eq!(
            f.ir.decl_nodes
                .iter()
                .map(|d| compact.interner.resolve(d.name).to_owned())
                .collect::<Vec<_>>(),
            declaration_names
        );
        let namespace = compact.interner.get_id("namespace").unwrap();
        let name = compact.interner.get_id("renamed-root").unwrap();
        assert_eq!(
            f.index.aliases[&compact.interner.get_id("alias").unwrap()],
            vec![namespace, name]
        );
        assert_eq!(
            f.index.module_aliases[0].1[&compact.interner.get_id("local-alias").unwrap()],
            vec![namespace, name]
        );
        assert_eq!(f.imports[0].0, namespace);
        let again =
            SymbolCompaction::apply(&mut f.ir, &mut f.index, &mut f.imports, &compact.interner)
                .unwrap();
        assert!(!again.renumbered);
        assert_eq!(
            again.interner.iter().collect::<Vec<_>>(),
            compact.interner.iter().collect::<Vec<_>>()
        );
        let bytes =
            cbor_writer::encode("model.hymeko", &f.ir, &f.index, &again.interner, &f.imports)
                .unwrap();
        let decoded = cbor_writer::decode(&bytes, &DecodeLimits::strict()).unwrap();
        assert_eq!(
            Canon::new(decoded.ir(), decoded.interner()).hash(),
            semantic
        );
        assert_eq!(decoded.canonical_hash(), structural);
        let base = CompiledModel::from_decoded_cbor(decoded);
        assert_eq!(base.semantic_digest(), semantic);
        let label = base
            .program()
            .ir
            .decls()
            .find(|(_, d)| base.interner().resolve(d.name) == "label")
            .unwrap()
            .0;
        let next = base
            .with_value_updates(&[crate::module_store::execution::ValueUpdate::Text {
                declaration: label,
                value: "new live value",
            }])
            .unwrap();
        assert_eq!(
            next.semantic_digest(),
            Canon::new(&next.program().ir, next.interner()).hash(),
            "record hashes remain valid while cached symbolic paths are remapped"
        );
        assert_eq!(base.semantic_digest(), semantic);
    }
}
