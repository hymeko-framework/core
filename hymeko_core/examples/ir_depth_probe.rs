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
//! Deliberately duplicates a *minimal* chain builder rather than sharing the
//! richer one in `tests/unified_ir_m05/builder.rs`: a Cargo example cannot link
//! against test-only code. The duplication is ~25 lines and is confined to this
//! file.

use std::process::ExitCode;

use hymeko::common::ids::{DeclId, EdgeId, HyperArcId, NodeId};
use hymeko::common::pathkey::PathKey;
use hymeko::ir::canonical_hash::{CanonHashCfg, canonical_program_hash};
use hymeko::ir::ir::{
    AnnoR, ArcRec, DeclKind, DeclNode, EdgeRec, Ir, NodeRec, RefAtomR, SignedRefR,
};
use hymeko::resolution::interner::Interner;
use hymeko::resolution::resolve::Index;

/// One chain of `depth` nested declarations plus a terminal edge and arc.
fn build_chain(depth: usize) -> (Ir, Index, Interner) {
    let mut ir = Ir::default();
    let mut idx = Index::new();
    let mut it = Interner::new();

    let mut segs: Vec<hymeko::common::ids::SymId> = Vec::new();
    let mut last = DeclId::NONE;

    let push = |ir: &mut Ir, idx: &mut Index, segs: &[hymeko::common::ids::SymId], kind, parent| {
        let id = DeclId::new(ir.decl_nodes.len());
        ir.decl_nodes.push(DeclNode {
            kind,
            name: *segs.last().expect("non-empty path"),
            parent,
            first_child: DeclId::NONE,
            last_child: DeclId::NONE,
            next_sibling: DeclId::NONE,
            anno: AnnoR::default(),
        });
        ir.decl_hash.push(None);
        ir.decl_to_node.push(None);
        ir.decl_to_edge.push(None);
        ir.decl_to_arc.push(None);
        idx.by_path.insert(PathKey(segs.to_vec()), id);
        id
    };

    for i in 0..depth {
        segs.push(it.intern(&format!("l{i}")));
        let d = push(&mut ir, &mut idx, &segs, DeclKind::Node, last);
        let nid = NodeId::new(ir.nodes.len());
        ir.nodes.push(NodeRec::new(d, Vec::new()));
        ir.decl_to_node[d.0] = Some(nid);
        last = d;
    }

    let mut esegs = segs.clone();
    esegs.push(it.intern("e"));
    let ed = push(&mut ir, &mut idx, &esegs, DeclKind::Edge, last);
    let eid = EdgeId::new(ir.edges.len());
    ir.edges.push(EdgeRec::new(ed, Vec::new()));
    ir.decl_to_edge[ed.0] = Some(eid);

    let aid = HyperArcId::new(ir.arcs.len());
    ir.arcs.push(ArcRec {
        anno: AnnoR::default(),
        in_edge: ed,
        refs: vec![SignedRefR::Plus(RefAtomR {
            target: last,
            anno: AnnoR::default(),
            weights: None,
        })],
    });
    ir.arc_hash.push(None);
    ir.edges[eid.0].arcs.push(aid);

    (ir, idx, it)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(depth) = args.get(1).and_then(|s| s.parse::<usize>().ok()) else {
        eprintln!("usage: ir_depth_probe <depth> [--hash]");
        return ExitCode::from(2);
    };
    let do_hash = args.iter().any(|a| a == "--hash");

    eprintln!("probe: building chain depth={depth}");
    let (ir, idx, it) = build_chain(depth);
    eprintln!("probe: built decls={}", ir.decl_nodes.len());

    // Iterative traversal of the intrusive parent chain — this one is a loop,
    // so it is expected to survive any depth.
    let mut cur = DeclId::new(ir.decl_nodes.len() - 2);
    let mut steps = 0usize;
    while cur.is_some() {
        cur = ir.decl_nodes[cur.0].parent;
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
        let h = canonical_program_hash(CanonHashCfg::default(), &idx, &ir, &it);
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
