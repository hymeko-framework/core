use hymeko_wgpu::{KernelId, TensorShape, WgpuBackend};

const ABS_BOUND: f32 = 5.0e-5;
const REL_BOUND: f32 = 2.0e-4;

fn cr(control: &[f32], grid: usize, raw: f32) -> f32 {
    let x = raw.clamp(-1.0, 1.0);
    let u = (x + 1.0) * (0.5 * (grid - 1) as f32);
    let i = (u.floor() as usize).min(grid - 2);
    let t = u - i as f32;
    let t2 = t * t;
    let t3 = t2 * t;
    let w = [
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    ];
    let c = [
        i.saturating_sub(1).min(grid - 1),
        i,
        (i + 1).min(grid - 1),
        (i + 2).min(grid - 1),
    ];
    w.iter()
        .zip(c)
        .map(|(weight, index)| weight * control[index])
        .sum()
}

struct Fixture {
    x: Vec<f32>,
    vertices: Vec<u32>,
    signs: Vec<i32>,
    inner: Vec<f32>,
    outer: Vec<f32>,
    gate_w: Vec<f32>,
    gate_b: Vec<f32>,
}

fn fixture() -> Fixture {
    Fixture {
        x: vec![-0.8, 0.3, 0.2, -0.5, 0.9, 0.7],
        vertices: vec![0, 1, 2, 0],
        signs: vec![1, -1, 1, 1],
        inner: vec![
            -0.4, 0.1, 0.8, -0.2, 0.3, 0.6, 0.7, -0.1, -0.5, 0.4, 0.2, -0.3,
        ],
        outer: vec![
            -0.3, 0.2, 0.9, 0.1, -0.4, 0.5, 0.8, 0.0, -0.6, -0.2, 0.6, 0.3,
        ],
        gate_w: vec![0.4, -0.2, 0.1, 0.5],
        gate_b: vec![0.05, -0.15],
    }
}

fn reference(f: &Fixture) -> Vec<f32> {
    let (edges, arity, hidden, branches, grid) = (2, 2, 2, 2, 3);
    let mut out = vec![0.0; edges * hidden];
    for edge in 0..edges {
        for channel in 0..hidden {
            for branch in 0..branches {
                let wanted = if branch == 0 { 1 } else { -1 };
                let mut aggregate = 0.0;
                let mut count = 0.0f32;
                for member in 0..arity {
                    let incidence = edge * arity + member;
                    if f.signs[incidence] == wanted {
                        let vertex = f.vertices[incidence] as usize;
                        let row = &f.x[vertex * hidden..(vertex + 1) * hidden];
                        let z = f.gate_b[channel]
                            + f.gate_w[channel * hidden..(channel + 1) * hidden]
                                .iter()
                                .zip(row)
                                .map(|(w, x)| w * x)
                                .sum::<f32>();
                        let gate = 1.0 / (1.0 + (-z).exp());
                        let base = (branch * hidden + channel) * grid;
                        let inner = cr(&f.inner[base..base + grid], grid, row[channel]);
                        aggregate += gate * inner + (1.0 - gate) * row[channel];
                        count += 1.0;
                    }
                }
                aggregate /= count.max(1.0);
                let base = (branch * hidden + channel) * grid;
                out[edge * hidden + channel] += cr(&f.outer[base..base + grid], grid, aggregate);
            }
        }
    }
    out
}

#[test]
fn fused_hsikan_matches_the_predeclared_cpu_reference() {
    let Ok(backend) = WgpuBackend::new() else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let f = fixture();
    let expected = reference(&f);
    let resources = backend.resources();
    let x = resources
        .upload_tensor("x", TensorShape::new(3, 2), &f.x)
        .unwrap();
    let vertices = resources
        .upload_u32_tensor("vertices", TensorShape::new(2, 2), &f.vertices)
        .unwrap();
    let signs = resources
        .upload_i32_tensor("signs", TensorShape::new(2, 2), &f.signs)
        .unwrap();
    let inner = resources
        .upload_tensor("inner", TensorShape::new(4, 3), &f.inner)
        .unwrap();
    let outer = resources
        .upload_tensor("outer", TensorShape::new(4, 3), &f.outer)
        .unwrap();
    let gate_w = resources
        .upload_tensor("gate_w", TensorShape::new(2, 2), &f.gate_w)
        .unwrap();
    let gate_b = resources
        .upload_tensor("gate_b", TensorShape::new(2, 1), &f.gate_b)
        .unwrap();
    let output = resources
        .zeroed_tensor("output", TensorShape::new(2, 2))
        .unwrap();

    let op = backend
        .operations()
        .hsikan()
        .features(&x)
        .vertices(&vertices)
        .signs(&signs)
        .inner_control(&inner)
        .outer_control(&outer)
        .gate_weight(&gate_w)
        .gate_bias(&gate_b)
        .output(&output)
        .prepare()
        .unwrap();
    assert_eq!(op.kernel(), KernelId::Hsikan);
    assert_eq!(op.elements(), expected.len());
    let mut encoder = backend.encoder(Some("hsikan-test"));
    op.encode(&mut encoder).unwrap();
    let submission = backend.submit(encoder);
    backend.wait(submission).unwrap();
    let actual = backend.read_tensor(&output).unwrap();
    eprintln!("adapter={}", backend.capabilities().summary());
    for (index, (&got, &want)) in actual.iter().zip(&expected).enumerate() {
        assert!(got.is_finite() && want.is_finite());
        let error = (got - want).abs();
        let bound = ABS_BOUND + REL_BOUND * want.abs();
        assert!(
            error <= bound,
            "index {index}: got {got}, want {want}, error {error}, bound {bound}"
        );
    }
}

#[test]
fn shape_and_storage_limit_controls_are_rejected_before_dispatch() {
    let Ok(backend) = WgpuBackend::new() else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let resources = backend.resources();
    let bad = resources.upload_u32_tensor("bad", TensorShape::new(2, 2), &[0, 1, 2]);
    assert!(bad.is_err(), "typed upload must enforce its shape");

    let f = fixture();
    let x = resources
        .upload_tensor("x", TensorShape::new(3, 2), &f.x)
        .unwrap();
    let vertices = resources
        .upload_u32_tensor("v", TensorShape::new(2, 2), &f.vertices)
        .unwrap();
    let signs = resources
        .upload_i32_tensor("s", TensorShape::new(2, 2), &f.signs)
        .unwrap();
    let inner = resources
        .upload_tensor("i", TensorShape::new(4, 3), &f.inner)
        .unwrap();
    let outer = resources
        .upload_tensor("o", TensorShape::new(4, 3), &f.outer)
        .unwrap();
    let gate_w = resources
        .upload_tensor("w", TensorShape::new(2, 2), &f.gate_w)
        .unwrap();
    let gate_b = resources
        .upload_tensor("b", TensorShape::new(2, 1), &f.gate_b)
        .unwrap();
    let wrong_output = resources
        .zeroed_tensor("y", TensorShape::new(1, 2))
        .unwrap();
    let result = backend
        .operations()
        .hsikan()
        .features(&x)
        .vertices(&vertices)
        .signs(&signs)
        .inner_control(&inner)
        .outer_control(&outer)
        .gate_weight(&gate_w)
        .gate_bias(&gate_b)
        .output(&wrong_output)
        .prepare();
    assert!(result.is_err(), "wrong output shape must be rejected");
}
