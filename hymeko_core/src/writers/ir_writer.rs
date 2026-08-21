use crate::ir::ir::{DeclKind, Ir};
use std::io::{self, Write};

use crate::common::ids::DeclId;
use crate::resolution::interner::Interner;

pub struct IrWriter<'a> {
    ir: &'a Ir,
    interner: &'a Interner,
}

impl<'a> IrWriter<'a> {
    pub fn new(ir: &'a Ir, interner: &'a Interner) -> Self {
        Self { ir, interner }
    }

    pub fn write_all<W: Write>(&self, w: &mut W) -> io::Result<()> {
        // Iterate only the roots (items without a parent) directly from the unified nodes
        for (i, node) in self.ir.decl_nodes.iter().enumerate() {
            if node.parent.is_none() {
                self.write_decl(w, DeclId::new(i), 0)?;
            }
        }
        Ok(())
    }

    /// The zero-allocation indentation technique.
    #[inline(always)]
    fn write_indent<W: Write>(&self, w: &mut W, depth: usize) -> io::Result<()> {
        const SPACES: &[u8] = b"                                                                "; // 64 spaces
        let mut needed = depth * 2;
        while needed > 0 {
            let take = needed.min(SPACES.len());
            w.write_all(&SPACES[..take])?;
            needed -= take;
        }
        Ok(())
    }

    fn write_decl<W: Write>(&self, w: &mut W, did: DeclId, depth: usize) -> io::Result<()> {
        // Extract the unified node exactly once for cache efficiency
        let node = &self.ir.decl_nodes[did.0 as usize];
        let name = self.interner.resolve(node.name);

        self.write_indent(w, depth)?;

        match node.kind {
            DeclKind::Node => {
                writeln!(w, "{} {{", name)?;
                self.write_children(w, did, depth + 1)?;
                self.write_indent(w, depth)?;
                writeln!(w, "}}")?;
            }
            DeclKind::Edge => {
                writeln!(w, "@{} {{", name)?;
                self.write_children(w, did, depth + 1)?;
                self.write_indent(w, depth)?;
                writeln!(w, "}}")?;
            }
            DeclKind::HyperArc => {
                writeln!(w, "arc {} {{ /* Arc references pending */ }}", name)?;
            }
        }
        Ok(())
    }
    #[inline(always)]
    fn write_children<W: Write>(&self, w: &mut W, parent: DeclId, depth: usize) -> io::Result<()> {
        for child in self.ir.decl_children(parent) {
            self.write_decl(w, child, depth)?;
        }
        Ok(())
    }
}
