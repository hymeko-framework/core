//! Part C — frozen known-answer vectors for `canonical_program_hash` v1.
//!
//! APPROVED-CORE-EDIT: unified-ir-m0-6-hash-and-traversal (2026-08-28).
//!
//! These were captured from the **recursive** implementation before the Part E
//! rewrite. They are the contract that protects v1 across that rewrite: if the
//! iterative traversal changes any of them, it changed `algo_version = 1`
//! output, which this phase forbids.
//!
//! They also serve as the reference the brief allows in place of keeping a
//! second production implementation — capturing frozen outputs rather than
//! shipping both a recursive and an iterative code path.
//!
//! To re-capture (only ever legitimate when `algo_version` is deliberately
//! bumped):
//! ```text
//! cargo test -p hymeko_core --test mod unified_ir_m06::frozen_v1 -- --nocapture
//! ```
//! and read the `[frozen]` lines.

use hymeko::common::ids::DeclId;
use hymeko::ir::canonical_hash::{CanonHashCfg, canonical_program_hash};

use crate::unified_ir_m05::builder::{S, TestIr};

fn hex(h: &hymeko::ir::hash::HashId) -> String {
    h.0.iter().map(|b| format!("{b:02x}")).collect()
}

fn v1(t: &TestIr) -> String {
    hex(&canonical_program_hash(
        CanonHashCfg::default(),
        &t.idx,
        &t.ir,
        &t.it,
    ))
}

// --- the five vectors -------------------------------------------------------

/// Small hierarchy: root with two child nodes and one edge.
fn vec_small_hierarchy() -> TestIr {
    let mut t = TestIr::new();
    let root = t.node(&["m"], DeclId::NONE);
    let x = t.node(&["m", "x"], root);
    let y = t.node(&["m", "y"], root);
    let e = t.edge(&["m", "e"], root);
    t.arc(e, &[(x, S::Plus), (y, S::Minus)]);
    t
}

/// Ordered signed incidences: three references, one of each sign.
fn vec_ordered_signs() -> TestIr {
    let mut t = TestIr::new();
    let root = t.node(&["s"], DeclId::NONE);
    let a = t.node(&["s", "a"], root);
    let b = t.node(&["s", "b"], root);
    let c = t.node(&["s", "c"], root);
    let e = t.edge(&["s", "e"], root);
    t.arc(e, &[(a, S::Plus), (b, S::Minus), (c, S::Neutral)]);
    t
}

/// Duplicate incidences (U3): the same signed reference twice.
fn vec_duplicate_incidences() -> TestIr {
    let mut t = TestIr::new();
    let root = t.node(&["d"], DeclId::NONE);
    let a = t.node(&["d", "a"], root);
    let e = t.edge(&["d", "e"], root);
    t.arc(e, &[(a, S::Plus), (a, S::Plus)]);
    t
}

/// Same references as `vec_ordered_signs`, different slot order (U1).
fn vec_slot_permuted() -> TestIr {
    let mut t = TestIr::new();
    let root = t.node(&["s"], DeclId::NONE);
    let a = t.node(&["s", "a"], root);
    let b = t.node(&["s", "b"], root);
    let c = t.node(&["s", "c"], root);
    let e = t.edge(&["s", "e"], root);
    t.arc(e, &[(c, S::Neutral), (b, S::Minus), (a, S::Plus)]);
    t
}

/// `vec_small_hierarchy` with every dense `DeclId` shifted by leading fillers,
/// which are then removed from the index so the *program* is identical.
fn vec_dense_permuted() -> TestIr {
    let mut t = TestIr::new();
    let mut doomed = Vec::new();
    for i in 0..5 {
        let name = format!("f{i}");
        t.node(&["filler", &name], DeclId::NONE);
        doomed.push(name);
    }
    let root = t.node(&["m"], DeclId::NONE);
    let x = t.node(&["m", "x"], root);
    let y = t.node(&["m", "y"], root);
    let e = t.edge(&["m", "e"], root);
    t.arc(e, &[(x, S::Plus), (y, S::Minus)]);
    let keys: Vec<_> = t
        .idx
        .by_path
        .keys()
        .filter(|pk| pk.0.len() == 2 && t.it.resolve(pk.0[0]) == "filler")
        .cloned()
        .collect();
    for k in keys {
        t.idx.by_path.remove(&k);
    }
    t
}

