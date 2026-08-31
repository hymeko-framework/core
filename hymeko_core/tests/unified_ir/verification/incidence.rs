//! Reverse-incidence navigation + sparse signed star operator V&V
//! (APPROVED-CORE-EDIT: canonical-ir-incidence-navigation, corrective pass).
//!
//! Golden shapes, an INDEPENDENT O(V*E) exact-coordinate scan oracle, an
//! INDEPENDENT dense B matrix-vector oracle for Bx / Bᵀy / BᵀBx, weight-policy
//! cases, and a dangling-target fault. Everything goes through the real Ir
//! (`TestIr` builder → canonical incidences). The validator's bijection
//! fault-mutants live in the lib unit tests (they corrupt the index's private
//! coordinate arrays).

use hymeko::common::ids::{DeclId, HyperArcId};
use hymeko::ir::incidence::{IncidenceError, build_incidence_index, scalar_weight};
use hymeko::ir::ir::{Ir, SignedRefR};

use crate::unified_ir::support::builder::{S, TestIr};

/// Independent O(V*E) exact-coordinate oracle: scan every arc/ref in order; emit
/// `(arc, slot)` for each ref targeting `node`. No index involved.
fn oracle_coords(ir: &Ir, node: DeclId) -> Vec<(HyperArcId, u32)> {
    let mut out = Vec::new();
    for (ai, arc) in ir.arcs.iter().enumerate() {
        for (slot, sref) in arc.refs.iter().enumerate() {
            if sref.target() == node {
                out.push((HyperArcId::new(ai), slot as u32));
            }
        }
    }
    out
}

fn assert_coords_match_oracle(t: &TestIr) {
    let idx = build_incidence_index(&t.ir).expect("well-formed index builds");
    idx.validate().expect("index is a coordinate bijection");
    for d in 0..t.ir.decl_nodes.len() {
        let did = DeclId::new(d);
        let got: Vec<(HyperArcId, u32)> = idx
            .incident_arcs(did)
            .iter()
            .copied()
            .zip(idx.incident_slots(did).iter().copied())
            .collect();
        assert_eq!(
            got,
            oracle_coords(&t.ir, did),
            "coords disagree at decl {d}"
        );
    }
}

// --- golden shapes ----------------------------------------------------------

#[test]
fn multiway_hyperedge_all_three_signs_resolves_coordinates() {
    let mut t = TestIr::new();
    let a = t.node(&["m", "a"], DeclId::NONE);
    let b = t.node(&["m", "b"], DeclId::NONE);
    let c = t.node(&["m", "c"], DeclId::NONE);
    let e = t.edge(&["m", "e"], DeclId::NONE);
    let arc = t.arc(e, &[(a, S::Plus), (b, S::Minus), (c, S::Neutral)]);
    let idx = build_incidence_index(&t.ir).unwrap();
    let ra = idx.incidences(a).next().unwrap();
    assert_eq!((ra.arc, ra.slot, ra.target, ra.sign), (arc, 0, a, 1));
    let rb = idx.incidences(b).next().unwrap();
    assert_eq!((rb.slot, rb.sign), (1, -1));
    let rc = idx.incidences(c).next().unwrap();
    assert_eq!((rc.slot, rc.sign), (2, 0));
    assert_coords_match_oracle(&t);
}

#[test]
fn duplicate_incidence_kept_as_distinct_coordinates() {
    let mut t = TestIr::new();
    let a = t.node(&["d", "a"], DeclId::NONE);
    let e = t.edge(&["d", "e"], DeclId::NONE);
    let arc = t.arc(e, &[(a, S::Plus), (a, S::Plus)]);
    let idx = build_incidence_index(&t.ir).unwrap();
    let coords: Vec<_> = idx
        .incident_arcs(a)
        .iter()
        .copied()
        .zip(idx.incident_slots(a).iter().copied())
        .collect();
    assert_eq!(
        coords,
        vec![(arc, 0), (arc, 1)],
        "distinct slots for duplicate incidence"
    );
    assert_coords_match_oracle(&t);
}

