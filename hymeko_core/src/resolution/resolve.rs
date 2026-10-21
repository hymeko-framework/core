// src/resolve.rs
use crate::common::ids::{DeclId, SymId};
use crate::common::pathkey::PathKey;
use crate::ir::ir::{
    AnnoR, DeclKind, DeclNode, IncidenceSign, PortDirection, RefAtomR, ReferenceDraft, ValueR,
};
use crate::resolution::interner::Interner;
use crate::sym_ast::AstSym;
use parser::ast::{
    Anno, EdgeDecl, HyperArc, HyperItem, NodeDecl, Ref, SignedRef, UsingStmt, Value,
};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Index {
    pub by_path: FxHashMap<PathKey, DeclId>,
    /// Alias map from `using path as alias` statements.
    #[serde(default)]
    pub aliases: FxHashMap<SymId, Vec<SymId>>,
    /// Module-local environments, keyed by their contiguous named declaration ranges.
    /// An empty environment still isolates a dependency from the entry's aliases.
    #[serde(skip)]
    pub(crate) module_aliases: Vec<(std::ops::Range<usize>, FxHashMap<SymId, Vec<SymId>>)>,
}

impl Index {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PathKey, &DeclId)> {
        self.by_path.iter()
    }
}

#[derive(Debug)]
pub enum ResolveError {
    DuplicateDecl {
        path: String,
    },
    UnresolvedRef {
        from_scope: String,
        target: String,
    },
    AmbiguousRef {
        from_scope: String,
        target: String,
        candidates: Vec<String>,
    },
    MissingDecl {
        detail: String,
    },
    UnevaluatedConst {
        from_scope: String,
    },
    InvalidIr(crate::ir::ir::dispatch::DispatchError),
    UnexpectedTopLevelArc {
        detail: String,
    },
}

pub fn build_index_sym<'a>(d: &AstSym<'a>, it: &Interner) -> Result<Index, ResolveError> {
    let mut idx = Index::default();
    let mut next: usize = 0;
    build_index_sym_with_prefix(d, &[], it, &mut idx, &mut next)?;
    Ok(idx)
}

pub(crate) fn module_aliases(
    idx: &Index,
    usings: &[UsingStmt<SymId>],
    prefix: &[SymId],
    imports: &[(SymId, SymId)],
    it: &Interner,
) -> Result<FxHashMap<SymId, Vec<SymId>>, ResolveError> {
    let mut aliases: FxHashMap<_, _> = imports
        .iter()
        .map(|&(local, linked)| (local, vec![linked]))
        .collect();
    for u in usings {
        let mut candidates = Vec::new();
        // A qualified local import name names its linked module even when another
        // module used the same name for a different file.
        if let Some((&first, tail)) = u.path.path.split_first() {
            if let Some(base) = aliases.get(&first) {
                let mut path = base.clone();
                path.extend_from_slice(tail);
                candidates.push(path);
            }
        }
        for &(_, linked) in imports {
            let mut path = vec![linked];
            path.extend_from_slice(&u.path.path);
            candidates.push(path);
        }
        let mut local = prefix.to_vec();
        local.extend_from_slice(&u.path.path);
        candidates.push(local);
        let resolved = candidates
            .into_iter()
            .find(|path| idx.by_path.contains_key(path.as_slice()))
            .ok_or_else(|| ResolveError::UnresolvedRef {
                from_scope: "<using>".into(),
                target: fmt_path_from_slice(&u.path.path, it),
            })?;
        aliases.insert(u.alias, resolved);
    }
    Ok(aliases)
}

pub fn build_index_sym_with_prefix<'a>(
    d: &AstSym<'a>,
    prefix: &[SymId],
    it: &Interner,
    idx: &mut Index,
    next: &mut usize,
) -> Result<(), ResolveError> {
    let mut path = prefix.to_vec();
    // Index header nodes with the given prefix
    for n in &d.header {
        index_node(idx, next, &mut path, n, it)?;
    }

    // items
    index_items(idx, next, &mut path, &d.items, it)?;
    Ok(())
}

