//! P2.1 V&V for the canonical incidence backbone (APPROVED-CORE-EDIT:
//! canonical-incidence-backbone). Proves the owned [`PreparedIr`] bundle + borrowed
//! [`IncidenceView`] / [`StarOperator`] are an exact, byte-identical replacement for
//! the borrowed [`IncidenceIndex`] path, that the derived index is *rebuilt* from the
//! canonical `Ir` (never carried in the wire), and that view creation is `O(1)` /
//! zero-allocation. Wired into `tests/mod.rs` (autotests=false).
//!
//! Coverage map (mission §7): old-vs-new API differential (B / Bᵀ / BᵀB, resolve,
//! incident_arcs/slots, degree) · owned-bundle lifecycle (new / ir / index / into_ir)
//! · star-operator handle parity · prepared-plan parity via the view · index-rebuild
//! parity from a cloned `Ir` (deserialize stand-in) · derived-index-not-in-wire
//! (structural) · zero-alloc view creation (structural: `Copy`, references-only) ·
//! wrong-owner fault mutant (bijection rejects a foreign `Ir`) · 0/1/2/63/64/65 and
//! large boundary sizes.

use std::mem::{size_of, size_of_val};

use hymeko::common::ids::DeclId;
use hymeko::ir::incidence::{
    IncidenceIndex, IncidenceIndexData, IncidenceView, PreparedIr, StarOperator,
    build_incidence_index,
};
use hymeko::ir::ir::Ir;

use super::incidence_support::{arc, bits_eq, median_ms, node, r, windowed};

// ---- workloads ------------------------------------------------------------

/// A mixed workload: 4 nodes, 3 arcs mixing +/−/~ signs, none/single/multi weights,
/// a duplicate incidence and a `-0.0` weight — the same shape the P2 parity test uses.
fn mixed() -> Ir {
    let mut ir = Ir::default();
    for _ in 0..4 {
        node(&mut ir);
    }
    arc(
        &mut ir,
        vec![
            r(0, 1, None),
            r(1, -1, Some(vec![2.0])),
            r(0, 1, Some(vec![3.0])),
        ],
    );
    arc(
        &mut ir,
        vec![r(1, 0, Some(vec![5.0])), r(2, 1, Some(vec![2.0, 0.5]))],
    );
    arc(
        &mut ir,
        vec![
            r(3, -1, Some(vec![-0.0])),
            r(2, 1, None),
            r(0, -1, Some(vec![4.0])),
        ],
    );
    ir
}

/// Deterministic pseudo-random dense vectors for the operator inputs.
fn xvec(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 1.0 + (i % 13) as f64 * 0.1 - (i % 5) as f64 * 0.07)
        .collect()
}
fn yvec(n_arcs: usize) -> Vec<f64> {
    (0..n_arcs).map(|i| 0.5 + (i % 11) as f64 * 0.13).collect()
}

