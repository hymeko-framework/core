//! Model-local symbol lifetime. One typed walk serves marking and dense rewriting.
//! Callers own the candidate IR/index; immutable source snapshots are never changed.
use crate::{
    common::ids::SymId,
    ir::ir::{AnnoR, Extent, Ir, ModelFacet, SignedRefR, ValueR},
    module_store::module_store::ModuleKey,
    resolution::{interner::Interner, resolve::Index},
};
use rustc_hash::FxHashMap;

#[derive(Debug, Clone, Copy)]
pub(crate) struct SymbolError {
    pub field: &'static str,
    pub symbol: usize,
}

trait SymbolVisitor {
    fn symbol(&mut self, id: &mut SymId, field: &'static str) -> Result<(), SymbolError>;
}

struct LiveSymbols<'a>(&'a mut [bool]);
impl SymbolVisitor for LiveSymbols<'_> {
    fn symbol(&mut self, id: &mut SymId, field: &'static str) -> Result<(), SymbolError> {
        let used = self.0.get_mut(id.raw()).ok_or(SymbolError {
            field,
            symbol: id.raw(),
        })?;
        *used = true;
        Ok(())
    }
}

struct DenseSymbols<'a>(&'a [SymId]);
impl SymbolVisitor for DenseSymbols<'_> {
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
        let mut program = ProgramSymbols { ir, index, imports };
        program.visit(&mut LiveSymbols(&mut used))?;
        let mut mapping = vec![SymId::NONE; used.len()];
        let mut interner = Interner::new();
        let mut renumbered = false;
        // Retain original relative order, independent of HashMap traversal order.
        for (old, text) in source.iter() {
            if used[old.raw()] {
                let new = interner.intern(text);
                mapping[old.raw()] = new;
                renumbered |= old != new;
            }
        }
        if renumbered {
            program.visit(&mut DenseSymbols(&mapping))?;
        }
        Ok(Self {
            interner,
            renumbered,
        })
    }
}

struct ProgramSymbols<'a> {
    ir: &'a mut Ir,
    index: &'a mut Index,
    imports: &'a mut [(SymId, ModuleKey)],
}
impl ProgramSymbols<'_> {
    fn visit(&mut self, visitor: &mut impl SymbolVisitor) -> Result<(), SymbolError> {
        for declaration in &mut self.ir.decl_nodes {
            visitor.symbol(&mut declaration.name, "declaration name")?;
            Self::anno(&mut declaration.anno, visitor)?;
        }
        for (_, prefix) in &mut self.ir.namespace_roots {
            Self::path(prefix, "namespace prefix symbol", visitor)?;
        }
        for refs in self
            .ir
            .nodes
            .iter_mut()
            .map(|n| &mut n.bases)
            .chain(self.ir.edges.iter_mut().map(|e| &mut e.bases))
        {
            Self::refs(refs, visitor)?;
        }
        for arc in &mut self.ir.arcs {
            Self::anno(&mut arc.anno, visitor)?;
            Self::refs(&mut arc.refs, visitor)?;
        }
        for facet in self.ir.decl_model.iter_mut().flatten() {
            match facet {
                ModelFacet::Tensor(tensor) => {
                    for dimension in &mut tensor.shape {
                        match dimension {
                            Extent::Symbolic(id) => visitor.symbol(id, "tensor symbolic extent")?,
                            Extent::Static(_) => {}
                        }
                    }
                }
                ModelFacet::Operator(operator) => {
                    visitor.symbol(&mut operator.identity, "operator identity")?
                }
            }
        }
        let paths = std::mem::take(&mut self.index.by_path);
        self.index.by_path.reserve(paths.len());
        for (mut path, declaration) in paths {
            Self::path(&mut path.0, "path-index symbol", visitor)?;
            self.index.by_path.insert(path, declaration);
        }
        Self::aliases(&mut self.index.aliases, visitor)?;
        for (_, aliases) in &mut self.index.module_aliases {
            Self::aliases(aliases, visitor)?;
        }
        for (namespace, _) in &mut *self.imports {
            visitor.symbol(namespace, "import namespace")?;
        }
        Ok(())
    }

    fn path(
        path: &mut [SymId],
        field: &'static str,
        visitor: &mut impl SymbolVisitor,
    ) -> Result<(), SymbolError> {
        for id in path {
            visitor.symbol(id, field)?;
        }
        Ok(())
    }

    fn aliases(
        aliases: &mut FxHashMap<SymId, Vec<SymId>>,
        visitor: &mut impl SymbolVisitor,
    ) -> Result<(), SymbolError> {
        let old = std::mem::take(aliases);
        aliases.reserve(old.len());
        for (mut alias, mut path) in old {
            visitor.symbol(&mut alias, "alias")?;
            Self::path(&mut path, "alias path symbol", visitor)?;
            aliases.insert(alias, path);
        }
        Ok(())
    }

    fn anno(anno: &mut AnnoR, visitor: &mut impl SymbolVisitor) -> Result<(), SymbolError> {
        Self::path(&mut anno.tags, "annotation tag", visitor)?;
        if let Some(value) = &mut anno.value {
            Self::value(value, visitor)?;
        }
        Ok(())
    }

    fn refs(refs: &mut [SignedRefR], visitor: &mut impl SymbolVisitor) -> Result<(), SymbolError> {
        for reference in refs {
            let (SignedRefR::Plus(atom) | SignedRefR::Minus(atom) | SignedRefR::Neutral(atom)) =
                reference;
            Self::anno(&mut atom.anno, visitor)?;
            if let Some(weights) = &mut atom.weights {
                for value in weights {
                    Self::value(value, visitor)?;
                }
            }
        }
        Ok(())
    }

    fn value(root: &mut ValueR, visitor: &mut impl SymbolVisitor) -> Result<(), SymbolError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ir::{
            canonical_hash::{CanonHashCfg, CanonicalPaths, DerivedPathIndex},
            ir::{DType, OperatorContract, PortDirection, RefAtomR, TensorContract, TensorRole},
            semantic_digest::semantic_model_digest,
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
            index.by_path = DerivedPathIndex::new(&ir)
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
    fn symbol_lifetime_covers_all_typed_symbol_positions_and_is_idempotent() {
        let mut f = Fixture::new();
        let semantic = semantic_model_digest(&f.ir, &f.strings);
        let structural = CanonicalPaths::new(&f.ir).hash(CanonHashCfg::default(), &f.strings);
        let declaration_names =
            f.ir.decl_nodes
                .iter()
                .map(|d| f.strings.resolve(d.name).to_owned())
                .collect::<Vec<_>>();
        let compact =
            SymbolCompaction::apply(&mut f.ir, &mut f.index, &mut f.imports, &f.strings).unwrap();
        assert!(compact.renumbered);
        assert!(
            compact
                .interner
                .iter()
                .all(|(_, s)| !s.starts_with("unused-before-"))
        );
        assert_eq!(semantic_model_digest(&f.ir, &compact.interner), semantic);
        assert_eq!(
            CanonicalPaths::new(&f.ir).hash(CanonHashCfg::default(), &compact.interner),
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
            semantic_model_digest(decoded.ir(), decoded.interner()),
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
            semantic_model_digest(&next.program().ir, next.interner()),
            "record hashes remain valid while cached symbolic paths are remapped"
        );
        assert_eq!(base.semantic_digest(), semantic);
    }
}
