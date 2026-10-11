//! P2 V&V + decision-gate bench for `IncidenceOperatorPlan` (prepared Bᵀ). Wired into `tests/mod.rs`
//! (autotests=false). Correctness: prepared == legacy **bitwise** across sign/weight variants; serial ==
//! Rayon bitwise; an independent dense-Bᵀ oracle; NaN/Inf rejected at prepare; empty/single/duplicate/-0.0.
//! Bench: the §4.1/§4.5 profile (30× confirmed, break-even, Bx no-regression, Rayon scaling).

use hymeko::ir::incidence::{OperatorPlanError, build_incidence_index};
use hymeko::ir::ir::Ir;

use super::incidence_support::{arc, bits_eq, dense_bt, median_ms, node, r, windowed};

/// Runs independent expected results through live, prepared and Rayon applies.
struct TransposeCase {
    ir: Ir,
    y: Vec<f64>,
    expected: Vec<f64>,
}

impl TransposeCase {
    fn check(&self) {
        let index = build_incidence_index(&self.ir).unwrap();
        let plan = index.prepare_operator().unwrap();
        let n = self.ir.decl_nodes.len();
        assert_eq!(self.expected.len(), n);
        let tail = [-0.0, f64::from_bits(0x7ff8_0000_0000_0042)];
        let mut outputs: [Vec<f64>; 3] = std::array::from_fn(|_| {
            let mut output = vec![123.0; n];
            output.extend_from_slice(&tail);
            output
        });
        for _ in 0..2 {
            for output in &mut outputs {
                output[..n].fill(123.0);
            }
            index.arc_to_node(&self.y, &mut outputs[0]);
            plan.arc_to_node(&self.y, &mut outputs[1]);
            plan.par_arc_to_node(&self.y, &mut outputs[2]);
            for output in &outputs {
                for (&actual, &expected) in output.iter().zip(&self.expected) {
                    if expected.is_nan() {
                        assert!(actual.is_nan());
                    } else {
                        assert_eq!(actual.to_bits(), expected.to_bits());
                    }
                }
                assert!(bits_eq(&output[n..], &tail), "output tail changed");
            }
        }
    }
}

#[test]
fn transpose_keeps_order_rounding_empty_rows_and_output_tails() {
    let mut ir = Ir::default();
    for _ in 0..3 {
        node(&mut ir);
    }
    arc(&mut ir, vec![r(0, 1, Some(vec![1e16])), r(1, 0, None)]);
    arc(
        &mut ir,
        vec![r(0, 1, None), r(0, -1, Some(vec![1e16])), r(0, 1, None)],
    );
    arc(&mut ir, vec![]);
    // Left-to-right: ((1e16 + 1) - 1e16) + 1 == 1, not a regrouped 2.
    TransposeCase {
        ir,
        y: vec![1.0, 1.0, -10.0],
        expected: vec![1.0, 0.0, 0.0],
    }
    .check();

    let mut ir = Ir::default();
    node(&mut ir);
    arc(&mut ir, vec![r(0, -1, None)]);
    arc(&mut ir, vec![r(0, 1, Some(vec![1.0 + 2.0_f64.powi(-27)]))]);
    // Separate multiplication rounds to 1; fusing into the sum would give -2^-54.
    TransposeCase {
        ir,
        y: vec![1.0, 1.0 - 2.0_f64.powi(-27)],
        expected: vec![0.0],
    }
    .check();

    TransposeCase {
        ir: Ir::default(),
        y: vec![],
        expected: vec![],
    }
    .check();
    let mut ir = Ir::default();
    node(&mut ir);
    node(&mut ir);
    arc(&mut ir, vec![]);
    TransposeCase {
        ir,
        y: vec![f64::INFINITY],
        expected: vec![0.0, 0.0],
    }
    .check();
}

#[test]
fn neutral_incidence_still_multiplies_nonfinite_input() {
    for y in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut ir = Ir::default();
        node(&mut ir);
        node(&mut ir);
        arc(&mut ir, vec![r(0, 0, Some(vec![-0.0]))]);
        TransposeCase {
            ir,
            y: vec![y],
            expected: vec![f64::NAN, 0.0],
        }
        .check();
    }
}

