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

    /// Write every root declaration and its subtree.
    ///
    /// # Errors
    ///
    /// Propagates any write error from `w`, and additionally returns
    /// [`io::ErrorKind::InvalidData`] when the `Ir` is structurally malformed
    /// in a way that prevents faithful serialisation:
    ///
    /// * a declaration named by a hierarchy link does not exist;
    /// * a sibling chain is cyclic.
    ///
    /// This is deliberate. The accessor layer is total and `Option`-valued so
    /// that *reads* never abort; a serialiser is a strict consumer, and silently
    /// emitting a truncated document would leave nothing downstream able to
    /// detect it. For a valid `Ir` the output is unchanged, byte for byte.
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
        // Point lookup through the accessor, extracted once for cache
        // efficiency as before.
        //
        // M2b-corrective: an absent declaration is an **error**, not a quiet
        // stop. The accessor layer is deliberately total and `Option`-valued;
        // turning absence into a visible failure is the *consumer's* job, and a
        // serialiser handed a declaration that does not exist has been asked to
        // write something that is not there. Returning `Ok(())` produced
        // silently truncated output, which is worse than either a panic or an
        // error because nothing downstream can tell it happened.
        let Some(node) = self.ir.decl_node(did) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("declaration {} does not exist in this Ir", did.raw()),
            ));
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
        // M2b-corrective: walk the sibling chain explicitly rather than through
        // `child_decls`.
        //
        // `child_decls` is the *total* iterator: it stops on a dangling link and
        // is bounded against a cycle, which is right for a read API but wrong
        // here — stopping and finishing normally are indistinguishable to the
        // caller, so malformed input produced quietly truncated output.
        //
        // This is not a second coherence validator. It checks exactly the two
        // links it is about to follow, which is the minimum a serialiser must
        // do to report what it could not write. Structural diagnosis of *why*
        // an `Ir` is malformed remains `ir::coherence`'s job.
        let mut cur = self.ir.first_child_of(parent);
        // A chain longer than the declaration count has revisited a node.
        let mut budget = self.ir.decl_count();

        while let Some(child) = cur {
            if budget == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "sibling chain under declaration {} exceeds {} links; it is cyclic",
                        parent.raw(),
                        self.ir.decl_count()
                    ),
                ));
            }
            budget -= 1;

            // A dangling child is reported by `write_decl`, which already has
            // to resolve the declaration. Pre-checking here would name the
            // parent in the message but would make that check unreachable —
            // dead code that coverage correctly refused to call covered.
            self.write_decl(w, child, depth)?;
            cur = self.ir.next_sibling_of(child);
        }
        Ok(())
    }
}