/// The heart of the differential: every observable of the borrowed `IncidenceIndex`
/// (old) must equal the owned `PreparedIr` view (new) for the *same* `Ir` — coordinate
/// resolution, adjacency, degree and all three operators, bit-for-bit.
fn assert_view_matches_index(ir: Ir) {
    let n = ir.decl_nodes.len();
    let n_arcs = ir.arcs.len();

    let index: IncidenceIndex<'_> = build_incidence_index(&ir).expect("index builds");
    index.validate().expect("index is a bijection");

    let prepared = PreparedIr::new(ir.clone()).expect("prepared builds");
    let view: IncidenceView<'_, '_> = prepared.incidences();
    view.validate().expect("view is a bijection");

    let (data, arcs, slots, incidences, plan) = {
        let short_lived = prepared.incidences();
        (
            short_lived.data(),
            short_lived.incident_arcs(DeclId::new(0)),
            short_lived.incident_slots(DeclId::new(0)),
            short_lived.incidences(DeclId::new(0)),
            short_lived
                .prepare_operator()
                .expect("short-lived view plan"),
        )
    };
    assert!(std::ptr::eq(data, prepared.index()));
    assert_eq!(arcs, index.incident_arcs(DeclId::new(0)));
    assert_eq!(slots, index.incident_slots(DeclId::new(0)));
    assert_eq!(incidences.count(), index.degree(DeclId::new(0)));
    assert_eq!(
        plan.retained_bytes(),
        index.prepare_operator().unwrap().retained_bytes()
    );

    // structural / accounting invariants
    assert_eq!(view.incidence_count(), index.incidence_count());
    assert_eq!(view.retained_bytes(), index.retained_bytes());
    assert_eq!(prepared.index().incidence_count(), index.incidence_count());

    // per-node adjacency + resolution
    for d in 0..n {
        let node = DeclId::new(d);
        assert_eq!(
            view.incident_arcs(node),
            index.incident_arcs(node),
            "arcs@{d}"
        );
        assert_eq!(
            view.incident_slots(node),
            index.incident_slots(node),
            "slots@{d}"
        );
        assert_eq!(view.degree(node), index.degree(node), "deg@{d}");

        let a: Vec<_> = index
            .incidences(node)
            .map(|i| (i.arc, i.slot, i.target, i.sign, i.edge))
            .collect();
        let b: Vec<_> = view
            .incidences(node)
            .map(|i| (i.arc, i.slot, i.target, i.sign, i.edge))
            .collect();
        assert_eq!(a, b, "resolved incidences@{d}");
        // resolve() coordinate-for-coordinate (incl. weights)
        for (&arc_id, &slot) in index
            .incident_arcs(node)
            .iter()
            .zip(index.incident_slots(node))
        {
            let ri = index.resolve(arc_id, slot);
            let rv = view.resolve(arc_id, slot);
            assert_eq!(
                (ri.arc, ri.slot, ri.target, ri.sign, ri.edge),
                (rv.arc, rv.slot, rv.target, rv.sign, rv.edge)
            );
            assert_eq!(ri.weights, rv.weights, "weights@({},{slot})", arc_id.raw());
        }
    }

    // operators B / Bᵀ / BᵀB — bit-identical
    let x = xvec(n);
    let y = yvec(n_arcs);

    let (mut yi, mut yv) = (vec![0.0; n_arcs], vec![0.0; n_arcs]);
    index.node_to_arc(&x, &mut yi);
    view.node_to_arc(&x, &mut yv);
    assert!(bits_eq(&yi, &yv), "Bx (node_to_arc) differs");

    let (mut zi, mut zv) = (vec![0.0; n], vec![0.0; n]);
    index.arc_to_node(&y, &mut zi);
    view.arc_to_node(&y, &mut zv);
    assert!(bits_eq(&zi, &zv), "Bᵀy (arc_to_node) differs");

    let (mut si, mut sv) = (vec![0.0; n], vec![0.0; n]);
    let (mut sci, mut scv) = (vec![0.0; n_arcs], vec![0.0; n_arcs]);
    index.star_operator(&x, &mut si, &mut sci);
    view.star_operator(&x, &mut sv, &mut scv);
    assert!(bits_eq(&si, &sv), "BᵀBx (star_operator) differs");

    // star-operator handle == the view's own star_operator
    let star: StarOperator<'_, '_> = prepared.star_operator();
    let mut sh = vec![0.0; n];
    let mut shc = vec![0.0; n_arcs];
    star.apply(&x, &mut sh, &mut shc);
    assert!(
        bits_eq(&si, &sh),
        "StarOperator::apply differs from star_operator"
    );
    let mut yh = vec![0.0; n_arcs];
    star.node_to_arc(&x, &mut yh);
    assert!(bits_eq(&yi, &yh), "StarOperator::node_to_arc differs");
    let mut zh = vec![0.0; n];
    star.arc_to_node(&y, &mut zh);
    assert!(bits_eq(&zi, &zh), "StarOperator::arc_to_node differs");

    // prepared plan via the view == prepared plan via the index (both bit-identical to legacy Bᵀ)
    let plan_i = index.prepare_operator().expect("index plan");
    let plan_v = view.prepare_operator().expect("view plan");
    assert_eq!(plan_i.retained_bytes(), plan_v.retained_bytes());
    let (mut pi, mut pv) = (vec![0.0; n], vec![0.0; n]);
    plan_i.arc_to_node(&y, &mut pi);
    plan_v.arc_to_node(&y, &mut pv);
    assert!(bits_eq(&pi, &pv), "prepared Bᵀy view vs index differs");
    assert!(bits_eq(&zi, &pv), "prepared Bᵀy differs from legacy Bᵀy");
    let mut par = vec![0.0; n];
    plan_v.par_arc_to_node(&y, &mut par);
    assert!(bits_eq(&pv, &par), "prepared serial vs Rayon differs");
}

// ---- tests ----------------------------------------------------------------