#[test]
fn short_buffers_preserve_serial_partial_writes_and_parallel_preflight() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let mut ir = Ir::default();
    for _ in 0..3 {
        node(&mut ir);
    }
    arc(&mut ir, vec![r(0, 1, Some(vec![2.0]))]);
    arc(&mut ir, vec![r(1, 1, Some(vec![3.0]))]);
    let index = build_incidence_index(&ir).unwrap();
    let plan = index.prepare_operator().unwrap();
    for prepared in [false, true] {
        let mut output = [123.0; 4];
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                if prepared {
                    plan.arc_to_node(&[5.0], &mut output);
                } else {
                    index.arc_to_node(&[5.0], &mut output);
                }
            }))
            .is_err()
        );
        assert!(bits_eq(&output, &[10.0, 0.0, 0.0, 123.0]));
    }
    let mut output = [123.0; 2];
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            plan.par_arc_to_node(&[5.0, 7.0], &mut output);
        }))
        .is_err()
    );
    assert!(bits_eq(&output, &[123.0; 2]));
}

#[test]
fn prepared_equals_legacy_and_oracle_across_sign_weight_variants() {
    // 4 nodes, 3 arcs mixing +/-/~, none/single/multi weight, a duplicate incidence, and -0.0.
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
    ); // dup node 0
    arc(
        &mut ir,
        vec![r(1, 0, Some(vec![5.0])), r(2, 1, Some(vec![2.0, 0.5]))],
    ); // ~ (sign 0) + multi-weight
    arc(&mut ir, vec![r(2, -1, Some(vec![-0.0])), r(3, 1, None)]); // -0.0 weight
    let index = build_incidence_index(&ir).unwrap();
    let plan = index.prepare_operator().expect("finite");
    let n_arcs = ir.arcs.len();
    let n = ir.decl_nodes.len();
    let y: Vec<f64> = (0..n_arcs).map(|i| 1.0 + i as f64).collect();

    let mut z_legacy = vec![0.0; n];
    let mut z_prep = vec![0.0; n];
    let mut z_par = vec![0.0; n];
    index.arc_to_node(&y, &mut z_legacy);
    plan.arc_to_node(&y, &mut z_prep);
    plan.par_arc_to_node(&y, &mut z_par);
    let oracle = dense_bt(&ir, &index, &y);

    assert!(bits_eq(&z_prep, &z_legacy), "prepared != legacy (bitwise)");
    assert!(bits_eq(&z_par, &z_legacy), "rayon != serial (bitwise)");
    assert!(bits_eq(&z_prep, &oracle), "prepared != dense oracle");

    // star_operator parity (BᵀBx).
    let x: Vec<f64> = (0..n).map(|i| 0.5 + i as f64).collect();
    let (mut zs_l, mut zs_p, mut ys1, mut ys2) = (
        vec![0.0; n],
        vec![0.0; n],
        vec![0.0; n_arcs],
        vec![0.0; n_arcs],
    );
    index.star_operator(&x, &mut zs_l, &mut ys1);
    plan.star_operator(&x, &mut zs_p, &mut ys2);
    assert!(bits_eq(&zs_p, &zs_l), "prepared star != legacy star");
}

#[test]
fn empty_and_single_incidence() {
    // empty graph
    let ir0 = Ir::default();
    let idx0 = build_incidence_index(&ir0).unwrap();
    let plan0 = idx0.prepare_operator().unwrap();
    assert_eq!(plan0.retained_bytes(), 0);
    // single node, single self-incidence
    let mut ir = Ir::default();
    node(&mut ir);
    arc(&mut ir, vec![r(0, 1, Some(vec![4.0]))]);
    let idx = build_incidence_index(&ir).unwrap();
    let plan = idx.prepare_operator().unwrap();
    let (mut zl, mut zp) = (vec![0.0; 1], vec![0.0; 1]);
    idx.arc_to_node(&[2.0], &mut zl);
    plan.arc_to_node(&[2.0], &mut zp);
    assert!(bits_eq(&zp, &zl));
}

#[test]
fn nonfinite_coefficients_rejected_in_node_order() {
    for (sign, weights) in [
        (1, vec![f64::NAN]),
        (1, vec![f64::INFINITY]),
        (1, vec![f64::NEG_INFINITY]),
        (0, vec![f64::INFINITY]),
        (1, vec![f64::MAX, 2.0]),
    ] {
        let mut ir = Ir::default();
        node(&mut ir);
        node(&mut ir);
        // The first arc/ref is node 1, but prepare must diagnose node 0 first.
        arc(
            &mut ir,
            vec![
                r(1, 1, Some(vec![f64::INFINITY])),
                r(0, sign, Some(weights)),
            ],
        );
        let idx = build_incidence_index(&ir).unwrap();
        assert!(
            matches!(
                idx.prepare_operator(),
                Err(OperatorPlanError::NonFiniteCoefficient { node }) if node.raw() == 0
            ),
            "prepare must reject the first nonfinite coefficient in node order"
        );
        let mut output = [123.0; 2];
        idx.arc_to_node(&[1.0], &mut output);
        assert!(output.iter().all(|value| !value.is_finite()));
    }
}

