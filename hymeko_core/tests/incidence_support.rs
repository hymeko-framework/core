//! Shared incidence-test scaffolding (§6.1 consolidation): the tiny `Ir`-builder
//! helpers, the bit-exact float comparator, the median timer and the independent
//! dense-Bᵀ oracle — used by both `incidence_precompute_profile` (P2) and
//! `prepared_ir_backbone` (P2.1). Wired into `tests/mod.rs` (autotests=false).

use std::time::Instant;

use hymeko::common::ids::{DeclId, NodeId, SymId};
use hymeko::ir::incidence::{IncidenceIndex, scalar_weight};
use hymeko::ir::ir::{
    AnnoR, ArcRec, DeclKind, DeclNode, Ir, NodeRec, PortDirection, RefAtomR, SignedRefR, ValueR,
};

/// Median of `iters` timed calls (2 warm-ups first), in milliseconds.
pub(crate) fn median_ms(iters: usize, mut f: impl FnMut()) -> f64 {
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

/// Push a bound `Node` declaration, returning its `DeclId`.
pub(crate) fn node(ir: &mut Ir) -> DeclId {
    let d = ir.push_decl(DeclNode {
        kind: DeclKind::Node,
        name: SymId::new(0),
        parent: DeclId::NONE,
        first_child: DeclId::NONE,
        last_child: DeclId::NONE,
        next_sibling: DeclId::NONE,
        anno: AnnoR::default(),
    });
    let nid = NodeId::new(ir.nodes.len());
    ir.nodes.push(NodeRec::new(d, Vec::new()));
    ir.bind_node_decl(d, nid).unwrap();
    d
}

/// A ref with sign in {+1, −1, 0} and an optional weight list.
pub(crate) fn r(target: usize, sign: i8, weights: Option<Vec<f64>>) -> SignedRefR {
    let atom = RefAtomR {
        target: DeclId::new(target),
        anno: AnnoR::default(),
        weights: weights.map(|ws| ws.into_iter().map(ValueR::Num).collect()),
        direction: PortDirection::Unspecified,
    };
    match sign {
        s if s < 0 => SignedRefR::Minus(atom),
        s if s > 0 => SignedRefR::Plus(atom),
        _ => SignedRefR::Neutral(atom),
    }
}

/// Push an arc with the given refs (no owning edge).
pub(crate) fn arc(ir: &mut Ir, refs: Vec<SignedRefR>) {
    ir.arcs.push(ArcRec {
        anno: AnnoR::default(),
        in_edge: DeclId::NONE,
        refs,
    });
}

/// A `k`-wide sliding window over `m` nodes (`m − k + 1` arcs); sign flips on every
/// third target, a single per-target weight. Deterministic; sized to hit CSR row /
/// degree boundaries and to drive the large-workload benches.
pub(crate) fn windowed(m: usize, k: usize) -> Ir {
    let mut ir = Ir::default();
    for _ in 0..m {
        node(&mut ir);
    }
    for i in 0..(m - k + 1) {
        let refs: Vec<_> = (i..i + k)
            .map(|j| {
                let sign = if j % 3 == 1 { -1 } else { 1 };
                r(j, sign, Some(vec![1.0 + (j % 7) as f64 * 0.25]))
            })
            .collect();
        arc(&mut ir, refs);
    }
    ir
}

/// Independent dense-Bᵀ oracle: `z[d] = Σ_arc (Σ_incidence of d in arc  sign·weight) · y[arc]`.
pub(crate) fn dense_bt(ir: &Ir, index: &IncidenceIndex<'_>, y: &[f64]) -> Vec<f64> {
    let n = ir.decl_nodes.len();
    let mut z = vec![0.0; n];
    for d in 0..n {
        let mut acc = 0.0;
        for (i, &a) in index.incident_arcs(DeclId::new(d)).iter().enumerate() {
            let slot = index.incident_slots(DeclId::new(d))[i];
            let sref = &ir.arcs[a.raw()].refs[slot as usize];
            acc += f64::from(sref.sign()) * scalar_weight(sref) * y[a.raw()];
        }
        z[d] = acc;
    }
    z
}

/// Bit-exact float-slice equality (`to_bits`, so `-0.0 != 0.0`, `NaN` pattern-exact).
pub(crate) fn bits_eq(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}