fn index_items<'a>(
    idx: &mut Index,
    next: &mut usize,
    path: &mut Vec<SymId>,
    items: &[HyperItem<'a, SymId>],
    it: &Interner,
) -> Result<(), ResolveError> {
    for item in items {
        match item {
            HyperItem::Node(n) => index_node(idx, next, path, n, it)?,
            HyperItem::Edge(e) => index_edge(idx, next, path, e, it)?,
            HyperItem::Arc(_) => {}
        }
    }
    Ok(())
}

fn resolve_value<'a>(
    idx: &Index,
    scope: &[SymId],
    v: &Value<'a, SymId>,
    it: &mut Interner,
) -> Result<ValueR, ResolveError> {
    Ok(match v {
        Value::Str(s) => ValueR::Str(it.intern(s)), // Bridging to owned IR
        Value::Num(x) => ValueR::Num(*x),
        Value::List(xs) => ValueR::List(
            xs.iter()
                .map(|x| resolve_value(idx, scope, x, it))
                .collect::<Result<_, _>>()?,
        ),
        Value::Ref(r) => {
            let did = resolve_ref_to_declid(idx, scope, r, it)?;
            ValueR::Ref(did)
        }
        // Tier B: const expressions must be evaluated by the
        // pre-resolve const pass; if one survives to this point it is
        // a pipeline-ordering bug.
        Value::Expr(_) => {
            return Err(ResolveError::UnevaluatedConst {
                from_scope: fmt_scope(scope, it),
            });
        }
    })
}

pub fn resolve_anno<'a>(
    idx: &Index,
    scope: &[SymId],
    a: &Anno<'a, SymId>,
    it: &mut Interner,
) -> Result<AnnoR, ResolveError> {
    Ok(AnnoR {
        tags: a.tags.clone(), // They are already SymIds! No interning needed.
        value: match &a.value {
            Some(v) => Some(resolve_value(idx, scope, v, it)?),
            None => None,
        },
    })
}

fn add_decl(
    idx: &mut Index,
    next: &mut usize,
    key: PathKey,
    it: &Interner,
) -> Result<DeclId, ResolveError> {
    if idx.by_path.contains_key(&key) {
        return Err(ResolveError::DuplicateDecl {
            path: fmt_path(&key, it),
        });
    }
    let id = DeclId::new(*next);
    *next += 1;
    idx.by_path.insert(key, id);
    Ok(id)
}

fn index_node<'a>(
    idx: &mut Index,
    next: &mut usize,
    path: &mut Vec<SymId>,
    n: &NodeDecl<'a, SymId>,
    it: &Interner,
) -> Result<(), ResolveError> {
    let scope_len = path.len();
    path.push(n.inner.name);
    let _id = add_decl(idx, next, PathKey(path.clone()), it)?;

    if let Some(body) = &n.inner.body {
        index_items(idx, next, path, body, it)?;
    }
    path.truncate(scope_len);
    Ok(())
}

fn index_edge<'a>(
    idx: &mut Index,
    next: &mut usize,
    path: &mut Vec<SymId>,
    e: &EdgeDecl<'a, SymId>,
    it: &Interner,
) -> Result<(), ResolveError> {
    let scope_len = path.len();
    path.push(e.inner.name);
    let _id = add_decl(idx, next, PathKey(path.clone()), it)?;

    index_items(idx, next, path, &e.inner.body, it)?;
    path.truncate(scope_len);
    Ok(())
}

fn fmt_path_from_slice(p: &[SymId], it: &Interner) -> String {
    p.iter()
        .map(|&s| it.resolve(s))
        .collect::<Vec<_>>()
        .join(".")
}

