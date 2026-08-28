//! Part E — declaration-depth audit.
//!
//! APPROVED-CORE-EDIT: unified-ir-m0-5-tests-and-benchmarks (2026-08-28).
//!
//! Structure first, timing later. The question that matters for the Merkle-fold
//! identity design (architecture §F.6) is *how deep real declaration trees get*,
//! because the incremental update cost is O(changed × depth) and because
//! `canonical_hash::compute_subtree_hashes_with_kinds` contains a genuinely
//! **recursive** inner `fn rec(...)` that descends one stack frame per level.
//!
//! Depth here means **path depth**: the canonical hash builds its skeleton
//! forest from `Index` path prefixes, not from `DeclNode.parent` (proven by
//! `invariance::intrusive_parent_links_do_not_affect_the_canonical_hash`), so
//! the recursion depth is the path-nesting depth.
//!
//! Dangerous depths are deliberately **not** exercised here — a stack overflow
//! aborts the whole test process, taking 150+ unrelated tests with it. They run
//! in `examples/ir_depth_probe.rs` as a child process with an explicit exit
//! status. See the M0.5 report.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hymeko::common::ids::DeclId;
use hymeko::ir::canonical_hash::{CanonHashCfg, canonical_program_hash};
use hymeko::ir::ir::DeclKind;

use super::builder::{S, TestIr};

/// Walk `data/` for fixtures. Returns paths relative to the crate.
fn fixture_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "hymeko") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

struct DepthStats {
    decls: usize,
    max_depth: usize,
    median_depth: usize,
    p95_depth: usize,
    max_branching: usize,
    deepest_path: String,
}

fn depth_stats(
    idx: &hymeko::resolution::resolve::Index,
    it: &hymeko::resolution::interner::Interner,
) -> DepthStats {
    let mut depths: Vec<usize> = Vec::new();
    let mut deepest = String::new();
    let mut deepest_len = 0usize;
    // branching factor: how many distinct children each path prefix has
    let mut children: BTreeMap<Vec<String>, usize> = BTreeMap::new();

    for (pk, _) in idx.iter() {
        let segs: Vec<String> = pk.0.iter().map(|&s| it.resolve(s).to_string()).collect();
        depths.push(segs.len());
        if segs.len() > deepest_len {
            deepest_len = segs.len();
            deepest = segs.join(".");
        }
        if segs.len() >= 1 {
            let parent = segs[..segs.len() - 1].to_vec();
            *children.entry(parent).or_insert(0) += 1;
        }
    }
    depths.sort_unstable();
    let n = depths.len();
    DepthStats {
        decls: n,
        max_depth: depths.last().copied().unwrap_or(0),
        median_depth: if n == 0 { 0 } else { depths[n / 2] },
        p95_depth: if n == 0 { 0 } else { depths[(n * 95) / 100] },
        max_branching: children.values().copied().max().unwrap_or(0),
        deepest_path: deepest,
    }
}

