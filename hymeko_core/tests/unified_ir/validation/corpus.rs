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

use crate::unified_ir::support::corpus::fixture_files;

#[test]
fn real_corpus_declaration_depth_audit() {
    // corpus root is resolved by the shared walker
    if fixture_files().is_empty() {
        eprintln!("[depth] ../data not present — corpus audit skipped");
        return;
    }
    let files = fixture_files();
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
    let files = fixture_files();
    if files.is_empty() {
        eprintln!("[coherence] no fixtures found — survey skipped");
        return;
    }

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
    // walker provided by support::corpus
    // corpus root is resolved by the shared walker
    let files = fixture_files();

    let mut h = blake3::Hasher::new();
    let mut n = 0usize;
    for f in &files {
        if let Ok((store, compiled)) = crate::test_helpers::load_and_lower(f) {
            let d = canonical_program_hash(
                CanonHashCfg::default(),
                &compiled.idx,
                &compiled.ir,
                &store.it,
            );
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
    if !std::path::Path::new("../data").exists() {
        eprintln!("[frozen] ../data absent — corpus gate skipped");
        return;
    }
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
    // corpus root is resolved by the shared walker
    if fixture_files().is_empty() {
        eprintln!("[m2a] ../data absent — corpus comparison skipped");
        return;
    }
    let files = fixture_files();

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
