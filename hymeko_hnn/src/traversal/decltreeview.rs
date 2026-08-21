use hymeko::common::ids::DeclId;
use hymeko::ir::ir::Ir;
use crate::traversal::graphview::GraphView;

pub struct DeclTreeView<'a> { pub ir: &'a Ir }

impl<'a> GraphView for DeclTreeView<'a> {
    type Node = DeclId;

    type NeighIter<'b> = hymeko::ir::ir::DeclChildren<'b>
    where Self: 'b;

    fn neighbors<'b>(&'b self, n: DeclId) -> Self::NeighIter<'b> {
        self.ir.children(n)
    }
}