#[test]
fn irregular_high_cardinality_parity() {
    // one arc over all nodes (high cardinality) + irregular small arcs.
    let mut ir = Ir::default();
    for _ in 0..64 {
        node(&mut ir);
    }
    arc(
        &mut ir,
        (0..64)
            .map(|j| {
                r(
                    j,
                    if j % 2 == 0 { 1 } else { -1 },
                    Some(vec![1.0 + j as f64 * 0.1]),
                )
            })
            .collect(),
    );
    arc(&mut ir, vec![r(0, 1, None), r(63, -1, Some(vec![2.0]))]);
    let idx = build_incidence_index(&ir).unwrap();
    let plan = idx.prepare_operator().unwrap();
    let n_arcs = ir.arcs.len();
    let y: Vec<f64> = (0..n_arcs).map(|i| 1.0 + i as f64).collect();
    let (mut zl, mut zp, mut zpar) = (vec![0.0; 64], vec![0.0; 64], vec![0.0; 64]);
    idx.arc_to_node(&y, &mut zl);
    plan.arc_to_node(&y, &mut zp);
    plan.par_arc_to_node(&y, &mut zpar);
    assert!(bits_eq(&zp, &zl) && bits_eq(&zpar, &zl));
}

#[test]
#[ignore = "performance benchmark; opt in with --ignored --exact"]
fn bench_operator_plan_decision_gate() {
    let (m, k) = (200_001usize, 5usize);
    let ir = windowed(m, k);
    let index = build_incidence_index(&ir).unwrap();
    let n = ir.decl_nodes.len();
    let n_arcs = ir.arcs.len();
    let nnz: usize = ir.arcs.iter().map(|a| a.refs.len()).sum();
    println!("\n=== P2 operator-plan decision gate — arity {k}, {nnz} incidences ===");
    let plan = index.prepare_operator().unwrap();

    let y: Vec<f64> = (0..n_arcs).map(|i| 1.0 + (i % 11) as f64 * 0.1).collect();
    let x: Vec<f64> = (0..n).map(|i| 1.0 + (i % 13) as f64 * 0.1).collect();
    let (mut z1, mut z2, mut ys) = (vec![0.0; n], vec![0.0; n], vec![0.0; n_arcs]);

    index.arc_to_node(&y, &mut z1);
    plan.arc_to_node(&y, &mut z2);
    assert!(bits_eq(&z1, &z2), "prepared Bᵀy must be bitwise-identical");

    let legacy_bty = median_ms(9, || index.arc_to_node(&y, &mut z1));
    let prep_bty = median_ms(9, || plan.arc_to_node(&y, &mut z2));
    let par_bty = median_ms(9, || plan.par_arc_to_node(&y, &mut z2));
    let prep_cost = median_ms(5, || {
        let _ = index.prepare_operator().unwrap();
    });
    let legacy_star = median_ms(9, || index.star_operator(&x, &mut z1, &mut ys));
    let prep_star = median_ms(9, || plan.star_operator(&x, &mut z2, &mut ys));
    let mut bx = vec![0.0; n_arcs];
    let legacy_bx = median_ms(9, || index.node_to_arc(&x, &mut bx));

    println!("legacy  Bᵀy : {legacy_bty:.3} ms");
    println!(
        "prepared Bᵀy: {prep_bty:.3} ms   ({:.1}x)",
        legacy_bty / prep_bty.max(1e-9)
    );
    println!(
        "rayon    Bᵀy: {par_bty:.3} ms   ({:.1}x vs serial-prepared)",
        prep_bty / par_bty.max(1e-9)
    );
    println!("legacy  BᵀBx: {legacy_star:.3} ms");
    println!(
        "prepared BᵀBx: {prep_star:.3} ms  ({:.1}x)",
        legacy_star / prep_star.max(1e-9)
    );
    println!("Bx (untouched): {legacy_bx:.3} ms");
    println!(
        "prepare cost: {prep_cost:.3} ms   break-even {:.1} applies",
        prep_cost / (legacy_bty - prep_bty).max(1e-9)
    );
    println!(
        "plan bytes/incidence: {:.1}",
        plan.retained_bytes() as f64 / nnz as f64
    );
}