// --- frozen expectations ----------------------------------------------------
//
// Captured 2026-08-28 from the recursive implementation, before the Part E
// rewrite, with CanonHashCfg { schema_version: 1, algo_version: 1, flags: 0 }.

const KAT_SMALL_HIERARCHY: &str =
    "ba63dbfd2163411b5cdebcad0890647894c24edc2cd11c51753b80313c848e96";
const KAT_ORDERED_SIGNS: &str = "8d80483ae645c0cd9f123daac5316999abb30aa5fe7f63eb09cb795e0e14aa54";
const KAT_DUPLICATE_INCIDENCES: &str =
    "5c7dfc300f857aa0dc715ed4df55601633de64ac88855000e4b5db414d58cb01";
const KAT_SLOT_PERMUTED: &str = "7984e99f89ca6551d0f85d905dde4edde0094f14cfb36329f42abe9d55ba9924";
const KAT_DENSE_PERMUTED: &str = "ba63dbfd2163411b5cdebcad0890647894c24edc2cd11c51753b80313c848e96";

/// Print the current values so they can be frozen. Always passes.
#[test]
fn print_frozen_vectors_for_capture() {
    for (name, t) in [
        ("small_hierarchy", vec_small_hierarchy()),
        ("ordered_signs", vec_ordered_signs()),
        ("duplicate_incidences", vec_duplicate_incidences()),
        ("slot_permuted", vec_slot_permuted()),
        ("dense_permuted", vec_dense_permuted()),
    ] {
        eprintln!("[frozen] {name:<22} {}", v1(&t));
    }
}

/// The gate. Any change here is a change to `algo_version = 1` output.
#[test]
fn v1_known_answer_vectors_are_unchanged() {
    let cases = [
        (
            "small_hierarchy",
            vec_small_hierarchy(),
            KAT_SMALL_HIERARCHY,
        ),
        ("ordered_signs", vec_ordered_signs(), KAT_ORDERED_SIGNS),
        (
            "duplicate_incidences",
            vec_duplicate_incidences(),
            KAT_DUPLICATE_INCIDENCES,
        ),
        ("slot_permuted", vec_slot_permuted(), KAT_SLOT_PERMUTED),
        ("dense_permuted", vec_dense_permuted(), KAT_DENSE_PERMUTED),
    ];
    for (name, t, expected) in cases {
        assert_eq!(
            v1(&t),
            expected,
            "FROZEN v1 VECTOR CHANGED for `{name}`. This is a change to \
             CanonHashCfg algo_version=1 output, which M0.6 forbids. Either the \
             traversal rewrite is not output-preserving, or algo_version must \
             be bumped deliberately."
        );
    }
}

/// U1 and U3 as hash-level contracts, independent of the frozen bytes.
#[test]
fn slot_order_and_multiplicity_change_v1() {
    assert_ne!(
        v1(&vec_ordered_signs()),
        v1(&vec_slot_permuted()),
        "U1: reference slot order is semantic; v1 must distinguish these"
    );
    let mut single = TestIr::new();
    let root = single.node(&["d"], DeclId::NONE);
    let a = single.node(&["d", "a"], root);
    let e = single.edge(&["d", "e"], root);
    single.arc(e, &[(a, S::Plus)]);
    assert_ne!(
        v1(&single),
        v1(&vec_duplicate_incidences()),
        "U3: multiplicity is semantic; v1 must distinguish one from two"
    );
}

/// Dense-ID permutation must NOT change v1 (M0.5 C5, frozen here as a vector).
#[test]
fn dense_permutation_does_not_change_v1() {
    assert_eq!(
        v1(&vec_small_hierarchy()),
        v1(&vec_dense_permuted()),
        "dense-ID assignment must not affect v1"
    );
}

/// Corpus-wide frozen digest: v1 over every lowerable fixture, folded in a
/// deterministic order. This is the broadest protection for the Part E rewrite
/// — it covers real declaration shapes the synthetic vectors do not reach.
fn corpus_digest() -> (usize, String) {
    use std::path::{Path, PathBuf};
    let root = Path::new("../data");
    let mut files: Vec<PathBuf> = Vec::new();
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
                files.push(p);
            }
        }
    }
    files.sort();

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
