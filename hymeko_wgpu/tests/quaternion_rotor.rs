use hymeko::tensor::compute::{SparseCsrShape, TensorShape};
use hymeko_wgpu::{KernelId, WgpuBackend};

const ABS_BOUND: f32 = 5.0e-4;
const REL_BOUND: f32 = 1.0e-3;

fn close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    assert!(expected.iter().any(|value| value.abs() > 1.0e-6));
    for (index, (&got, &want)) in actual.iter().zip(expected).enumerate() {
        assert!(got.is_finite() && want.is_finite());
        let error = (got - want).abs();
        let bound = ABS_BOUND + REL_BOUND * want.abs();
        assert!(
            error <= bound,
            "index {index}: got {got}, want {want}, error {error}, bound {bound}"
        );
    }
}

fn normalise(q: &[f32]) -> [f32; 4] {
    let norm = q.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm < 1.0e-6 {
        [1.0, 0.0, 0.0, 0.0]
    } else {
        [q[0] / norm, q[1] / norm, q[2] / norm, q[3] / norm]
    }
}

fn mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
        a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
        a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
        a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
    ]
}

fn attention_reference(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    p: &[f32],
    offsets: &[usize],
    sources: &[usize],
    kappa: f32,
) -> Vec<f32> {
    let tokens = q.len() / 4;
    let preferred = normalise(p);
    let mut out = vec![0.0; tokens * 4];
    for row in 0..tokens {
        let (lo, hi) = (offsets[row], offsets[row + 1]);
        if lo == hi {
            continue;
        }
        let unit = normalise(&q[row * 4..row * 4 + 4]);
        let conjugate = [unit[0], -unit[1], -unit[2], -unit[3]];
        let scores: Vec<f32> = sources[lo..hi]
            .iter()
            .map(|&source| {
                let relative = mul(conjugate, normalise(&k[source * 4..source * 4 + 4]));
                let alignment: f32 = preferred.iter().zip(relative).map(|(a, b)| a * b).sum();
                kappa * (alignment * alignment - 1.0)
            })
            .collect();
        let maximum = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let denominator: f32 = scores.iter().map(|score| (score - maximum).exp()).sum();
        for (&source, score) in sources[lo..hi].iter().zip(scores) {
            let weight = (score - maximum).exp() / denominator;
            for channel in 0..4 {
                out[row * 4 + channel] += weight * v[source * 4 + channel];
            }
        }
    }
    out
}

#[test]
fn sparse_rotor_attention_matches_the_predeclared_reference() {
    let Ok(backend) = WgpuBackend::new() else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let q = vec![
        1.0, 0.1, -0.2, 0.3, 0.8, -0.4, 0.2, 0.1, 0.7, 0.5, -0.1, 0.2,
    ];
    let k = vec![
        0.9, -0.1, 0.3, 0.2, 0.6, 0.4, -0.3, 0.1, 0.8, -0.2, 0.1, 0.5,
    ];
    let values = vec![
        -0.3, 0.2, 0.7, -0.4, 0.9, -0.5, 0.1, 0.6, 0.4, 0.8, -0.2, 0.3,
    ];
    let preferred = vec![0.8, 0.2, -0.1, 0.5];
    let offsets = vec![0usize, 2, 3, 3];
    let sources = vec![1usize, 2, 0];
    let kappa = 2.3;
    let expected = attention_reference(&q, &k, &values, &preferred, &offsets, &sources, kappa);
    assert_eq!(&expected[8..12], &[0.0; 4], "empty row control");

    let resources = backend.resources();
    let q_gpu = resources
        .upload_tensor("q", TensorShape::new(3, 4), &q)
        .unwrap();
    let k_gpu = resources
        .upload_tensor("k", TensorShape::new(3, 4), &k)
        .unwrap();
    let v_gpu = resources
        .upload_tensor("v", TensorShape::new(3, 4), &values)
        .unwrap();
    let p_gpu = resources
        .upload_tensor("p", TensorShape::new(1, 4), &preferred)
        .unwrap();
    let adjacency = resources
        .upload_csr(
            "adj",
            SparseCsrShape::new(3, 3, 3),
            &offsets,
            &sources,
            &[1.0; 3],
        )
        .unwrap();
    let output = resources
        .zeroed_tensor("y", TensorShape::new(3, 4))
        .unwrap();
    let op = backend
        .operations()
        .rotor_attention()
        .queries(&q_gpu)
        .keys(&k_gpu)
        .values(&v_gpu)
        .preferred(&p_gpu)
        .adjacency(&adjacency)
        .output(&output)
        .kappa(kappa)
        .prepare()
        .unwrap();
    assert_eq!(op.kernel(), KernelId::RotorAttention);
    let mut encoder = backend.encoder(Some("rotor-attention-test"));
    op.encode(&mut encoder).unwrap();
    let submission = backend.submit(encoder);
    backend.wait(submission).unwrap();
    let actual = backend.read_tensor(&output).unwrap();
    close(&actual, &expected);

    let preferred_negated: Vec<f32> = preferred.iter().map(|value| -*value).collect();
    let p_neg = resources
        .upload_tensor("p-neg", TensorShape::new(1, 4), &preferred_negated)
        .unwrap();
    let output_neg = resources
        .zeroed_tensor("y-neg", TensorShape::new(3, 4))
        .unwrap();
    let op_neg = backend
        .operations()
        .rotor_attention()
        .queries(&q_gpu)
        .keys(&k_gpu)
        .values(&v_gpu)
        .preferred(&p_neg)
        .adjacency(&adjacency)
        .output(&output_neg)
        .kappa(kappa)
        .prepare()
        .unwrap();
    let mut encoder = backend.encoder(Some("rotor-attention-double-cover"));
    op_neg.encode(&mut encoder).unwrap();
    let submission = backend.submit(encoder);
    backend.wait(submission).unwrap();
    let negated = backend.read_tensor(&output_neg).unwrap();
    close(&negated, &expected);
    assert_eq!(
        actual
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>(),
        negated
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>(),
        "quaternion double-cover sign must be bit-invariant"
    );
}