pub fn resolve_ref_to_declid(
    idx: &Index,
    scope: &[SymId],
    target: &Ref<SymId>,
    it: &Interner,
) -> Result<DeclId, ResolveError> {
    let owner = (0..=scope.len())
        .rev()
        .find_map(|end| idx.by_path.get(&scope[..end]));
    let aliases = owner
        .and_then(|id| {
            let module = idx
                .module_aliases
                .partition_point(|(range, _)| range.end <= id.raw());
            idx.module_aliases
                .get(module)
                .filter(|(range, _)| range.contains(&id.raw()))
                .map(|(_, aliases)| aliases)
        })
        .unwrap_or(&idx.aliases);
    // ── Alias expansion ──────────────────────────────────────
    let expanded;
    let effective = if !target.path.is_empty() {
        if let Some(exp) = aliases.get(&target.path[0]) {
            let mut p = exp.clone();
            p.extend_from_slice(&target.path[1..]);
            expanded = Ref { path: p };
            &expanded
        } else {
            target
        }
    } else {
        target
    };
    // ─────────────────────────────────────────────────────────

    let mut hit: Option<(PathKey, DeclId)> = None;

    // Single scratch buffer for the entire search [cite: 2026-02-08]
    let mut fq_buffer = Vec::with_capacity(scope.len() + effective.path.len());

    for k in (0..=scope.len()).rev() {
        fq_buffer.clear();
        fq_buffer.extend_from_slice(&scope[..k]);
        fq_buffer.extend_from_slice(&effective.path);

        // Borrowed lookup wrapper to avoid unnecessary PathKey allocations
        // Note: Unless you use a custom lookup wrapper, .get() usually requires
        // the same type as the key (PathKey). We'll assume PathKey(Vec<SymId>) here.

        if let Some(&did) = idx.by_path.get(fq_buffer.as_slice()) {
            if let Some((ref first_key, _)) = hit {
                let fk = PathKey(fq_buffer.clone()); // Clone only the buffer, not the strings inside
                return Err(ResolveError::AmbiguousRef {
                    from_scope: fmt_scope(scope, it),
                    target: fmt_path_from_slice(&target.path, it), // Use a slice formatter [cite: 2026-02-08]
                    candidates: vec![fmt_path(first_key, it), fmt_path(&fk, it)],
                });
            }
            hit = Some((PathKey(fq_buffer.clone()), did));
        }
    }

    match hit {
        Some((_, did)) => Ok(did),
        None => Err(ResolveError::UnresolvedRef {
            from_scope: fmt_scope(scope, it),
            target: fmt_path_from_slice(&target.path, it), // No clone needed [cite: 2026-02-08]
        }),
    }
}

pub fn resolve_signed_refs<'a>(
    idx: &Index,
    scope: &[SymId],
    refs: &[SignedRef<'a, SymId>],
    it: &mut Interner,
) -> Result<Vec<ReferenceDraft>, ResolveError> {
    let mut out = Vec::with_capacity(refs.len());

    for sref in refs {
        let atom = match sref {
            SignedRef::Plus(x) | SignedRef::Minus(x) | SignedRef::Neutral(x) => x,
        };

        let did = resolve_ref_to_declid(idx, scope, &atom.target, it)?;

        let anno_r = resolve_anno(idx, scope, &atom.anno, it)?;

        // Both IR fields own their values. Reuse the resolved payload instead
        // of repeating scope lookups and string interning for every weight.
        let weights_r = match &anno_r.value {
            Some(ValueR::List(ws)) => Some(ws.clone()),
            Some(v) => Some(vec![v.clone()]),
            None => None,
        };

        let atom_r = RefAtomR {
            target: did,
            weights: weights_r,
            anno: anno_r,
            // Lowering declares no direction yet: the surface syntax has no port-direction form, so
            // inventing one here would be exactly the sign/direction conflation this axis prevents.
            direction: PortDirection::Unspecified,
        };

        let sref_r = match sref {
            SignedRef::Plus(_) => ReferenceDraft::new(IncidenceSign::Plus, atom_r),
            SignedRef::Minus(_) => ReferenceDraft::new(IncidenceSign::Minus, atom_r),
            SignedRef::Neutral(_) => ReferenceDraft::new(IncidenceSign::Neutral, atom_r),
        };

        out.push(sref_r);
    }

    Ok(out)
}

pub fn resolve_arc_refs<'a>(
    idx: &Index,
    scope: &[SymId],
    arc: &HyperArc<'a, SymId>,
    it: &mut Interner,
) -> Result<Vec<ReferenceDraft>, ResolveError> {
    resolve_signed_refs(idx, scope, &arc.inner.refs, it)
}

pub fn validate_all_refs_sym<'a>(
    d: &AstSym<'a>,
    idx: &Index,
    it: &mut Interner,
) -> Result<(), ResolveError> {
    validate_all_refs_sym_with_prefix(d, &[], idx, it)
}