#[test]
fn ragged_arity_shared_and_singleton() {
    let mut t = TestIr::new();
    let hub = t.node(&["g", "hub"], DeclId::NONE);
    let x = t.node(&["g", "x"], DeclId::NONE);
    let solo = t.node(&["g", "solo"], DeclId::NONE);
    let e = t.edge(&["g", "e"], DeclId::NONE);
    t.arc(e, &[(hub, S::Plus), (x, S::Minus)]);
    t.arc(e, &[(hub, S::Neutral)]);
    let idx = build_incidence_index(&t.ir).unwrap();
    assert_eq!(idx.degree(hub), 2);
    assert_eq!(idx.degree(solo), 0);
    assert_coords_match_oracle(&t);
}

#[test]
fn empty_is_total() {
    let empty = Ir::default();
    let idx = build_incidence_index(&empty).unwrap();
    assert_eq!(idx.incidence_count(), 0);
    assert!(idx.validate().is_ok());
    assert_eq!(idx.incident_arcs(DeclId::new(0)), &[] as &[HyperArcId]);
}

// --- weight policy ----------------------------------------------------------

#[test]
fn weight_policy_none_single_and_multivalued() {
    use hymeko::ir::ir::{RefAtomR, ValueR};
    let mk = |w: Option<Vec<ValueR>>| {
        SignedRefR::Plus(RefAtomR {
            target: DeclId::new(0),
            anno: hymeko::ir::ir::AnnoR::default(),
            weights: w,
        })
    };
    assert_eq!(scalar_weight(&mk(None)), 1.0);
    assert_eq!(scalar_weight(&mk(Some(vec![ValueR::Num(2.5)]))), 2.5);
    assert_eq!(
        scalar_weight(&mk(Some(vec![ValueR::Num(2.0), ValueR::Num(3.0)]))),
        6.0,
        "product policy"
    );
    assert_eq!(
        scalar_weight(&mk(Some(vec![ValueR::Ref(DeclId::new(1))]))),
        1.0,
        "all-non-numeric -> 1.0"
    );
}

// --- sparse star operator vs an INDEPENDENT dense B matvec ------------------

/// Dense signed incidence matrix `B[arc][node] = Σ_slot sign·weight` (multiplicity
/// summed) — the independent oracle for the sparse operator.
fn dense_b(t: &TestIr) -> Vec<Vec<f64>> {
    let n = t.ir.decl_nodes.len();
    let mut b = vec![vec![0.0f64; n]; t.ir.arcs.len()];
    for (ai, arc) in t.ir.arcs.iter().enumerate() {
        for sref in &arc.refs {
            b[ai][sref.target().raw()] += f64::from(sref.sign()) * scalar_weight(sref);
        }
    }
    b
}

#[test]
fn star_operator_matches_dense_b_matvec() {
    let mut t = TestIr::new();
    let v: Vec<DeclId> = (0..5)
        .map(|i| {
            t.node(
                &["s", Box::leak(format!("n{i}").into_boxed_str())],
                DeclId::NONE,
            )
        })
        .collect();
    let e = t.edge(&["s", "e"], DeclId::NONE);
    t.arc(e, &[(v[0], S::Plus), (v[1], S::Minus), (v[2], S::Plus)]);
    t.arc(e, &[(v[2], S::Minus), (v[3], S::Plus)]);
    t.arc(e, &[(v[0], S::Plus), (v[0], S::Plus)]); // duplicate incidence
    let idx = build_incidence_index(&t.ir).unwrap();
    let b = dense_b(&t);
    let n = t.ir.decl_nodes.len();
    let na = t.ir.arcs.len();

    let x: Vec<f64> = (0..n).map(|i| ((i * 7 % 11) as f64) - 5.0).collect();

    let mut y = vec![0.0; na];
    idx.node_to_arc(&x, &mut y);
    for (a, ya) in y.iter().enumerate() {
        let expect: f64 = (0..n).map(|j| b[a][j] * x[j]).sum();
        assert!((ya - expect).abs() < 1e-9, "Bx mismatch at arc {a}");
    }

    let yv: Vec<f64> = (0..na).map(|i| (i as f64) + 1.0).collect();
    let mut z = vec![0.0; n];
    idx.arc_to_node(&yv, &mut z);
    for (j, zj) in z.iter().enumerate() {
        let expect: f64 = (0..na).map(|a| b[a][j] * yv[a]).sum();
        assert!((zj - expect).abs() < 1e-9, "BTy mismatch at node {j}");
    }

    let bx: Vec<f64> = (0..na)
        .map(|a| (0..n).map(|k| b[a][k] * x[k]).sum())
        .collect();
    let mut zz = vec![0.0; n];
    let mut yscr = vec![0.0; na];
    idx.star_operator(&x, &mut zz, &mut yscr);
    for (j, zj) in zz.iter().enumerate() {
        let expect: f64 = (0..na).map(|a| b[a][j] * bx[a]).sum();
        assert!((zj - expect).abs() < 1e-9, "BTBx mismatch at node {j}");
    }
}

