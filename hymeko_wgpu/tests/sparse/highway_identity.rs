//! Gates for the identity-transport highway transition.
//!
//! The reference transcribes the CPU owner's expression and its operand order:
//! `let t = gate[i % d]; t * step[i] + (1.0 - t) * carry[i]`, with `t = 1/(1+exp(-b))`.

use hymeko_wgpu::{KernelId, TensorShape};

use crate::support::{assert_parity, series};

fn reference(step: &[f32], carry: &[f32], gate: &[f32], hidden: usize) -> Vec<f32> {
    (0..step.len())
        .map(|i| {
            let t = 1.0f32 / (1.0 + (-gate[i % hidden]).exp());
            t * step[i] + (1.0 - t) * carry[i]
        })
        .collect()
}

fn run(
    step: &[f32],
    carry: &[f32],
    gate: &[f32],
    n_nodes: usize,
    hidden: usize,
) -> Option<Vec<f32>> {
    let backend = crate::support::backend()?;
    let resources = backend.resources();
    let shape = TensorShape::new(n_nodes, hidden);
    let step_t = resources.upload_tensor("step", shape, step).unwrap();
    let carry_t = resources.upload_tensor("carry", shape, carry).unwrap();
    let gate_t = resources
        .upload_tensor("gate", TensorShape::new(hidden, 1), gate)
        .unwrap();
    let next_t = resources.zeroed_tensor("next", shape).unwrap();

    let op = backend
        .operations()
        .highway_identity()
        .step(&step_t)
        .carry(&carry_t)
        .gate(&gate_t)
        .next(&next_t)
        .prepare()
        .unwrap();
    assert_eq!(op.kernel(), KernelId::HighwayIdentity);
    assert_eq!(op.elements(), n_nodes * hidden);
    let mut encoder = backend.encoder(Some("highway-test"));
    op.encode(&mut encoder).unwrap();
    let submission = backend.submit(encoder);
    backend.wait(submission).unwrap();
    Some(backend.read_tensor(&next_t).unwrap())
}

#[test]
fn the_gate_saturates_in_both_directions_and_matches_the_cpu_owner() {
    // Bias -4 / 0 / +4: the carry-dominant, balanced and step-dominant regimes.
    let (n_nodes, hidden) = (4usize, 3usize);
    let step = series(n_nodes * hidden, 1, 0.6);
    let carry = series(n_nodes * hidden, 11, 0.4);
    for bias in [-4.0f32, 0.0, 4.0] {
        let gate = vec![bias; hidden];
        let expected = reference(&step, &carry, &gate, hidden);
        let Some(actual) = run(&step, &carry, &gate, n_nodes, hidden) else {
            eprintln!("SKIP: no WGPU adapter");
            return;
        };
        assert_parity(&format!("bias {bias}"), &actual, &expected);
    }
}

#[test]
fn a_per_channel_gate_is_read_per_channel_not_per_element() {
    // Distinct bias per channel, so an implementation indexing the gate by element rather than by
    // `i % hidden` diverges immediately.
    let (n_nodes, hidden) = (3usize, 4usize);
    let step = series(n_nodes * hidden, 2, 0.6);
    let carry = series(n_nodes * hidden, 7, 0.5);
    let gate = vec![-3.0f32, -0.5, 0.5, 3.0];
    let expected = reference(&step, &carry, &gate, hidden);
    let Some(actual) = run(&step, &carry, &gate, n_nodes, hidden) else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    // The control is discriminating only if the channels actually differ.
    assert!(expected[0] != expected[1], "the reference is channel-flat");
    assert_parity("per-channel gate", &actual, &expected);
}

#[test]
fn several_shapes_and_widths_match_including_non_square() {
    for (n_nodes, hidden) in [(1usize, 1usize), (7, 2), (2, 8), (5, 3)] {
        let step = series(n_nodes * hidden, 3, 0.6);
        let carry = series(n_nodes * hidden, 13, 0.35);
        let gate = series(hidden, 5, 2.0);
        let expected = reference(&step, &carry, &gate, hidden);
        let Some(actual) = run(&step, &carry, &gate, n_nodes, hidden) else {
            eprintln!("SKIP: no WGPU adapter");
            return;
        };
        assert_parity(&format!("n={n_nodes} d={hidden}"), &actual, &expected);
    }
}

#[test]
fn shape_disagreements_and_missing_operands_are_refused_before_dispatch() {
    let Some(backend) = crate::support::backend() else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let resources = backend.resources();
    let shape = TensorShape::new(3, 2);
    let step = resources.upload_tensor("step", shape, &[0.1; 6]).unwrap();
    let carry = resources.upload_tensor("carry", shape, &[0.2; 6]).unwrap();
    let gate = resources
        .upload_tensor("gate", TensorShape::new(2, 1), &[0.0; 2])
        .unwrap();
    let short_gate = resources
        .upload_tensor("short_gate", TensorShape::new(1, 1), &[0.0])
        .unwrap();
    let wrong_carry = resources
        .upload_tensor("wrong_carry", TensorShape::new(2, 2), &[0.0; 4])
        .unwrap();
    let next = resources.zeroed_tensor("next", shape).unwrap();

    assert!(
        backend
            .operations()
            .highway_identity()
            .step(&step)
            .carry(&wrong_carry)
            .gate(&gate)
            .next(&next)
            .prepare()
            .is_err(),
        "a carry of the wrong shape was accepted"
    );
    assert!(
        backend
            .operations()
            .highway_identity()
            .step(&step)
            .carry(&carry)
            .gate(&short_gate)
            .next(&next)
            .prepare()
            .is_err(),
        "a gate shorter than hidden was accepted"
    );
    assert!(
        backend
            .operations()
            .highway_identity()
            .step(&step)
            .gate(&gate)
            .next(&next)
            .prepare()
            .is_err(),
        "a missing carry was accepted"
    );
    assert!(
        backend
            .operations()
            .highway_identity()
            .step(&step)
            .carry(&carry)
            .gate(&gate)
            .next(&next)
            .prepare()
            .is_ok(),
        "the well-formed combination must still prepare"
    );
}

#[test]
fn the_shader_reuses_the_activation_owners_sigmoid_rather_than_defining_one() {
    let shader = hymeko_wgpu::HIGHWAY_IDENTITY_SHADER;
    // Composed with the formulas module, so `act_sigmoid` is defined exactly once and by its owner.
    assert_eq!(
        shader.matches("fn act_sigmoid").count(),
        1,
        "act_sigmoid must be defined once, by activation_formulas.wgsl"
    );
    assert!(
        shader.contains("act_sigmoid(gate[channel], 0.0)"),
        "the transition must call the activation owner's sigmoid"
    );
    let code: String = shader
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    // The expression, in the CPU owner's operand order.
    assert!(
        code.contains("t * step_state[i] + (1.0 - t) * carry_state[i]"),
        "the transition expression drifted from the CPU owner's operand order"
    );
}