#[test]
fn prepared_view_matches_index_bitwise_mixed() {
    assert_view_matches_index(mixed());
}

#[test]
fn prepared_view_matches_index_across_boundary_sizes() {
    // Empty, single, two, and the 63/64/65 CSR/word boundaries, plus a wider window.
    assert_view_matches_index(Ir::default());
    assert_view_matches_index(windowed(1, 1));
    assert_view_matches_index(windowed(2, 1));
    assert_view_matches_index(windowed(2, 2));
    for m in [63usize, 64, 65] {
        assert_view_matches_index(windowed(m, 1));
        assert_view_matches_index(windowed(m, 3));
    }
}

#[test]
fn prepared_view_matches_index_large() {
    assert_view_matches_index(windowed(50_000, 5));
}

#[test]
fn prepared_ir_lifecycle_new_ir_index_into_ir() {
    let ir = mixed();
    let arcs_before = ir.arcs.len();
    let prepared = PreparedIr::new(ir).expect("prepared builds");

    // ir() borrows the owned Ir; index() the derived payload; both consistent.
    assert_eq!(prepared.ir().arcs.len(), arcs_before);
    let nnz: usize = prepared.ir().arcs.iter().map(|a| a.refs.len()).sum();
    assert_eq!(prepared.index().incidence_count(), nnz);
    prepared.incidences().validate().expect("bijection");

    // into_ir returns the owned Ir unchanged (the derived index is dropped, not serialized).
    let back = prepared.into_ir();
    assert_eq!(back.arcs.len(), arcs_before);
}

#[test]
fn index_rebuilt_from_cloned_ir_is_bit_identical() {
    // A clone stands in for serialize → deserialize: the wire carries only the
    // canonical `Ir`; the derived index is rebuilt on load (never carried). The
    // rebuilt operators must be bit-identical to the original.
    let ir = mixed();
    let n = ir.decl_nodes.len();
    let n_arcs = ir.arcs.len();
    let index = build_incidence_index(&ir).unwrap();

    let reloaded = PreparedIr::new(ir.clone()).unwrap();
    let view = reloaded.incidences();

    let y = yvec(n_arcs);
    let (mut z0, mut z1) = (vec![0.0; n], vec![0.0; n]);
    index.arc_to_node(&y, &mut z0);
    view.arc_to_node(&y, &mut z1);
    assert!(bits_eq(&z0, &z1), "rebuilt Bᵀy differs from original");

    // The data alone rebuilds identically too (data-level constructor).
    let data = IncidenceIndexData::build(reloaded.ir()).unwrap();
    let (mut z2, mut z3) = (vec![0.0; n], vec![0.0; n]);
    data.arc_to_node(reloaded.ir(), &y, &mut z2);
    reloaded.index().arc_to_node(reloaded.ir(), &y, &mut z3);
    assert!(
        bits_eq(&z0, &z2) && bits_eq(&z0, &z3),
        "data-level rebuild differs"
    );
}

#[test]
fn view_and_star_creation_is_zero_alloc_structural() {
    // `IncidenceView` / `StarOperator` are `Copy` and hold only references — creating
    // one is a register move of two (resp. two) pointers, provably heap-free. The
    // owned index memory lives in the `PreparedIr`; the view borrows it, copies nothing.
    let prepared = PreparedIr::new(mixed()).unwrap();

    let view = prepared.incidences();
    assert_eq!(
        size_of_val(&view),
        2 * size_of::<usize>(),
        "view is two thin references"
    );
    let star = prepared.star_operator();
    assert_eq!(
        size_of_val(&star),
        2 * size_of::<usize>(),
        "star wraps the two-ref view"
    );

    // Compile-time proof of `Copy` (fails to compile if either loses `Copy`).
    fn assert_copy<T: Copy>(_: &T) {}
    assert_copy(&view);
    assert_copy(&star);

    // Handing out a million views allocates nothing structurally (references only).
    let mut acc = 0usize;
    for _ in 0..1_000_000 {
        let v: IncidenceView<'_, '_> = prepared.incidences();
        acc = acc.wrapping_add(v.incidence_count());
    }
    assert_eq!(
        acc,
        prepared.index().incidence_count().wrapping_mul(1_000_000)
    );
}

