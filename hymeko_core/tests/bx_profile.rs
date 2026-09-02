//! Part-2 §4 Bx profile. After the Bᵀ precompute (P2), Bx (`node_to_arc`, arc-major) is the dominant term
//! of BᵀBx. This isolates whether a *second* (arc-major) coefficient precompute would pay — the mission
//! forbids a second array without measurement. Bx already iterates `arc.refs` contiguously; the candidate
//! removes only the `sign()`/`scalar_weight` recompute (it still reads `target` from the ref, i.e. no
//! topology copy). Wired into `tests/mod.rs` (autotests=false).

use std::time::Instant;

use hymeko::common::ids::{DeclId, NodeId};
use hymeko::ir::incidence::scalar_weight;
use hymeko::ir::ir::{
    AnnoR, ArcRec, DeclKind, DeclNode, Ir, NodeRec, RefAtomR, SignedRefR, ValueR,
};

fn median_ms(iters: usize, mut f: impl FnMut()) -> f64 {
    for _ in 0..2 {
        f();
    }
    let mut s = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        f();
        s.push(t.elapsed().as_secs_f64() * 1e3);
    }
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[s.len() / 2]
}

fn weighted_window(m: usize, k: usize) -> Ir {
    let mut ir = Ir::default();
    for _ in 0..m {
        let d = ir.push_decl(DeclNode {
            kind: DeclKind::Node,
            name: hymeko::common::ids::SymId::new(0),
            parent: DeclId::NONE,
            first_child: DeclId::NONE,
            last_child: DeclId::NONE,
            next_sibling: DeclId::NONE,
            anno: AnnoR::default(),
        });
        let nid = NodeId::new(ir.nodes.len());
        ir.nodes.push(NodeRec::new(d, Vec::new()));
        ir.bind_node_decl(d, nid).unwrap();
    }
    for i in 0..(m - k + 1) {
        let refs: Vec<SignedRefR> = (i..i + k)
            .map(|j| {
                let atom = RefAtomR {
                    target: DeclId::new(j),
                    anno: AnnoR::default(),
                    weights: Some(vec![ValueR::Num(1.0 + (j % 7) as f64 * 0.25)]),
                };
                if j % 3 == 1 {
                    SignedRefR::Minus(atom)
                } else {
                    SignedRefR::Plus(atom)
                }
            })
            .collect();
        ir.arcs.push(ArcRec {
            anno: AnnoR::default(),
            in_edge: DeclId::NONE,
            refs,
        });
    }
    ir
}

#[test]
fn profile_bx_decomposition() {
    let (m, k) = (200_001usize, 5usize);
    let ir = weighted_window(m, k);
    let index = hymeko::ir::incidence::build_incidence_index(&ir).unwrap();
    let n = ir.decl_nodes.len();
    let n_arcs = ir.arcs.len();
    let nnz: usize = ir.arcs.iter().map(|a| a.refs.len()).sum();
    let x: Vec<f64> = (0..n).map(|i| 1.0 + (i % 13) as f64 * 0.1).collect();
    let mut y = vec![0.0; n_arcs];
    println!("\n=== Part-2 §4 Bx profile — arity {k}, {nnz} incidences ===");

    // arc-major prepared coefficient (parallel to arc.refs order) — the candidate second array.
    let mut coeff_am: Vec<f64> = Vec::with_capacity(nnz);
    for a in &ir.arcs {
        for sref in &a.refs {
            coeff_am.push(f64::from(sref.sign()) * scalar_weight(sref));
        }
    }
    // (A) current Bx
    let cur = median_ms(9, || index.node_to_arc(&x, &mut y));
    // (B) prepared-coeff Bx: skips sign()/scalar_weight, still reads target from the ref (no topology copy)
    let prep = median_ms(9, || {
        let mut ci = 0usize;
        for (ai, a) in ir.arcs.iter().enumerate() {
            let mut acc = 0.0;
            for sref in &a.refs {
                acc += coeff_am[ci] * x[sref.target().raw()];
                ci += 1;
            }
            y[ai] = acc;
        }
    });
    // (C) memory-traffic floor: same scatter, coefficient = 1.0 (no weight/sign at all)
    let floor = median_ms(9, || {
        for (ai, a) in ir.arcs.iter().enumerate() {
            let mut acc = 0.0;
            for sref in &a.refs {
                acc += x[sref.target().raw()];
            }
            y[ai] = acc;
        }
    });

    println!("(A) current Bx            : {cur:.3} ms");
    println!(
        "(B) prepared-coeff Bx     : {prep:.3} ms   ({:.2}x)",
        cur / prep.max(1e-9)
    );
    println!("(C) scatter floor (coef=1): {floor:.3} ms   (memory-traffic bound)");
    println!(
        "sign+weight resolution share: {:.0}%   scatter/traversal share: {:.0}%",
        (cur - floor) / cur * 100.0,
        floor / cur * 100.0
    );
    println!(
        "second-array cost would be +8 bytes/incidence (doubles the plan) — no consumer to justify it."
    );
}