// --- fault ------------------------------------------------------------------

#[test]
fn dangling_target_is_a_typed_error() {
    let mut t = TestIr::new();
    let a = t.node(&["f", "a"], DeclId::NONE);
    let e = t.edge(&["f", "e"], DeclId::NONE);
    let bogus = DeclId::new(9_999);
    t.arc(e, &[(a, S::Plus), (bogus, S::Minus)]);
    let err = build_incidence_index(&t.ir).expect_err("dangling target must error");
    assert_eq!(
        err,
        IncidenceError::TargetOutOfRange {
            arc: HyperArcId::new(0),
            target: bogus,
        }
    );
}

// --- corrected reference benchmark (real production struct, µs, honest controls)

#[cfg(test)]
fn build_synth_ir(n_nodes: usize, n_arcs: usize, arity: usize, seed: u64) -> Ir {
    use hymeko::ir::ir::{AnnoR, ArcRec, DeclKind, DeclNode, EdgeRec, NodeRec, RefAtomR};
    let mut ir = Ir::default();
    let sym = hymeko::common::ids::SymId::new(0);
    let dn = |kind| DeclNode {
        kind,
        name: sym,
        parent: DeclId::NONE,
        first_child: DeclId::NONE,
        last_child: DeclId::NONE,
        next_sibling: DeclId::NONE,
        anno: AnnoR::default(),
    };
    for i in 0..n_nodes {
        ir.push_decl(dn(DeclKind::Node));
        ir.decl_hash.push(None);
        ir.nodes.push(NodeRec::new(DeclId::new(i), Vec::new()));
        ir.bind_node_decl(DeclId::new(i), hymeko::common::ids::NodeId::new(i))
            .unwrap();
    }
    let edge_decl = ir.push_decl(dn(DeclKind::Edge));
    ir.decl_hash.push(None);
    ir.edges.push(EdgeRec::new(edge_decl, Vec::new()));
    ir.bind_edge_decl(edge_decl, hymeko::common::ids::EdgeId::new(0))
        .unwrap();
    let mut s = seed | 1;
    let mut rng = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for a in 0..n_arcs {
        let refs: Vec<SignedRefR> = (0..arity)
            .map(|_| {
                let t = (rng() as usize) % n_nodes;
                SignedRefR::Plus(RefAtomR {
                    target: DeclId::new(t),
                    anno: AnnoR::default(),
                    weights: None,
                })
            })
            .collect();
        let ad = ir.push_anonymous_arc();
        ir.arcs.push(ArcRec {
            anno: AnnoR::default(),
            in_edge: edge_decl,
            refs,
        });
        ir.bind_arc_decl(ad, hymeko::common::ids::HyperArcId::new(a))
            .unwrap();
    }
    ir
}