#[test]
fn wrong_owner_index_is_rejected_by_bijection() {
    // Fault mutant: an `IncidenceIndexData` built from one `Ir` validated against a
    // *different* `Ir` must fail the bijection — the runtime guard for the raw-data
    // path (the `IncidenceIndex` / `PreparedIr` wrappers make the wrong pairing a
    // compile error by bundling the `Ir` with its data).
    let ir_a = windowed(8, 3);
    let ir_b = windowed(5, 2); // different node/arc counts
    let data = IncidenceIndexData::build(&ir_a).unwrap();
    assert!(data.validate(&ir_a).is_ok(), "matches its own Ir");
    assert!(data.validate(&ir_b).is_err(), "must reject a foreign Ir");
}

#[test]
#[ignore = "performance benchmark; opt in with --ignored --exact"]
fn bench_representation_view_vs_index_and_o1_creation() {
    // §8 decision gate. Representations compared on the SAME data + math:
    //   A = borrowed `IncidenceIndex` (row/arcs/slots SoA borrowing the `Ir`);
    //   B = owned `PreparedIr` + borrowed `IncidenceView` (the chosen model).
    // A and B share the operator code (the logic lives on `IncidenceIndexData`), so B
    // must be a no-regression drop-in AND its view creation must be O(1) / zero-alloc.
    // (The flat arc-major SoA "C" — a second (arc,slot)/coefficient array — was measured
    // in P2's bx_profile: ~2× Bx but +8 B/incidence with no consumer, so it is deferred,
    // not forced; see the report.)
    let (m, k) = (200_001usize, 5usize);
    let ir = windowed(m, k);
    let n = ir.decl_nodes.len();
    let n_arcs = ir.arcs.len();
    let nnz: usize = ir.arcs.iter().map(|a| a.refs.len()).sum();

    let index = build_incidence_index(&ir).unwrap();
    let prepared = PreparedIr::new(ir.clone()).unwrap();
    let view = prepared.incidences();

    // retained memory is identical (B owns the same SoA arrays A borrows).
    assert_eq!(view.retained_bytes(), index.retained_bytes());

    let x = xvec(n);
    let y = yvec(n_arcs);
    let (mut za, mut zb) = (vec![0.0; n], vec![0.0; n]);

    // O(1) view creation: median over 1e6 creations, black-boxed against elision.
    let create = median_ms(9, || {
        let mut acc = 0usize;
        for _ in 0..1_000_000 {
            acc = acc.wrapping_add(prepared.incidences().incidence_count());
        }
        assert_eq!(acc, nnz.wrapping_mul(1_000_000));
    });
    let per_create_ns = create * 1e6 / 1_000_000.0;

    let bt_a = median_ms(9, || index.arc_to_node(&y, &mut za));
    let bt_b = median_ms(9, || view.arc_to_node(&y, &mut zb));
    assert!(bits_eq(&za, &zb), "Bᵀy A vs B must be bit-identical");

    let (mut sa, mut sb) = (vec![0.0; n], vec![0.0; n]);
    let mut scr = vec![0.0; n_arcs];
    let star = prepared.star_operator();
    let st_a = median_ms(9, || index.star_operator(&x, &mut sa, &mut scr));
    let st_b = median_ms(9, || star.apply(&x, &mut sb, &mut scr));
    assert!(bits_eq(&sa, &sb), "BᵀBx A vs B must be bit-identical");

    println!("\n=== P2.1 §8 representation gate — {nnz} incidences ===");
    println!(
        "retained bytes A==B      : {} B ({:.1} B/incidence)",
        view.retained_bytes(),
        view.retained_bytes() as f64 / nnz as f64
    );
    println!("view creation (O(1))     : {per_create_ns:.2} ns/create");
    println!("Bᵀy   A (index)          : {bt_a:.3} ms");
    println!(
        "Bᵀy   B (view)           : {bt_b:.3} ms   ({:.2}x)",
        bt_a / bt_b.max(1e-9)
    );
    println!("BᵀBx  A (index)          : {st_a:.3} ms");
    println!(
        "BᵀBx  B (star handle)    : {st_b:.3} ms   ({:.2}x)",
        st_a / st_b.max(1e-9)
    );

    // O(1) guard: a 16-byte reference-pair copy is sub-100 ns even on a loaded host.
    assert!(
        per_create_ns < 100.0,
        "view creation must be O(1) (was {per_create_ns:.1} ns)"
    );
    // No-regression guard: same code path, so B must stay within a generous band of A.
    assert!(bt_b <= bt_a * 1.5 + 0.5, "view Bᵀy regressed vs index");
    assert!(st_b <= st_a * 1.5 + 0.5, "star BᵀBx regressed vs index");
}
