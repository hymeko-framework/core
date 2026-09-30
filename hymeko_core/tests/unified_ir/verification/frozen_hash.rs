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
use hymeko::ir::canonical_hash::{Canon, CanonHashCfg};

use crate::unified_ir::support::builder::{S, TestIr};

fn hex(h: &hymeko::ir::hash::HashId) -> String {
    h.0.iter().map(|b| format!("{b:02x}")).collect()
}

fn v1(t: &TestIr) -> String {
    hex(&Canon::new(&t.ir, &t.it)
        .structure(CanonHashCfg::default())
        .0)
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

/// The same *program* as `vec_small_hierarchy`, but the declarations are inserted
/// in a different order, so the dense `DeclId`s differ. No fillers, no `Index`
/// edits: both IRs are fully coherent and hash equal iff dense-ID assignment is
/// irrelevant to v1 (U2.2 Part A â the honest dense-permutation oracle).
fn vec_dense_permuted() -> TestIr {
    let mut t = TestIr::new();
    let root = t.node(&["m"], DeclId::NONE); // m -> 0 (same)
    let e = t.edge(&["m", "e"], root); // e -> 1 (was 3)
    let y = t.node(&["m", "y"], root); // y -> 2 (same)
    let x = t.node(&["m", "x"], root); // x -> 3 (was 1)
    t.arc(e, &[(x, S::Plus), (y, S::Minus)]);
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

    // Captured before compacting Pruefer elimination state. The generated
    // forests exercise root/leaf transitions, degree > 2, dense-ID gaps from
    // anonymous arcs, and the hash configuration in the synthetic root label.
    let mut aggregate = blake3::Hasher::new();
    for shape in 0..4 {
        for count in [0, 1, 2, 3, 8, 31, 128] {
            let mut t = TestIr::new();
            let mut vertices = Vec::new();
            let mut paths: Vec<Vec<String>> = Vec::new();
            for i in 0..count {
                let parent = if i == 0 || (shape == 3 && i % 5 == 0) {
                    None
                } else {
                    Some(match shape {
                        0 => 0,
                        2 => (i - 1) / 3,
                        _ => i - 1,
                    })
                };
                let mut path = parent.map(|p| paths[p].clone()).unwrap_or_default();
                path.push(format!("v{i}"));
                let names: Vec<&str> = path.iter().map(String::as_str).collect();
                let parent_id = parent.map(|p| vertices[p]).unwrap_or(DeclId::NONE);
                let did = if i % 3 == 2 {
                    let edge = t.edge(&names, parent_id);
                    t.arc(edge, &[(edge, S::Plus)]);
                    edge
                } else {
                    t.node(&names, parent_id)
                };
                paths.push(path);
                vertices.push(did);
            }
            for cfg in [
                CanonHashCfg::default(),
                CanonHashCfg {
                    schema_version: 2,
                    algo_version: 7,
                    flags: 13,
                },
            ] {
                aggregate.update(&Canon::new(&t.ir, &t.it).structure(cfg).0.0);
            }
        }
    }
    assert_eq!(
        aggregate.finalize().to_hex().as_str(),
        "d8755d5b7caa5705e988a4c2fde385a71c1cb1343a4ef79b70c077e9d6075811",
        "Pruefer elimination changed the frozen forest/configuration corpus"
    );
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

// --- Authority-flip differential oracle (U2) --------------------------------
//
// The link-derived hash must equal the index-based hash on every well-formed
// fixture. `dense_permuted` is excluded here: it manufactures an index/link
// divergence (fillers removed from the index but left in `decl_nodes`), which is
// exactly what the flip resolves in favour of the links — it is redesigned as a
// genuine DeclId permutation in the authority tests.

fn v1_ir(t: &TestIr) -> String {
    hex(&Canon::new(&t.ir, &t.it)
        .structure(CanonHashCfg::default())
        .0)
}

#[test]
fn link_derived_hash_equals_index_hash_on_wellformed_vectors() {
    for (name, t) in [
        ("small_hierarchy", vec_small_hierarchy()),
        ("ordered_signs", vec_ordered_signs()),
        ("duplicate_incidences", vec_duplicate_incidences()),
        ("slot_permuted", vec_slot_permuted()),
    ] {
        assert_eq!(
            v1_ir(&t),
            v1(&t),
            "U2 authority flip diverged from the index-based hash for `{name}`"
        );
    }
}

#[test]
fn link_derived_hash_matches_frozen_kats_on_wellformed_vectors() {
    assert_eq!(v1_ir(&vec_small_hierarchy()), KAT_SMALL_HIERARCHY);
    assert_eq!(v1_ir(&vec_ordered_signs()), KAT_ORDERED_SIGNS);
    assert_eq!(v1_ir(&vec_duplicate_incidences()), KAT_DUPLICATE_INCIDENCES);
    assert_eq!(v1_ir(&vec_slot_permuted()), KAT_SLOT_PERMUTED);
}

/// Part A proof: the dense-permutation fixture really assigns different DeclIds to
/// the same program, both IRs are coherent, and v1 is unchanged.
#[test]
fn dense_permutation_uses_genuinely_different_declids() {
    let a = vec_small_hierarchy();
    let b = vec_dense_permuted();
    // find the edge `m.e` DeclId in each by its path
    let eid = |t: &TestIr| -> usize {
        for (pk, did) in &t.idx.by_path {
            if pk.0.len() == 2 && t.it.resolve(pk.0[1]) == "e" {
                return did.raw();
            }
        }
        panic!("no edge e");
    };
    assert_ne!(
        eid(&a),
        eid(&b),
        "the permutation must assign a different DeclId to `e`"
    );
    // same program => identical v1 (both index- and link-derived)
    assert_eq!(v1(&a), v1(&b), "dense permutation changed v1");
    assert_eq!(
        v1_ir(&a),
        v1_ir(&b),
        "dense permutation changed the link-derived v1"
    );
    assert_eq!(
        v1_ir(&b),
        v1(&b),
        "link and index hash disagree on the permutation"
    );
}
