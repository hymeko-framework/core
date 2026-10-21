//! Source lowering is an input adapter for the shared IR builder.
use crate::common::ids::SymId;
pub use crate::ir::ir::IrBuilder;
use crate::ir::ir::{
    AnnoR, ArcPlacement, DeclNode, Ir, IrCapacities, IrConstruction, ReferenceDraft,
};
use crate::ir::meta::Meta;
use crate::ir::time::now_ns;
use crate::resolution::interner::Interner;
use crate::resolution::resolve::{Index, ResolutionWalk, ResolveError, ResolvedDeclarations};
use crate::sym_ast::AstSym;

impl ResolvedDeclarations for &mut IrConstruction<'_> {
    fn named(
        &mut self,
        id: crate::common::ids::DeclId,
        node: DeclNode<AnnoR>,
        bases: Vec<ReferenceDraft>,
        namespace: &[SymId],
    ) -> Result<(), ResolveError> {
        IrConstruction::named(self, id, node, bases, namespace).map_err(ResolveError::InvalidIr)
    }
    fn arc(
        &mut self,
        parent: crate::common::ids::DeclId,
        anno: AnnoR,
        refs: Vec<ReferenceDraft>,
    ) -> Result<(), ResolveError> {
        self.append_arc(ArcPlacement::under_owner(parent), anno, refs)
            .map(|_| ())
            .map_err(ResolveError::InvalidIr)
    }
}

/// Borrowed source/index input for canonical IR construction. No AST is cloned.
/// All declaration, record, hierarchy and dispatch writes go through IrConstruction.
pub struct IrLowering<'a, 'source> {
    root: &'a AstSym<'source>,
    imported: &'a [(SymId, &'a AstSym<'source>)],
    index: &'a Index,
    interner: &'a mut Interner,
    meta: Meta,
}

impl<'a, 'source> IrLowering<'a, 'source> {
    /// Select the root source and its existing name/ID domain.
    pub fn new(root: &'a AstSym<'source>, index: &'a Index, interner: &'a mut Interner) -> Self {
        Self {
            root,
            imported: &[],
            index,
            interner,
            meta: Meta {
                created_at_unix_ns: now_ns(),
                build_id: [0; 16],
            },
        }
    }
    /// Include the program's already interned modules without copying them.
    pub fn with_imports(mut self, imported: &'a [(SymId, &'a AstSym<'source>)]) -> Self {
        self.imported = imported;
        self
    }
    /// Supply deterministic construction metadata, for persisted/reproducible input.
    pub fn with_meta(mut self, meta: Meta) -> Self {
        self.meta = meta;
        self
    }
}

impl IrBuilder for IrLowering<'_, '_> {
    type Output = Ir;
    type Error = ResolveError;
    fn capacities(&self) -> IrCapacities {
        IrCapacities {
            declarations: self.index.by_path.len(),
            ..Default::default()
        }
    }
    fn populate(&mut self, construction: &mut IrConstruction<'_>) -> Result<(), ResolveError> {
        construction.set_meta(self.meta.clone());
        construction
            .prepare_named_slots(self.index.by_path.len())
            .map_err(ResolveError::InvalidIr)?;
        let mut lowering = ResolutionWalk::new(self.index, self.interner, construction);
        lowering.description(self.root, &[])?;
        for (namespace, ast) in self.imported {
            lowering.description(ast, &[*namespace])?;
        }
        Ok(())
    }
    fn finish(self, mut ir: Ir) -> Ir {
        ir.shrink_to_fit();
        ir
    }
}
