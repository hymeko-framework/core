// src/resolve.rs
use crate::common::ids::{DeclId, SymId};
use crate::common::pathkey::PathKey;
use crate::ir::ir::{AnnoR, RefAtomR, SignedRefR, ValueR};
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
    UnexpectedTopLevelArc {
        detail: String,
    },
}

pub fn build_index_sym<'a>(d: &AstSym<'a>, it: &Interner) -> Result<Index, ResolveError> {
    let mut idx = Index::default();
    let mut next: usize = 0;
    let mut path = Vec::new();

    for n in &d.header {
        index_node(&mut idx, &mut next, &mut path, n, it)?;
    }

    index_items(&mut idx, &mut next, &mut path, &d.items, it)?;

    Ok(idx)
}

/// Process `using path as alias` statements.
/// Call AFTER build_index_sym and AFTER imports are indexed.
pub fn apply_usings(
    idx: &mut Index,
    usings: &[UsingStmt<SymId>],
    import_namespaces: &[SymId],
    it: &Interner,
) -> Result<(), ResolveError> {
    for u in usings {
        // Try each import namespace as prefix (e.g., kinematics.elements
        // lives at [meta_kinematics, kinematics, elements] in the index)
        let mut resolved = false;
        for &ns in import_namespaces {
            let mut prefixed = vec![ns];
            prefixed.extend_from_slice(&u.path.path);
            let key = PathKey(prefixed.clone());
            if idx.by_path.contains_key(&key) {
                // Store FULL path so alias expansion maps directly to index entries
                idx.aliases.insert(u.alias, prefixed);
                resolved = true;
                break;
            }
        }
        if !resolved {
            // Try without prefix (root-level refs)
            let key = PathKey(u.path.path.clone());
            if idx.by_path.contains_key(&key) {
                idx.aliases.insert(u.alias, u.path.path.clone());
            } else {
                return Err(ResolveError::UnresolvedRef {
                    from_scope: "<using>".into(),
                    target: u
                        .path
                        .path
                        .iter()
                        .map(|&s| it.resolve(s))
                        .collect::<Vec<_>>()
                        .join("."),
                });
            }
        }
    }
    Ok(())
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
        Value::Expr(_) => unreachable!(
            "const expression in resolve_value — const-resolution pass must run before resolve"
        ),
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

pub fn resolve_arc_anno<'a>(
    idx: &Index,
    scope: &[SymId],
    arc: &HyperArc<'a, SymId>,
    it: &mut Interner,
) -> Result<AnnoR, ResolveError> {
    resolve_anno(idx, scope, &arc.anno, it)
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
    // ── Alias expansion ──────────────────────────────────────
    let expanded;
    let effective = if !target.path.is_empty() {
        if let Some(exp) = idx.aliases.get(&target.path[0]) {
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
) -> Result<Vec<SignedRefR>, ResolveError> {
    let mut out = Vec::with_capacity(refs.len());

    for sref in refs {
        let atom = match sref {
            SignedRef::Plus(x) | SignedRef::Minus(x) | SignedRef::Neutral(x) => x,
        };

        let did = resolve_ref_to_declid(idx, scope, &atom.target, it)?;

        let anno_r = resolve_anno(idx, scope, &atom.anno, it)?;

        let weights_r = match &atom.anno.value {
            Some(Value::List(ws)) => {
                let resolved = ws
                    .iter()
                    .map(|w| resolve_value(idx, scope, w, it))
                    .collect::<Result<Vec<ValueR>, _>>()?;
                Some(resolved)
            }
            Some(v) => {
                let resolved = resolve_value(idx, scope, v, it)?;
                Some(vec![resolved])
            }
            None => None,
        };

        let atom_r = RefAtomR {
            target: did,
            weights: weights_r,
            anno: anno_r,
        };

        let sref_r = match sref {
            SignedRef::Plus(_) => SignedRefR::Plus(atom_r),
            SignedRef::Minus(_) => SignedRefR::Minus(atom_r),
            SignedRef::Neutral(_) => SignedRefR::Neutral(atom_r),
        };

        out.push(sref_r);
    }

    Ok(out)
}

pub fn resolve_node_bases<'a>(
    idx: &Index,
    scope: &[SymId],
    refs: &[SignedRef<'a, SymId>],
    it: &mut Interner,
) -> Result<Vec<SignedRefR>, ResolveError> {
    resolve_signed_refs(idx, scope, refs, it)
}

pub fn resolve_arc_refs<'a>(
    idx: &Index,
    scope: &[SymId],
    arc: &HyperArc<'a, SymId>,
    it: &mut Interner,
) -> Result<Vec<SignedRefR>, ResolveError> {
    resolve_signed_refs(idx, scope, &arc.inner.refs, it)
}

pub fn validate_all_refs_sym<'a>(
    d: &AstSym<'a>,
    idx: &Index,
    it: &mut Interner,
) -> Result<(), ResolveError> {
    let mut path = Vec::new();
    validate_items(&mut path, &d.items, idx, it)
}

pub fn validate_all_refs_sym_with_prefix<'a>(
    d: &AstSym<'a>,
    prefix: &[SymId],
    idx: &Index,
    it: &mut Interner,
) -> Result<(), ResolveError> {
    let mut path = prefix.to_vec();
    validate_items(&mut path, &d.items, idx, it)
}

fn validate_items<'a>(
    path: &mut Vec<SymId>,
    items: &[HyperItem<'a, SymId>],
    idx: &Index,
    it: &mut Interner,
) -> Result<(), ResolveError> {
    for item in items {
        match item {
            HyperItem::Node(n) => {
                let _ = resolve_node_bases(idx, path.as_slice(), &n.inner.bases, it)?;
                if let Some(body) = &n.inner.body {
                    let scope_len = path.len();
                    path.push(n.inner.name);
                    validate_items(path, body, idx, it)?;
                    path.truncate(scope_len);
                }
            }
            HyperItem::Edge(e) => {
                let _ = resolve_node_bases(idx, path.as_slice(), &e.inner.bases, it)?;
                let scope_len = path.len();
                path.push(e.inner.name);
                validate_items(path, &e.inner.body, idx, it)?;
                path.truncate(scope_len);
            }
            HyperItem::Arc(a) => {
                let _ = resolve_arc_refs(idx, path.as_slice(), a, it)?;
                if let Some(v) = &a.anno.value {
                    validate_value(path.as_slice(), v, idx, it)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_value<'a>(
    scope: &[SymId],
    v: &Value<'a, SymId>,
    idx: &Index,
    it: &Interner,
) -> Result<(), ResolveError> {
    match v {
        Value::Ref(r) => {
            let _ = resolve_ref_to_declid(idx, scope, r, it)?;
        }
        Value::List(xs) => {
            for x in xs {
                validate_value(scope, x, idx, it)?;
            }
        }
        Value::Str(_) | Value::Num(_) => {}
        // Tier B: see resolve_value above.
        Value::Expr(_) => unreachable!(
            "const expression in validate_value — const-resolution pass must run before validate_value"
        ),
    }
    Ok(())
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
