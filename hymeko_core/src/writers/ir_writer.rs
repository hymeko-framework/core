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
        // M2b: dense scan through the typed-id iterator. Yields the `DeclId`
        // directly, so the id no longer has to be reconstructed from an
        // enumeration index — which is exactly the coupling to ordinal storage
        // the accessor layer exists to remove.
        for (did, node) in self.ir.decls() {
            if node.parent.is_none() {
                self.write_decl(w, did, 0)?;
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
        // M2b: point lookup through the accessor, extracted once for cache
        // efficiency as before. A dangling child link now ends this subtree
        // instead of panicking — a writer should not abort the process on
        // malformed input it is being asked to serialise.
        let Some(node) = self.ir.decl_node(did) else {
            return Ok(());
        };
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
        // M2b: `child_decls` is the total form. `decl_children` resolves each
        // step through `next_sibling`, which indexes directly and therefore
        // panics on a dangling link and loops forever on a sibling cycle. On
        // well-formed input the two yield the same sequence — asserted over all
        // 96 fixtures — so this is a strict robustness gain, not a behaviour
        // change for any valid IR.
        for child in self.ir.child_decls(parent) {
            self.write_decl(w, child, depth)?;
        }
        Ok(())
    }
}
