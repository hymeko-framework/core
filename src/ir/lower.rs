use crate::common::ids::{DeclId, EdgeId, HyperArcId, NodeId, SymId};
use crate::ir::ir::{ArcRec, DeclKind, EdgeRec, Ir, NodeRec};
use crate::ir::meta::Meta;
use crate::ir::time::now_ns;
use crate::resolution::interner::Interner;
use crate::resolution::resolve::{
    Index, ResolveError, resolve_anno, resolve_arc_refs, resolve_node_bases,
};
use crate::sym_ast::AstSym;
use parser::ast::{EdgeDecl, HyperArc, HyperItem, NodeDecl};

fn default_build_id() -> [u8; 16] {
    [0u8; 16]
}

fn parent_decl(idx: &Index, scope: &[SymId]) -> DeclId {
    if scope.is_empty() {
        DeclId::NONE
    } else {
        *idx.by_path.get(scope).unwrap_or(&DeclId::NONE)
    }
}

fn decl_id_of(idx: &Index, path: &[SymId]) -> Result<DeclId, ResolveError> {
    idx.by_path
        .get(path)
        .copied()
        .ok_or_else(|| ResolveError::MissingDecl {
            detail: format!("No DeclId for path {:?}", path),
        })
}

fn link_decl_child(ir: &mut Ir, parent: DeclId, child: DeclId) {
    let p = parent.0;
    if ir.decl_nodes[p].first_child.is_none() {
        ir.decl_nodes[p].first_child = child;
        ir.decl_nodes[p].last_child = child;
    } else {
        let last = ir.decl_nodes[p].last_child;
        ir.decl_nodes[last.0].next_sibling = child;
        ir.decl_nodes[p].last_child = child;
    }
}

pub fn lower_to_ir(ast: &AstSym, idx: &Index, it: &mut Interner) -> Result<Ir, ResolveError> {
    lower_to_ir_with_meta(
        ast,
        idx,
        it,
        Meta {
            created_at_unix_ns: now_ns(),
            build_id: default_build_id(),
        },
    )
}

pub fn lower_to_ir_with_meta(
    ast: &AstSym,
    idx: &Index,
    it: &mut Interner,
    meta: Meta,
) -> Result<Ir, ResolveError> {
    let mut ir = Ir::new(meta);
    let mut path = Vec::new();
    ir.preallocate_from_index(idx.by_path.len());
    for n in &ast.header {
        lower_node(&mut ir, idx, it, &mut path, n)?;
    }
    lower_items(&mut ir, idx, it, &mut path, &ast.items)?;
    Ok(ir)
}

fn lower_items(
    ir: &mut Ir,
    idx: &Index,
    it: &mut Interner,
    path: &mut Vec<SymId>,
    items: &[HyperItem<SymId>],
) -> Result<(), ResolveError> {
    for item in items {
        match item {
            HyperItem::Node(n) => lower_node(ir, idx, it, path, n)?,
            HyperItem::Edge(e) => lower_edge(ir, idx, it, path, e)?,
            HyperItem::Arc(a) => {
                return Err(ResolveError::UnexpectedTopLevelArc {
                    detail: format!("{:?}", a.inner.refs),
                });
            }
        }
    }
    Ok(())
}

fn lower_node(
    ir: &mut Ir,
    idx: &Index,
    it: &mut Interner,
    path: &mut Vec<SymId>,
    n: &NodeDecl<SymId>,
) -> Result<(), ResolveError> {
    // Fully qualified path = scope + [name_sym]
    let scope_len = path.len();
    path.push(n.inner.name);

    let did = decl_id_of(idx, path)?;
    // Replace the old flat assignments:
    ir.decl_nodes[did.0].kind = DeclKind::Node;
    ir.decl_nodes[did.0].name = n.inner.name;

    let parent_did = parent_decl(idx, &path[..scope_len]);
    ir.decl_nodes[did.0].parent = parent_did;

    let anno = resolve_anno(idx, path, &n.anno, it)?;
    ir.decl_nodes[did.0].anno = anno;

    let nid = NodeId::new(ir.nodes.len());
    let bases = resolve_node_bases(idx, path, &n.inner.bases, it)?;
    ir.nodes.push(NodeRec::new(did, bases));
    ir.decl_to_node[did.0] = Some(nid);

    if parent_did.is_some() {
        link_decl_child(ir, parent_did, did);
    }

    // children
    if let Some(body) = &n.inner.body {
        lower_items(ir, idx, it, path, body)?;
    }
    path.truncate(scope_len);
    Ok(())
}

