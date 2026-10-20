//! Part E — dangerous-depth probe, run as a **child process**.
//!
//! APPROVED-CORE-EDIT: unified-ir-m0-5-tests-and-benchmarks (2026-08-28).
//! Diagnostic only; not production code and not linked into the library.
//!
//! `canonical_hash::compute_subtree_hashes_with_kinds` contains a recursive
//! inner `fn rec(...)` that descends one stack frame per hierarchy level. A
//! deep enough declaration chain therefore overflows the stack, and a stack
//! overflow **aborts the process** — it cannot be caught by `catch_unwind`. Run
//! inside the integration-test binary it would take 150+ unrelated tests down
//! with it, so the deep cases live here and the test harness invokes this
//! binary and reads its exit status.
//!
//! Usage:
//! ```text
//! cargo run --release --example ir_depth_probe -- <depth> [--hash]
//! ```
//! Exit status: `0` traversed and (optionally) hashed successfully; `2` bad
//! usage. A crash (stack overflow) surfaces as an abnormal termination status,
//! which is exactly the signal the caller is looking for.
//!
//! Chain input uses the same Core construction owner as source lowering.

use std::process::ExitCode;

use hymeko::common::ids::{DeclId, SymId};
use hymeko::common::pathkey::PathKey;
use hymeko::ir::canonical_hash::{Canon, CanonHashCfg};
use hymeko::ir::ir::{
    AnnoR, ArcPlacement, DeclKind, DeclNode, IncidenceSign, Ir, IrBuilder, IrCapacities,
    IrConstruction, ReferenceDraft, dispatch,
};
use hymeko::resolution::interner::Interner;
use hymeko::resolution::resolve::Index;

/// Input adapter for a depth probe; Core owns record/link/binding population.
struct ChainBuilder {
    depth: usize,
    index: Index,
    interner: Interner,
}
impl ChainBuilder {
    fn new(depth: usize) -> Self {
        Self {
            depth,
            index: Index::new(),
            interner: Interner::new(),
        }
    }
    fn named(
        &mut self,
        construction: &mut IrConstruction<'_>,
        path: &[SymId],
        kind: DeclKind,
        parent: DeclId,
    ) -> Result<DeclId, dispatch::DispatchError> {
        let mut node = DeclNode::root(
            kind,
            *path.last().expect("non-empty path"),
            AnnoR::default(),
        );
        node.parent = parent;
        let id = construction.append_named(node, Vec::new(), &[])?;
        self.index.by_path.insert(PathKey(path.to_vec()), id);
        Ok(id)
    }
}
impl IrBuilder for ChainBuilder {
    type Output = (Ir, Index, Interner);
    type Error = dispatch::DispatchError;
    fn capacities(&self) -> IrCapacities {
        IrCapacities {
            declarations: self.depth + 2,
            nodes: self.depth,
            edges: 1,
            arcs: 1,
        }
    }
    fn populate(&mut self, construction: &mut IrConstruction<'_>) -> Result<(), Self::Error> {
        let mut path = Vec::with_capacity(self.depth + 1);
        let mut last = DeclId::NONE;
        for i in 0..self.depth {
            path.push(self.interner.intern(&format!("l{i}")));
            last = self.named(construction, &path, DeclKind::Node, last)?;
        }
        path.push(self.interner.intern("e"));
        let owner = self.named(construction, &path, DeclKind::Edge, last)?;
        construction.append_arc(
            ArcPlacement::under_owner(owner),
            AnnoR::default(),
            vec![ReferenceDraft::plain(last, IncidenceSign::Plus)],
        )?;
        Ok(())
    }
    fn finish(self, ir: Ir) -> Self::Output {
        (ir, self.index, self.interner)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(depth) = args.get(1).and_then(|s| s.parse::<usize>().ok()) else {
        eprintln!("usage: ir_depth_probe <depth> [--hash]");
        return ExitCode::from(2);
    };
    let do_hash = args.iter().any(|a| a == "--hash");

    eprintln!("probe: building chain depth={depth}");
    let (ir, _idx, it) = ChainBuilder::new(depth).build().expect("valid chain");
    eprintln!("probe: built decls={}", ir.decl_nodes.len());

    // Iterative traversal of the intrusive parent chain — this one is a loop,
    // so it is expected to survive any depth.
    let mut cur = DeclId::new(depth.saturating_sub(1));
    let mut steps = 0usize;
    while cur.is_some() {
        cur = ir.decl_nodes[cur.raw()].parent;
        steps += 1;
        if steps > depth + 4 {
            eprintln!("probe: FAIL parent walk did not terminate");
            return ExitCode::from(3);
        }
    }
    eprintln!("probe: parent_walk_ok steps={steps}");

    if do_hash {
        // The recursive path. If it overflows, the process dies here and the
        // caller sees an abnormal exit status.
        eprintln!("probe: hashing (recursive subtree fold)…");
        let h = Canon::new(&ir, &it).structure(CanonHashCfg::default()).0;
        eprintln!(
            "probe: hash_ok prefix={:02x}{:02x}{:02x}",
            h.0[0], h.0[1], h.0[2]
        );
    }

    // Drop is also a candidate for deep recursion in linked structures; `Ir`
    // holds flat `Vec`s, so this is expected to be iterative. Timed implicitly
    // by the process exiting cleanly.
    drop(ir);
    eprintln!("probe: drop_ok");
    ExitCode::SUCCESS
}
