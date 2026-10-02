use crate::common::ids::{DeclId, SymId};
use crate::ir::ir::{AnnoR, DeclNode, Ir, IrConstruction, SignedRefR};
use crate::ir::meta::Meta;
use crate::ir::time::now_ns;
use crate::resolution::interner::Interner;
use crate::resolution::resolve::{Index, ResolutionWalk, ResolveError, ResolvedDeclarations};
use crate::sym_ast::AstSym;

fn default_build_id() -> [u8; 16] {
    [0; 16]
}

impl ResolvedDeclarations for IrConstruction<'_> {
    fn named(
        &mut self,
        id: DeclId,
        node: DeclNode,
        bases: Vec<SignedRefR>,
        namespace: &[SymId],
    ) -> Result<(), ResolveError> {
        self.named(id, node, bases, namespace)
            .map_err(ResolveError::InvalidIr)
    }
    fn arc(
        &mut self,
        parent: DeclId,
        anno: AnnoR,
        refs: Vec<SignedRefR>,
    ) -> Result<(), ResolveError> {
        self.arc(parent, anno, refs)
            .map_err(ResolveError::InvalidIr)
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
    lower_program_to_ir_with_meta(ast, &[], idx, it, meta)
}

pub fn lower_program_to_ir_with_meta(
    root: &AstSym<'_>,
    imported: &[(SymId, &AstSym<'_>)],
    idx: &Index,
    it: &mut Interner,
    meta: Meta,
) -> Result<Ir, ResolveError> {
    let mut ir = Ir::new(meta);
    ir.preallocate_from_index(idx.by_path.len());
    let mut lowering = ResolutionWalk::new(idx, it, IrConstruction::new(&mut ir));
    lowering.description(root, &[])?;
    for (namespace, ast) in imported {
        lowering.description(ast, &[*namespace])?;
    }
    // The tables are complete: their growth slack goes now, in place, rather than riding along
    // with every consumer that keeps the IR (ir-shrink-after-lowering, 2026-09-29).
    ir.shrink_to_fit();
    Ok(ir)
}

pub fn lower_program_to_ir(
    root: &AstSym,
    imported: &[(SymId, &AstSym)],
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