fn lower_arc(
    ir: &mut Ir,
    idx: &Index,
    it: &mut Interner,
    edge_decl: DeclId,
    path: &[SymId],
    eid: EdgeId,
    a: &HyperArc<SymId>,
) -> Result<(), ResolveError> {
    // 1) Push directly onto the vectors (amortized O(1), zero resize-tracking overhead)
    let arc_decl = ir.push_anonymous_arc();
    let idx_usize = arc_decl.0;

    // 2) fill decl tables via the unified node
    ir.decl_nodes[idx_usize].parent = edge_decl;

    // 3) resolve arc payload once
    let anno = resolve_anno(idx, &path, &a.anno, it)?;
    let refs = resolve_arc_refs(idx, &path, a, it)?;

    // 4) store decl anno
    ir.decl_nodes[idx_usize].anno = anno.clone();

    // 5) create ArcRec + mapping
    let aid = HyperArcId::new(ir.arcs.len());
    ir.arcs.push(ArcRec {
        anno,
        in_edge: edge_decl,
        refs,
    });
    ir.decl_to_arc[idx_usize] = Some(aid);

    // 6) link into edge's decl-children chain
    link_decl_child(ir, edge_decl, arc_decl);

    // 7) keep per-edge arc list once
    ir.edges[eid.0].arcs.push(aid);

    Ok(())
}

fn lower_edge(
    ir: &mut Ir,
    idx: &Index,
    it: &mut Interner,
    path: &mut Vec<SymId>,
    e: &EdgeDecl<SymId>,
) -> Result<(), ResolveError> {
    let scope_len = path.len();
    path.push(e.inner.name);

    let did = decl_id_of(idx, &path)?;

    let idx_usize = did.0;

    ir.decl_nodes[idx_usize].kind = DeclKind::Edge;
    ir.decl_nodes[idx_usize].name = e.inner.name;

    let parent_did = parent_decl(idx, &path[..scope_len]);
    ir.decl_nodes[idx_usize].parent = parent_did;

    let anno = resolve_anno(idx, path, &e.anno, it)?;
    ir.decl_nodes[idx_usize].anno = anno;

    // EdgeId is assigned after resolving the edge-level anno, so that we can
    let eid = EdgeId::new(ir.edges.len());
    let bases = resolve_node_bases(idx, path, &e.inner.bases, it)?;
    ir.edges.push(EdgeRec::new(did, bases));
    ir.decl_to_edge[idx_usize] = Some(eid);

    if parent_did.is_some() {
        link_decl_child(ir, parent_did, did);
    }

    let edge_decl = did;

    for item in &e.inner.body {
        match item {
            HyperItem::Arc(a) => {
                lower_arc(ir, idx, it, edge_decl, path, eid, a)?;
            }
            HyperItem::Node(n) => {
                lower_node(ir, idx, it, path, n)?;
            }
            HyperItem::Edge(ed) => {
                lower_edge(ir, idx, it, path, ed)?;
            }
        }
    }
    path.truncate(scope_len);

    Ok(())
}

pub fn lower_into_ir(
    ir: &mut Ir,
    ast: &AstSym,
    idx: &Index,
    it: &mut Interner,
    prefix: &[SymId],
) -> Result<(), ResolveError> {
    let mut path = prefix.to_vec();
    for n in &ast.header {
        lower_node(ir, idx, it, &mut path, n)?;
    }
    lower_items(ir, idx, it, &mut path, &ast.items)?;
    Ok(())
}

pub fn lower_program_to_ir_with_meta(
    root: &AstSym,
    imported: &[(SymId, AstSym)],
    idx: &Index,
    it: &mut Interner,
    meta: Meta,
) -> Result<Ir, ResolveError> {
    let mut ir = Ir::new(meta);
    // MATHEMATICAL PRE-ALLOCATION: We know exactly how many named decls exist
    ir.preallocate_from_index(idx.by_path.len());
    // 2) lower root + deps into IR under their namespaces (prefixes)
    lower_into_ir(&mut ir, root, idx, it, &[])?;
    for (ns, dep_ast) in imported {
        lower_into_ir(&mut ir, dep_ast, idx, it, &[*ns])?;
    }
    Ok(ir)
}

pub fn lower_program_to_ir(
    root: &AstSym,
    imported: &[(SymId, AstSym)],
    idx: &Index,
    it: &mut Interner,
) -> Result<Ir, ResolveError> {
    lower_program_to_ir_with_meta(
        root,
        imported,
        idx,
        it,
        Meta {
            created_at_unix_ns: now_ns(),
            build_id: default_build_id(),
        },
    )
}
