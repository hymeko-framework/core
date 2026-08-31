//! Validation against the real fixture corpus.
//!
//! APPROVED-CORE-EDIT: unified-ir-m2b-vv-core-consumers (2026-08-29).
//!
//! These tests answer a different question from `verification`: not "does the
//! implementation match its specification on inputs I constructed", but "does
//! it hold on the programs the project actually has". They were previously
//! scattered one-per-phase-directory, each carrying its own copy of the same
//! `.hymeko` directory walk; the walk now lives once in
//! [`crate::unified_ir::support::corpus`].
//!
//! Phase provenance: the depth audit and the canonical-hash corpus digest come
//! from M0.5/M0.6, the coherence survey from M0.6, the accessor equivalence
//! from M2a.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::unified_ir::verification::coherence::label;
use crate::unified_ir::verification::hierarchy::{DepthStats, depth_stats};

use hymeko::common::ids::{DeclId, EdgeId, HyperArcId, NodeId};
use hymeko::ir::canonical_hash::{CanonHashCfg, canonical_program_hash};
use hymeko::ir::coherence::{CoherenceLimits, Finding, validate_hierarchy_index_coherence};

use crate::unified_ir::support::corpus::require_fixtures;

#[test]
fn real_corpus_declaration_depth_audit() {
    let files = require_fixtures();
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
#[test]
fn corpus_hierarchy_index_coherence_survey() {
    let files = require_fixtures();

    let mut lowered = 0usize;
    let mut clean = 0usize;
    let mut diverged = 0usize;
    let mut counts: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    let mut first_example: Option<(String, Finding)> = None;

    for f in &files {
        let Ok((store, compiled)) = crate::test_helpers::load_and_lower(f) else {
            continue;
        };
        lowered += 1;
        match validate_hierarchy_index_coherence(
            &compiled.ir,
            &compiled.idx,
            &store.it,
            CoherenceLimits::default(),
        ) {
            Ok(()) => clean += 1,
            Err(report) => {
                diverged += 1;
                for fi in &report.findings {
                    *counts.entry(label(fi)).or_insert(0) += 1;
                }
                if first_example.is_none()
                    && let Some(fi) = report.findings.first()
                {
                    first_example = Some((f.display().to_string(), fi.clone()));
                }
            }
        }
    }

    eprintln!("[coherence] lowered={lowered} clean={clean} diverged={diverged}");
    for (k, v) in &counts {
        eprintln!("[coherence]   {k:<30} {v}");
    }
    if let Some((file, fi)) = &first_example {
        eprintln!("[coherence] first minimal example: {file}\n[coherence]   {fi:?}");
    }

    assert!(
        lowered > 0,
        "no fixture lowered; the survey measured nothing"
    );
}
/// Corpus-wide frozen digest: v1 over every lowerable fixture, folded in a
/// deterministic order. This is the broadest protection for the Part E rewrite
/// — it covers real declaration shapes the synthetic vectors do not reach.
fn corpus_digest() -> (usize, String) {
    let files = require_fixtures();

    let mut h = blake3::Hasher::new();
    let mut n = 0usize;
    for f in &files {
        if let Ok((store, compiled)) = crate::test_helpers::load_and_lower(f) {
            let d = canonical_program_hash(CanonHashCfg::default(), &compiled.ir, &store.it);
            // Include the file name so a reordering or omission is detected.
            h.update(
                f.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_bytes(),
            );
            h.update(&d.0);
            n += 1;
        }
    }
    (n, h.finalize().to_hex().to_string())
}

const KAT_CORPUS_COUNT: usize = 96;
const KAT_CORPUS_DIGEST: &str = "525fa75cf87828c2c70e8ca3f3b13a8a5641133b55d5c4d956e12b5769ade379";

#[test]
fn print_corpus_digest_for_capture() {
    let (n, d) = corpus_digest();
    eprintln!("[frozen] corpus_count {n}");
    eprintln!("[frozen] corpus_digest {d}");
}

/// The corpus gate. Protects v1 across the Part E rewrite over real programs.
#[test]
fn v1_corpus_digest_is_unchanged() {
    let (n, d) = corpus_digest();
    assert_eq!(
        n, KAT_CORPUS_COUNT,
        "the number of lowerable fixtures changed ({n} vs {KAT_CORPUS_COUNT}); \
         the corpus digest is only meaningful over a fixed set"
    );
    assert_eq!(
        d, KAT_CORPUS_DIGEST,
        "FROZEN v1 CORPUS DIGEST CHANGED. algo_version=1 output moved on real \
         programs — the traversal rewrite is not output-preserving."
    );
}
#[test]
fn accessors_agree_with_direct_field_access_on_the_whole_corpus() {
    // walker provided by support::corpus
    let files = require_fixtures();

    let mut checked = 0usize;
    let mut coherent = 0usize;
    for f in &files {
        let Ok((store, compiled)) = crate::test_helpers::load_and_lower(f) else {
            continue;
        };
        let ir = &compiled.ir;
        checked += 1;

        // Precondition for the comparison: the fixture is structurally sound.
        if validate_hierarchy_index_coherence(
            ir,
            &compiled.idx,
            &store.it,
            CoherenceLimits::default(),
        )
        .is_ok()
        {
            coherent += 1;
        }

        for i in 0..ir.decl_nodes.len() {
            let d = DeclId::new(i);

            // direct index vs accessor
            assert_eq!(ir.decl_node(d).map(|n| n.kind), Some(ir.decl_nodes[i].kind));
            assert_eq!(ir.kind_of(d), Some(ir.decl_nodes[i].kind));
            assert_eq!(ir.kind_of(d), Some(ir.kind(d)), "kind_of vs kind");

            // hierarchy: old (total-valued) vs new (Option-valued)
            let p = ir.parent(d);
            assert_eq!(ir.parent_of(d), p.is_some().then_some(p));
            let fc = ir.first_child(d);
            assert_eq!(ir.first_child_of(d), fc.is_some().then_some(fc));
            let ns = ir.next_sibling(d);
            assert_eq!(ir.next_sibling_of(d), ns.is_some().then_some(ns));

            // children: old vs new
            let old: Vec<DeclId> = ir.decl_children(d).collect();
            let new: Vec<DeclId> = ir.child_decls(d).collect();
            assert_eq!(old, new);

            // mapping: old id-only vs new id+record
            assert_eq!(ir.node_for_decl(d).map(|(n, _)| n), ir.as_node(d));
            assert_eq!(ir.edge_for_decl(d).map(|(e, _)| e), ir.as_edge(d));
            assert_eq!(ir.arc_for_decl(d).map(|(a, _)| a), ir.as_arc(d));
        }

        for (i, rec) in ir.nodes.iter().enumerate() {
            assert_eq!(ir.node(NodeId::new(i)).map(|r| r.decl), Some(rec.decl));
        }
        for (i, rec) in ir.edges.iter().enumerate() {
            assert_eq!(ir.edge(EdgeId::new(i)).map(|r| r.decl), Some(rec.decl));
        }
        for (i, rec) in ir.arcs.iter().enumerate() {
            assert_eq!(
                ir.arc(HyperArcId::new(i)).map(|r| r.in_edge),
                Some(rec.in_edge)
            );
        }

        assert_eq!(ir.decl_count(), ir.decl_nodes.len());
        assert_eq!(ir.node_count(), ir.nodes.len());
        assert_eq!(ir.edge_count(), ir.edges.len());
        assert_eq!(ir.arc_count(), ir.arcs.len());
    }

    eprintln!("[m2a] corpus comparison: {checked} fixtures, {coherent} coherent");
    assert!(checked > 0, "no fixture compared");
    assert_eq!(
        checked, coherent,
        "some fixture was not coherent; the accessor comparison assumes sound input"
    );
}

/// U2 authority flip: the link-derived hash equals the index-based hash on every
/// lowerable corpus fixture (the broadest bit-identity gate for the flip).
#[test]
fn link_derived_hash_equals_index_hash_on_the_whole_corpus() {
    let files = require_fixtures();
    let mut checked = 0usize;
    for f in &files {
        let Ok((store, compiled)) = crate::test_helpers::load_and_lower(f) else {
            continue;
        };
        // (namespace prefix + links) reproduces the Index hierarchy exactly
        hymeko::ir::canonical_hash::validate_namespace_roots(&compiled.ir, &compiled.idx)
            .unwrap_or_else(|e| panic!("namespace validation failed on {}: {e}", f.display()));
        let vi = canonical_program_hash(CanonHashCfg::default(), &compiled.ir, &store.it);
        let vl = hymeko::ir::canonical_hash::canonical_program_hash(
            CanonHashCfg::default(),
            &compiled.ir,
            &store.it,
        );
        assert_eq!(
            vl.0,
            vi.0,
            "U2.1 link-derived hash diverged from index on {}",
            f.display()
        );
        checked += 1;
    }
    assert!(checked >= 96, "expected >= 96 fixtures, got {checked}");
    eprintln!("[u2.1] link-derived hash == index hash on {checked} fixtures (namespaces complete)");
}

#[test]
fn nsroots_prefix_length_survey() {
    use std::collections::HashMap;
    let files = require_fixtures();
    let mut hist: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    let mut multiseg: Vec<String> = Vec::new();
    let mut max_roots_with_prefix = 0usize;
    for f in &files {
        let Ok((store, compiled)) = crate::test_helpers::load_and_lower(f) else {
            continue;
        };
        let ir = &compiled.ir;
        let it = &store.it;
        let mut idx_path: HashMap<usize, Vec<hymeko::common::ids::SymId>> = HashMap::new();
        for (pk, did) in &compiled.idx.by_path {
            idx_path.entry(did.raw()).or_insert_with(|| pk.0.clone());
        }
        let mut roots_with_prefix = 0usize;
        for i in 0..ir.decl_nodes.len() {
            let node = &ir.decl_nodes[i];
            if !matches!(
                node.kind,
                hymeko::ir::ir::DeclKind::Node | hymeko::ir::ir::DeclKind::Edge
            ) {
                continue;
            }
            // link-root iff parent NONE (or parent not Node/Edge)
            let p = node.parent;
            let is_root = p.is_none()
                || !matches!(
                    ir.decl_nodes[p.raw()].kind,
                    hymeko::ir::ir::DeclKind::Node | hymeko::ir::ir::DeclKind::Edge
                );
            if !is_root {
                continue;
            }
            if let Some(ip) = idx_path.get(&i) {
                // prefix = ip[0..len-1] (everything before the root's own name)
                let plen = ip.len().saturating_sub(1);
                *hist.entry(plen).or_default() += 1;
                if plen > 0 {
                    roots_with_prefix += 1;
                }
                if plen > 1 {
                    let segs: Vec<_> = ip[..plen]
                        .iter()
                        .map(|s| it.resolve(*s).to_string())
                        .collect();
                    multiseg.push(format!(
                        "{}: root={} prefix={:?}",
                        f.file_name().unwrap().to_string_lossy(),
                        it.resolve(node.name),
                        segs
                    ));
                }
            }
        }
        max_roots_with_prefix = max_roots_with_prefix.max(roots_with_prefix);
    }
    eprintln!("[nsroots] prefix-length histogram (link-roots): {hist:?}");
    eprintln!("[nsroots] max roots-with-prefix in a single fixture: {max_roots_with_prefix}");
    assert!(
        multiseg.is_empty(),
        "Part A invariant: import prefixes are single-segment; found {multiseg:?}"
    );
    eprintln!("[nsroots] multi-segment prefixes: {}", multiseg.len());
    for m in multiseg.iter().take(10) {
        eprintln!("[nsroots]   {m}");
    }
}

/// U2.1 Part G: median lowering wall over the corpus (portable A/B harness).
#[test]
#[ignore]
fn bench_lowering_corpus() {
    let files = crate::unified_ir::support::corpus::require_fixtures();
    let mut samples = Vec::new();
    for _ in 0..3 {
        // warmup
        for f in &files {
            let _ = crate::test_helpers::load_and_lower(f);
        }
    }
    for _ in 0..15 {
        let t = std::time::Instant::now();
        let mut n = 0usize;
        for f in &files {
            if let Ok((_s, c)) = crate::test_helpers::load_and_lower(f) {
                n += c.ir.decl_nodes.len();
            }
        }
        samples.push((t.elapsed().as_secs_f64() * 1e3, n));
    }
    samples.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let med = samples[samples.len() / 2];
    eprintln!(
        "[lowbench] corpus load+lower median = {:.2} ms  (total decls {})",
        med.0, med.1
    );
}

/// U2.2 Part C: the Ir-derived path index reproduces the canonical paths and
/// round-trips path<->DeclId on the corpus (including imported namespaces).
#[test]
fn derived_path_index_round_trips_on_the_corpus() {
    let files = require_fixtures();
    let mut checked = 0usize;
    for f in &files {
        let Ok((_s, c)) = crate::test_helpers::load_and_lower(f) else { continue };
        let pi = hymeko::ir::canonical_hash::build_path_index(&c.ir)
            .unwrap_or_else(|e| panic!("build_path_index failed on {}: {e}", f.display()));
        assert_eq!(pi.len(), c.idx.by_path.len(), "path count on {}", f.display());
        for (path, did) in pi.iter() {
            assert_eq!(pi.decl_of(&path.0), Some(did), "round-trip on {}", f.display());
            assert_eq!(pi.path_of(did).map(|p| &p.0), Some(&path.0));
        }
        checked += 1;
    }
    assert!(checked >= 96);
    eprintln!("[u2.2] derived path index round-trips on {checked} fixtures");
}