pub fn validate_all_refs_sym_with_prefix(
    d: &AstSym<'_>,
    prefix: &[SymId],
    idx: &Index,
    it: &mut Interner,
) -> Result<(), ResolveError> {
    ResolutionWalk::new(idx, it, ()).description(d, prefix)
}

/// A phase consumes resolved declarations without owning another AST traversal.
/// `()` validates only; IR construction supplies the checked mutation owner.
pub(crate) trait ResolvedDeclarations {
    fn named(
        &mut self,
        id: DeclId,
        node: DeclNode<AnnoR>,
        bases: Vec<ReferenceDraft>,
        namespace: &[SymId],
    ) -> Result<(), ResolveError>;
    fn arc(
        &mut self,
        parent: DeclId,
        anno: AnnoR,
        refs: Vec<ReferenceDraft>,
    ) -> Result<(), ResolveError>;
}

impl ResolvedDeclarations for () {
    fn named(
        &mut self,
        _: DeclId,
        _: DeclNode<AnnoR>,
        _: Vec<ReferenceDraft>,
        _: &[SymId],
    ) -> Result<(), ResolveError> {
        Ok(())
    }
    fn arc(&mut self, _: DeclId, _: AnnoR, _: Vec<ReferenceDraft>) -> Result<(), ResolveError> {
        Ok(())
    }
}

/// Private state: neither phase can change the path, interner or index outside
/// the traversal. The sink owns the phase-specific state (the IR when lowering).
pub(crate) struct ResolutionWalk<'a, S> {
    idx: &'a Index,
    it: &'a mut Interner,
    path: Vec<SymId>,
    sink: S,
}

impl<'a, S: ResolvedDeclarations> ResolutionWalk<'a, S> {
    pub(crate) fn new(idx: &'a Index, it: &'a mut Interner, sink: S) -> Self {
        Self {
            idx,
            it,
            path: Vec::new(),
            sink,
        }
    }

    pub(crate) fn description(
        &mut self,
        ast: &AstSym<'_>,
        prefix: &[SymId],
    ) -> Result<(), ResolveError> {
        self.path.clear();
        self.path.extend_from_slice(prefix);
        for node in &ast.header {
            self.node(node)?;
        }
        self.items(&ast.items, false)
    }

