use crate::common::ids::SymId;
use parser::ast::{Description, EdgeInner, HyperItem, NodeInner, SignedRef};

pub type AstSym<'a> = Description<'a, SymId>;

/// Shared declaration traversal contract. A missing node body stays distinct
/// from an explicitly empty body when rebuilding an AST.
pub(crate) trait DeclarationFields<'a, Id> {
    fn fields(&self) -> (&Id, &[SignedRef<'a, Id>], Option<&[HyperItem<'a, Id>]>);
    fn fields_mut(&mut self) -> (&mut [SignedRef<'a, Id>], &mut [HyperItem<'a, Id>]);
}

impl<'a, Id> DeclarationFields<'a, Id> for NodeInner<'a, Id> {
    fn fields(&self) -> (&Id, &[SignedRef<'a, Id>], Option<&[HyperItem<'a, Id>]>) {
        (&self.name, &self.bases, self.body.as_deref())
    }
    fn fields_mut(&mut self) -> (&mut [SignedRef<'a, Id>], &mut [HyperItem<'a, Id>]) {
        (
            &mut self.bases,
            self.body.as_deref_mut().unwrap_or_default(),
        )
    }
}

impl<'a, Id> DeclarationFields<'a, Id> for EdgeInner<'a, Id> {
    fn fields(&self) -> (&Id, &[SignedRef<'a, Id>], Option<&[HyperItem<'a, Id>]>) {
        (&self.name, &self.bases, Some(&self.body))
    }
    fn fields_mut(&mut self) -> (&mut [SignedRef<'a, Id>], &mut [HyperItem<'a, Id>]) {
        (&mut self.bases, &mut self.body)
    }
}