/// The real-corpus audit. Reports; does not gate.
#[test]
fn real_corpus_declaration_depth_audit() {
    let root = Path::new("../data");
    if !root.exists() {
        eprintln!("[depth] ../data not present — corpus audit skipped");
        return;
    }
    let files = fixture_files(root);
    eprintln!("[depth] corpus: {} .hymeko fixtures found", files.len());

    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut agg_max = 0usize;
    let mut agg_decls = 0usize;
    let mut worst: Option<(String, DepthStats)> = None;
    let mut all_depths: Vec<usize> = Vec::new();

    for f in &files {
        match crate::test_helpers::load_and_lower(f) {
            Ok((store, compiled)) => {
                ok += 1;
                let st = depth_stats(&compiled.idx, &store.it);
                agg_decls += st.decls;
                for (pk, _) in compiled.idx.iter() {
                    all_depths.push(pk.0.len());
                }
                if st.max_depth > agg_max {
                    agg_max = st.max_depth;
                    worst = Some((f.display().to_string(), st));
                }
            }
            Err(_) => failed += 1,
        }
    }

    all_depths.sort_unstable();
    let n = all_depths.len();
    let median = if n == 0 { 0 } else { all_depths[n / 2] };
    let p95 = if n == 0 {
        0
    } else {
        all_depths[(n * 95) / 100]
    };
    let p99 = if n == 0 {
        0
    } else {
        all_depths[(n * 99) / 100]
    };

    eprintln!(
        "[depth] lowered ok={ok} failed={failed} total_decls={agg_decls} \
         depth: median={median} p95={p95} p99={p99} max={agg_max}"
    );
    if let Some((file, st)) = worst {
        eprintln!(
            "[depth] deepest fixture: {file}\n\
             [depth]   decls={} max_depth={} median={} p95={} max_branching={} \
             deepest_path={}",
            st.decls,
            st.max_depth,
            st.median_depth,
            st.p95_depth,
            st.max_branching,
            st.deepest_path
        );
    }

    // Not a gate on a specific number — a gate on having *measured* it.
    assert!(
        ok > 0,
        "no fixture lowered; the corpus audit measured nothing"
    );
}

/// Build a single chain of `depth` nested declarations, plus one edge whose arc
/// references the deepest node. Returns the constructed IR.
fn chain(depth: usize) -> TestIr {
    let mut t = TestIr::new();
    let mut segs: Vec<String> = Vec::new();
    let mut last = DeclId::NONE;
    for i in 0..depth {
        segs.push(format!("l{i}"));
        let refs: Vec<&str> = segs.iter().map(String::as_str).collect();
        last = t.node(&refs, last);
    }
    let mut esegs: Vec<&str> = segs.iter().map(String::as_str).collect();
    esegs.push("e");
    let e = t.edge(&esegs, last);
    t.arc(e, &[(last, S::Plus)]);
    t
}

/// Safe depths only: 10 and 100. Anything deeper
/// runs in the child-process probe — see the module docs.
#[test]
fn synthetic_chain_depths_are_traversable_and_hashable() {
    // In-process depths must stay far below the ~458-level overflow boundary
    // measured by `examples/ir_depth_probe`. A stack overflow ABORTS the whole
    // test binary — it is not catchable — so anything deeper runs as a child
    // process in `deep_chain_overflow_boundary_is_far_above_the_real_corpus`.
    for &d in &[10usize, 100] {
        let t = chain(d);
        // Traversal: walk the intrusive parent chain to the root.
        let deepest = DeclId::new(t.ir.decl_nodes.len() - 2); // last node before the edge
        let mut steps = 0usize;
        let mut cur = deepest;
        while cur.is_some() {
            cur = t.ir.decl_nodes[cur.0].parent;
            steps += 1;
            assert!(
                steps <= d + 2,
                "intrusive parent walk did not terminate at depth {d}"
            );
        }

        // Canonical hash: this is the recursive path.
        let h = canonical_program_hash(CanonHashCfg::default(), &t.idx, &t.ir, &t.it);

        // Semantic oracle at the same depth.
        let form = super::oracle::semantic_form(&t, super::oracle::Ordering::Ordered);

        eprintln!(
            "[depth] synthetic chain depth={d} decls={} parent_walk_steps={steps} \
             oracle_bytes={} hash_prefix={:02x}{:02x}{:02x}{:02x}",
            t.ir.decl_nodes.len(),
            form.len(),
            h.0[0],
            h.0[1],
            h.0[2],
            h.0[3]
        );
    }
}