    fn items(
        &mut self,
        items: &[HyperItem<'_, SymId>],
        arcs_allowed: bool,
    ) -> Result<(), ResolveError> {
        for item in items {
            match item {
                HyperItem::Node(node) => self.node(node)?,
                HyperItem::Edge(edge) => self.named(
                    edge.inner.name,
                    DeclKind::Edge,
                    &edge.anno,
                    &edge.inner.bases,
                    &edge.inner.body,
                )?,
                HyperItem::Arc(arc) if arcs_allowed => self.arc(arc)?,
                HyperItem::Arc(arc) => {
                    return Err(ResolveError::UnexpectedTopLevelArc {
                        detail: format!("{:?}", arc.inner.refs),
                    });
                }
            }
        }
        Ok(())
    }

    fn node(&mut self, node: &NodeDecl<'_, SymId>) -> Result<(), ResolveError> {
        self.named(
            node.inner.name,
            DeclKind::Node,
            &node.anno,
            &node.inner.bases,
            node.inner.body.as_deref().unwrap_or_default(),
        )
    }

    fn named(
        &mut self,
        name: SymId,
        kind: DeclKind,
        anno: &Anno<'_, SymId>,
        bases: &[SignedRef<'_, SymId>],
        body: &[HyperItem<'_, SymId>],
    ) -> Result<(), ResolveError> {
        let scope_len = self.path.len();
        let parent = self
            .idx
            .by_path
            .get(self.path.as_slice())
            .copied()
            .unwrap_or(DeclId::NONE);
        self.path.push(name);
        let result = (|| {
            let id = self.declaration()?;
            // This is the compiler's original order and scope, shared by validation.
            let anno = resolve_anno(self.idx, &self.path, anno, self.it)?;
            let bases = resolve_signed_refs(self.idx, &self.path, bases, self.it)?;
            let node = DeclNode {
                kind,
                name,
                parent,
                anno,
                first_child: DeclId::NONE,
                last_child: DeclId::NONE,
                next_sibling: DeclId::NONE,
            };
            let namespace = if parent.is_none() {
                &self.path[..scope_len]
            } else {
                &[]
            };
            self.sink.named(id, node, bases, namespace)?;
            self.items(body, kind == DeclKind::Edge)
        })();
        self.path.truncate(scope_len);
        result
    }

    fn declaration(&self) -> Result<DeclId, ResolveError> {
        self.idx
            .by_path
            .get(self.path.as_slice())
            .copied()
            .ok_or_else(|| ResolveError::MissingDecl {
                detail: format!("No DeclId for path {:?}", self.path),
            })
    }

    fn arc(&mut self, arc: &HyperArc<'_, SymId>) -> Result<(), ResolveError> {
        let parent = self.declaration()?;
        let anno = resolve_anno(self.idx, &self.path, &arc.anno, self.it)?;
        let refs = resolve_arc_refs(self.idx, &self.path, arc, self.it)?;
        self.sink.arc(parent, anno, refs)
    }
}

fn fmt_scope(scope: &[SymId], it: &Interner) -> String {
    if scope.is_empty() {
        "<root>".into()
    } else {
        scope
            .iter()
            .map(|&s| it.resolve(s))
            .collect::<Vec<_>>()
            .join(".")
    }
}

fn fmt_path(p: &PathKey, it: &Interner) -> String {
    p.0.iter()
        .map(|&s| it.resolve(s))
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower::{IrBuilder, IrLowering};
    use crate::resolution::{const_resolve::resolve_consts, intern_pass::intern_ast};

    #[test]
    fn validation_and_lowering_agree_on_all_reference_positions_and_order() {
        for source in [
            "description { header missing; } root {}",
            "description {} root -> missing;",
            "description {} @edge <tag> missing {}",
            "description {} root: local { local {} }",
            "description {} base {} root: base { base {} }",
            "description {} @edge { (+missing [other]); }",
            "description {} root { (+root); }",
            "description { const A = 2.0; header (A * 3.0); } root { n (A + 1.0); }",
        ] {
            let parsed =
                parser::parse_description(source).unwrap_or_else(|e| panic!("{source}: {e:?}"));
            let mut interned = intern_ast(&parsed);
            resolve_consts(&mut interned.ast, &interned.interner).unwrap();
            let idx = build_index_sym(&interned.ast, &interned.interner).unwrap();
            let validation = validate_all_refs_sym(&interned.ast, &idx, &mut interned.interner);
            let lowering = IrLowering::new(&interned.ast, &idx, &mut interned.interner).build();
            assert_eq!(
                format!("{:?}", validation.err()),
                format!("{:?}", lowering.err()),
                "{source}"
            );
        }
    }

    #[test]
    fn unprocessed_const_is_a_shared_error_and_succeeds_after_const_resolution() {
        let parsed =
            parser::parse_description("description { const A = 2.0; } root (A + 1.0);").unwrap();
        let mut interned = intern_ast(&parsed);
        let idx = build_index_sym(&interned.ast, &interned.interner).unwrap();
        assert!(matches!(
            validate_all_refs_sym(&interned.ast, &idx, &mut interned.interner),
            Err(ResolveError::UnevaluatedConst { .. })
        ));
        assert!(matches!(
            IrLowering::new(&interned.ast, &idx, &mut interned.interner).build(),
            Err(ResolveError::UnevaluatedConst { .. })
        ));
        resolve_consts(&mut interned.ast, &interned.interner).unwrap();
        validate_all_refs_sym(&interned.ast, &idx, &mut interned.interner).unwrap();
        let ir = IrLowering::new(&interned.ast, &idx, &mut interned.interner)
            .with_meta(crate::ir::meta::Meta::new(1234, [7; 16]))
            .build()
            .unwrap();
        assert_eq!(
            ir.decl_annotation(crate::common::ids::DeclId::new(0))
                .unwrap()
                .value,
            Some(ValueR::Num(3.0))
        );
        assert_eq!(ir.meta.as_ref().unwrap().created_at_unix_ns, 1234);
        assert_eq!(ir.meta.as_ref().unwrap().build_id, [7; 16]);
    }
}
