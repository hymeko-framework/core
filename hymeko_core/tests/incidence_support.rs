//! Shared incidence-test scaffolding (§6.1 consolidation): the tiny `Ir`-builder
//! helpers, the bit-exact float comparator, the median timer and the independent
//! dense-Bᵀ oracle — used by both `incidence_precompute_profile` (P2) and
//! `prepared_ir_backbone` (P2.1). Wired into `tests/mod.rs` (autotests=false).

use hymeko::ir::ir::IrConstruction;
use hymeko::util::measurement::{MeasurementRunner, Runner};

use hymeko::common::ids::{DeclId, NodeId, SymId};
use hymeko::ir::incidence::IncidenceIndex;
use hymeko::ir::ir::{
    AnnoR, ArcRec, DeclKind, DeclNode, IncidenceSign, Ir, NodeRec, PortDirection, RefAtomR,
    ReferenceDraft, ValueR,
};

/// Median of `iters` timed calls (2 warm-ups first), in milliseconds.
pub(crate) fn median_ms(iters: usize, f: impl FnMut()) -> f64 {
    MeasurementRunner::new(2, iters).run(f).median / 1000.0
}

/// Push a bound `Node` declaration, returning its `DeclId`.
pub(crate) fn node(ir: &mut Ir) -> DeclId {
    IrConstruction::new(ir)
        .append_node(SymId::new(0), AnnoR::default(), Vec::new())
        .unwrap()
}

/// A ref with sign in {+1, −1, 0} and an optional weight list.
pub(crate) fn r(target: usize, sign: i8, weights: Option<Vec<f64>>) -> ReferenceDraft {
    let atom = RefAtomR::plain(DeclId::new(target))
        .with_weights(weights.map(|ws| ws.into_iter().map(ValueR::Num).collect()));
    match sign {
        s if s < 0 => ReferenceDraft::new(IncidenceSign::Minus, atom),
        s if s > 0 => ReferenceDraft::new(IncidenceSign::Plus, atom),
        _ => ReferenceDraft::new(IncidenceSign::Neutral, atom),
    }
}

/// Push an arc with the given refs (no owning edge).
pub(crate) fn arc(ir: &mut Ir, refs: Vec<ReferenceDraft>) {
    let refs = refs
        .into_iter()
        .map(|reference| ir.store_reference(reference))
        .collect();
    ir.arcs.push(ArcRec::unowned(refs));
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
            acc += f64::from(sref.sign()) * sref.scalar_weight(ir) * y[a.raw()];
        }
        z[d] = acc;
    }
    z
}

/// Bit-exact float-slice equality (`to_bits`, so `-0.0 != 0.0`, `NaN` pattern-exact).
pub(crate) fn bits_eq(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}