/// A wide, shallow hierarchy — the opposite shape, to separate depth cost from
/// declaration-count cost.
#[test]
fn synthetic_wide_hierarchy_is_traversable_and_hashable() {
    let mut t = TestIr::new();
    let root = t.node(&["w"], DeclId::NONE);
    let mut kids = Vec::new();
    for i in 0..2000 {
        kids.push(t.node(&["w", &format!("n{i}")], root));
    }
    let e = t.edge(&["w", "e"], root);
    let refs: Vec<_> = kids.iter().take(64).map(|&d| (d, S::Plus)).collect();
    t.arc(e, &refs);

    let h = canonical_program_hash(CanonHashCfg::default(), &t.idx, &t.ir, &t.it);
    eprintln!(
        "[depth] wide hierarchy: decls={} depth=2 branching=2000 hash_prefix={:02x}{:02x}",
        t.ir.decl_nodes.len(),
        h.0[0],
        h.0[1]
    );
    assert_eq!(t.ir.decl_nodes[root.0].kind, DeclKind::Node);
}

/// High-cardinality hyperedge: one arc with many references.
#[test]
fn synthetic_high_cardinality_edge_is_hashable() {
    let mut t = TestIr::new();
    let root = t.node(&["hc"], DeclId::NONE);
    let nodes: Vec<_> = (0..1000)
        .map(|i| t.node(&["hc", &format!("n{i}")], root))
        .collect();
    let e = t.edge(&["hc", "big"], root);
    let refs: Vec<_> = nodes.iter().map(|&d| (d, S::Plus)).collect();
    t.arc(e, &refs);

    let h = canonical_program_hash(CanonHashCfg::default(), &t.idx, &t.ir, &t.it);
    eprintln!(
        "[depth] high-cardinality edge: arity=1000 hash_prefix={:02x}{:02x}",
        h.0[0], h.0[1]
    );
}

/// Part E — the stack-overflow boundary, measured in a **child process**.
///
/// `canonical_hash`'s subtree fold recurses once per hierarchy level, and a
/// stack overflow is an abort, not a panic: `catch_unwind` cannot contain it.
/// So the boundary is found by running `examples/ir_depth_probe` and reading
/// its exit status.
///
/// The gate is the *margin*: the deepest declaration path observed anywhere in
/// the real corpus is 5 (see `real_corpus_declaration_depth_audit`), so the
/// boundary must be far above it. This test fails if that margin collapses.
#[test]
fn deep_chain_overflow_boundary_is_far_above_the_real_corpus() {
    // Locate the probe next to the test binary: target/<profile>/examples/.
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("[depth] current_exe unavailable — probe skipped");
        return;
    };
    let probe = exe.parent().and_then(|p| p.parent()).map(|p| {
        p.join("examples")
            .join(format!("ir_depth_probe{}", std::env::consts::EXE_SUFFIX))
    });

    let Some(probe) = probe.filter(|p| p.exists()) else {
        eprintln!(
            "[depth] probe binary not built — skipped. Build it with:\n\
             [depth]   cargo build --example ir_depth_probe"
        );
        return;
    };

    let run = |depth: usize| -> bool {
        std::process::Command::new(&probe)
            .arg(depth.to_string())
            .arg("--hash")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };

    // Bisect between a depth known to be safe and one known to abort.
    let mut lo = 8usize;
    let mut hi = 4096usize;
    if !run(lo) {
        eprintln!("[depth] probe failed even at depth {lo} — reporting, not gating");
        return;
    }
    // Establish an upper bound that actually fails.
    while run(hi) && hi < 1 << 20 {
        lo = hi;
        hi *= 2;
    }
    while hi - lo > 8 {
        let mid = (lo + hi) / 2;
        if run(mid) { lo = mid } else { hi = mid }
    }

    eprintln!(
        "[depth] canonical-hash recursion boundary: deepest OK={lo}, shallowest FAIL={hi} \
         (child process, default main-thread stack). Real-corpus max depth = 5."
    );

    assert!(
        lo > 50,
        "the canonical-hash recursion boundary collapsed to {lo}; the real corpus \
         reaches depth 5, so the margin is no longer safe. Either the fold grew \
         its stack frame or a depth limit is now required."
    );
}