struct SpikeShape {
    edges: usize,
    arity: usize,
    hidden: usize,
    bins: usize,
    kappa: f32,
}

fn spike_reference(x: &[f32], centres: &[f32], vertices: &[u32], shape: SpikeShape) -> Vec<f32> {
    let mut out = vec![0.0; shape.edges * shape.bins];
    for edge in 0..shape.edges {
        let mut energy = vec![0.0; shape.bins];
        for member in 0..shape.arity {
            let vertex = vertices[edge * shape.arity + member] as usize;
            let gx = x[vertex * shape.hidden] - centres[edge * shape.hidden];
            let gy = x[vertex * shape.hidden + 1] - centres[edge * shape.hidden + 1];
            let magnitude = (gx * gx + gy * gy).sqrt();
            if magnitude < 1.0e-6 {
                continue;
            }
            let theta = gy.atan2(gx);
            for (bin, value) in energy.iter_mut().enumerate() {
                let mu = std::f32::consts::PI * bin as f32 / shape.bins as f32;
                let phi = 2.0 * theta - 2.0 * mu;
                *value += magnitude * (shape.kappa * (phi.cos() - 1.0)).exp();
            }
        }
        let partition = 1.0e-6 + energy.iter().sum::<f32>();
        for bin in 0..shape.bins {
            out[edge * shape.bins + bin] = energy[bin] / partition;
        }
    }
    out
}

#[test]
fn star_rotor_spike_matches_the_predeclared_reference() {
    let Ok(backend) = WgpuBackend::new() else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let x = vec![
        1.0, 0.2, 0.1, 0.0, -0.4, 0.9, 0.2, -0.1, 0.6, -0.8, 0.0, 0.3, -0.7, -0.5, 0.4, 0.2,
    ];
    let centres = vec![0.1, -0.2, 0.0, 0.0, -0.2, 0.3, 0.1, -0.1];
    let vertices = vec![0u32, 1, 2, 2, 3, 1];
    let expected = spike_reference(
        &x,
        &centres,
        &vertices,
        SpikeShape {
            edges: 2,
            arity: 3,
            hidden: 4,
            bins: 8,
            kappa: 4.0,
        },
    );
    let resources = backend.resources();
    assert!(
        resources
            .upload_csr(
                "bad-adj",
                SparseCsrShape::new(2, 2, 2),
                &[0, 2, 1],
                &[0, 1],
                &[1.0; 2]
            )
            .is_err(),
        "malformed CSR must fail before a shader sees it"
    );
    let x_gpu = resources
        .upload_tensor("x", TensorShape::new(4, 4), &x)
        .unwrap();
    let c_gpu = resources
        .upload_tensor("c", TensorShape::new(2, 4), &centres)
        .unwrap();
    let v_gpu = resources
        .upload_u32_tensor("v", TensorShape::new(2, 3), &vertices)
        .unwrap();
    let output = resources
        .zeroed_tensor("spike", TensorShape::new(2, 8))
        .unwrap();
    let op = backend
        .operations()
        .star_rotor_spike()
        .features(&x_gpu)
        .centres(&c_gpu)
        .vertices(&v_gpu)
        .output(&output)
        .projection(0, 1)
        .kappa(4.0)
        .prepare()
        .unwrap();
    assert_eq!(op.kernel(), KernelId::StarRotorSpike);
    let mut encoder = backend.encoder(Some("star-rotor-spike-test"));
    op.encode(&mut encoder).unwrap();
    let submission = backend.submit(encoder);
    backend.wait(submission).unwrap();
    close(&backend.read_tensor(&output).unwrap(), &expected);
}

#[test]
fn invalid_restricted_shapes_are_rejected_before_dispatch() {
    let Ok(backend) = WgpuBackend::new() else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let resources = backend.resources();
    let rows = resources
        .zeroed_tensor("rows", TensorShape::new(2, 4))
        .unwrap();
    let bad_preferred = resources
        .zeroed_tensor("bad-p", TensorShape::new(2, 4))
        .unwrap();
    let adjacency = resources
        .upload_csr(
            "adj",
            SparseCsrShape::new(2, 2, 2),
            &[0, 1, 2],
            &[1, 0],
            &[1.0; 2],
        )
        .unwrap();
    let result = backend
        .operations()
        .rotor_attention()
        .queries(&rows)
        .keys(&rows)
        .values(&rows)
        .preferred(&bad_preferred)
        .adjacency(&adjacency)
        .output(&rows)
        .prepare();
    assert!(result.is_err());

    let vertices = resources
        .upload_u32_tensor("v", TensorShape::new(2, 1), &[0, 1])
        .unwrap();
    let result = backend
        .operations()
        .star_rotor_spike()
        .features(&rows)
        .centres(&rows)
        .vertices(&vertices)
        .output(&rows)
        .projection(0, 4)
        .prepare();
    assert!(result.is_err());
}