/// Single typed time helper — always reports MICROSECONDS.
#[cfg(test)]
fn median_us<F: FnMut() -> u64>(reps: usize, mut f: F) -> f64 {
    use std::hint::black_box;
    for _ in 0..2 {
        black_box(f());
    }
    let mut v: Vec<f64> = (0..reps)
        .map(|_| {
            let t = std::time::Instant::now();
            let r = f();
            let us = t.elapsed().as_secs_f64() * 1e6;
            black_box(r);
            us
        })
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

#[test]
#[ignore]
fn bench_incidence_operator_vs_clique() {
    use std::hint::black_box;
    use std::mem::size_of;
    eprintln!(
        "[inc-bench] size_of HyperArcId={} u32={}  (all times MICROSECONDS)",
        size_of::<hymeko::common::ids::HyperArcId>(),
        size_of::<u32>()
    );
    for &(n_nodes, n_arcs, arity) in &[(500_000usize, 200_000usize, 5usize), (50_000, 50_000, 20)] {
        let nnz = n_arcs * arity;
        let ir = build_synth_ir(n_nodes, n_arcs, arity, 0x9E37_79B9);
        let n_decls = ir.decl_nodes.len();

        // B: sparse reverse-incidence index (arc, slot), REAL production sizes.
        let t_build = median_us(7, || {
            build_incidence_index(&ir).unwrap().incidence_count() as u64
        });
        let idx = build_incidence_index(&ir).unwrap();
        idx.validate().unwrap();
        let retained = idx.retained_bytes();

        let x: Vec<f64> = (0..n_decls).map(|i| (i % 7) as f64 - 3.0).collect();
        let mut y = vec![0.0; n_arcs];
        let mut z = vec![0.0; n_decls];
        let mut yscr = vec![0.0; n_arcs];
        let t_bx = median_us(9, || {
            idx.node_to_arc(&x, &mut y);
            black_box(y[0]).to_bits()
        });
        let t_bty = median_us(9, || {
            idx.arc_to_node(&y, &mut z);
            black_box(z[0]).to_bits()
        });
        let t_star = median_us(9, || {
            idx.star_operator(&x, &mut z, &mut yscr);
            black_box(z[0]).to_bits()
        });

        // D: clique / 2-section expansion, O(sum k^2). COMPLETE build (count + fill).
        let clique_legs: usize = ir
            .arcs
            .iter()
            .map(|a| a.refs.len() * (a.refs.len() - 1))
            .sum();
        let t_clique = median_us(3, || {
            let mut deg = vec![0u32; n_nodes];
            for arc in &ir.arcs {
                let k = arc.refs.len() as u32;
                for r in &arc.refs {
                    deg[r.target().raw()] += k - 1;
                }
            }
            let mut rowp = vec![0u32; n_nodes + 1];
            for i in 0..n_nodes {
                rowp[i + 1] = rowp[i] + deg[i];
            }
            let mut col = vec![0u32; rowp[n_nodes] as usize];
            let mut cur = rowp.clone();
            for arc in &ir.arcs {
                for (i, ri) in arc.refs.iter().enumerate() {
                    for (j, rj) in arc.refs.iter().enumerate() {
                        if i != j {
                            let u = ri.target().raw();
                            col[cur[u] as usize] = rj.target().raw() as u32;
                            cur[u] += 1;
                        }
                    }
                }
            }
            black_box(col.len()) as u64
        });
        let clique_bytes = clique_legs * size_of::<u32>() + (n_nodes + 1) * size_of::<u32>();

        eprintln!(
            "\n[inc-bench] --- n_nodes={n_nodes} n_arcs={n_arcs} arity={arity} nnz={nnz} ---"
        );
        eprintln!(
            "[inc-bench] B sparse index: build={:.0}us  retained={}B  bytes/inc={:.1}",
            t_build,
            retained,
            retained as f64 / nnz as f64
        );
        eprintln!(
            "[inc-bench] operator us: Bx={:.0}  BTy={:.0}  BTBx={:.0}",
            t_bx, t_bty, t_star
        );
        eprintln!(
            "[inc-bench] D clique/2-section (COMPLETE build): build={:.0}us  legs={}  bytes={}B  bytes/inc={:.1}  ({:.1}x index build, {:.1}x index memory)",
            t_clique,
            clique_legs,
            clique_bytes,
            clique_bytes as f64 / nnz as f64,
            t_clique / t_build,
            clique_bytes as f64 / retained as f64
        );
    }
}
