//! Phase 3 correctness: elementwise activation, and SpMM → activation chaining.
//!
//! # Reference
//!
//! The oracle is `cpu_activation` below — written independently in this file
//! rather than reused from a library, so a shared bug cannot hide.
//!
//! # NaN / Inf / signed-zero policy
//!
//! Defined per kind and asserted **structurally**, never absorbed by a
//! tolerance:
//!
//! | kind | NaN | −0.0 | −Inf | +Inf |
//! |---|---|---|---|---|
//! | Identity | NaN | −0.0 | −Inf | +Inf |
//! | Relu | +0.0 | +0.0 | +0.0 | +Inf |
//! | LeakyRelu(α>0) | NaN | −0.0 | −Inf | +Inf |
//! | Sigmoid | NaN | 0.5 | 0.0 | 1.0 |
//! | Tanh | NaN | −0.0 | −1.0 | 1.0 |
//!
//! Relu maps NaN to +0.0 because `NaN > 0.0` is false — the same result the CPU
//! convolution layers produce with `if val > zero { val } else { zero }`. That
//! is a deliberate contract, not an accident of the shading language.

use hymeko::tensor::compute::{ComputeError, TensorShape};
use hymeko::tensor::representations::tensor_csr::TensorCsr;
use hymeko_hnn::workloads::SparseSpec;
use hymeko_wgpu::testing::backend_or_skip;
use hymeko_wgpu::{ActivationKind, GpuTensor, PreparedActivation, WgpuBackend};

const POISON: f32 = -98765.5;

/// Independent CPU reference.
fn cpu_activation(kind: ActivationKind, v: f32) -> f32 {
    match kind {
        ActivationKind::Identity => v,
        ActivationKind::Relu => {
            if v > 0.0 {
                v
            } else {
                0.0
            }
        }
        ActivationKind::LeakyRelu { alpha } => {
            if v > 0.0 {
                v
            } else {
                alpha * v
            }
        }
        ActivationKind::Sigmoid => 1.0 / (1.0 + (-v).exp()),
        ActivationKind::Tanh => v.tanh(),
    }
}

/// Is `gpu` an acceptable flush-to-zero of the subnormal `cpu` result?
///
/// GPUs routinely flush subnormal f32 results to zero, and WGSL does not
/// guarantee subnormal support — unlike the CPU, which is IEEE-754 compliant
/// here. Measured on this adapter: `LeakyRelu(alpha=0.01)` applied to
/// `-f32::MIN_POSITIVE` yields `-1.1755e-40` on the CPU (a subnormal) and
/// exactly `-0.0` on the GPU.
///
/// This is admitted as a **named, narrow exception**, not by widening the
/// tolerance: the GPU must produce a zero *of the correct sign*, and only where
/// the CPU result was subnormal. Anything else still fails.
fn subnormal_flushed_to_zero(gpu: f32, cpu: f32) -> bool {
    cpu.is_subnormal() && gpu == 0.0 && gpu.is_sign_negative() == cpu.is_sign_negative()
}

/// Structural comparison: NaN/Inf must match exactly; finite values within
/// tolerance. Exact kinds additionally require bit equality, modulo the
/// subnormal flush-to-zero exception above.
fn assert_activation(got: &[f32], input: &[f32], kind: ActivationKind, ctx: &str) {
    let (abs_tol, rel_tol) = if kind.is_transcendental() {
        // exp / tanh are bounded-precision intrinsics, not exact operations.
        (1e-6f32, 1e-5f32)
    } else {
        (0.0, 0.0)
    };

    assert_eq!(got.len(), input.len(), "{ctx}: length mismatch");

    for (i, (&g, &v)) in got.iter().zip(input.iter()).enumerate() {
        let want = cpu_activation(kind, v);

        if want.is_nan() || g.is_nan() {
            assert!(
                want.is_nan() && g.is_nan(),
                "{ctx}: NaN disagreement at {i}: input={v} gpu={g} cpu={want}"
            );
            continue;
        }
        if want.is_infinite() || g.is_infinite() {
            assert!(
                want.is_infinite() && g.is_infinite() && want.signum() == g.signum(),
                "{ctx}: infinity disagreement at {i}: input={v} gpu={g} cpu={want}"
            );
            continue;
        }

        if !kind.is_transcendental() {
            // Exact kinds: bit equality, which also pins the sign of zero.
            // The one admitted exception is a subnormal result the GPU flushed.
            if subnormal_flushed_to_zero(g, want) {
                continue;
            }
            assert_eq!(
                g.to_bits(),
                want.to_bits(),
                "{ctx}: at {i} input={v} gpu={g} ({:#x}) cpu={want} ({:#x})",
                g.to_bits(),
                want.to_bits()
            );
        } else {
            let abs = (g - want).abs();
            let rel = if want.abs() > 0.0 {
                abs / want.abs()
            } else {
                abs
            };
            assert!(
                abs <= abs_tol || rel <= rel_tol,
                "{ctx}: at {i} input={v} gpu={g} cpu={want} abs={abs:e} rel={rel:e}"
            );
        }
    }
}

/// Run an activation out-of-place and read back.
fn run_activation(
    backend: &WgpuBackend,
    kind: ActivationKind,
    data: &[f32],
    workgroup: u32,
) -> Vec<f32> {
    let res = backend.resources();
    let shape = TensorShape::new(data.len(), 1);
    let input = res.upload_tensor("in", shape, data).expect("upload");
    let poison = vec![POISON; data.len()];
    let output = res.upload_tensor("out", shape, &poison).expect("alloc");

    let op = backend
        .operations()
        .activation()
        .kind(kind)
        .input(&input)
        .output(&output)
        .workgroup(workgroup)
        .prepare()
        .expect("prepare");
    assert!(
        !op.is_in_place(),
        "distinct buffers must select out-of-place"
    );

    let mut enc = backend.encoder(Some("act"));
    op.encode(&mut enc).expect("encode");
    backend.wait(backend.submit(enc)).expect("submit");
    backend.read_tensor(&output).expect("readback")
}

/// Values chosen to hit every clause of the numerical contract.
fn adversarial_input() -> Vec<f32> {
    vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.5,
        -0.5,
        f32::MIN_POSITIVE,
        -f32::MIN_POSITIVE,
        1e-30,
        -1e-30,
        3.4e38,
        -3.4e38,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
        123.456,
        -123.456,
    ]
}

fn kinds() -> Vec<ActivationKind> {
    vec![
        ActivationKind::Identity,
        ActivationKind::Relu,
        ActivationKind::LeakyRelu { alpha: 0.01 },
        ActivationKind::LeakyRelu { alpha: 0.2 },
        ActivationKind::LeakyRelu { alpha: 0.0 },
        ActivationKind::Sigmoid,
        ActivationKind::Tanh,
    ]
}

// ---------------------------------------------------------------------------
// numerical contract
// ---------------------------------------------------------------------------

#[test]
fn test_all_kinds_match_cpu_on_adversarial_values() {
    let Some(backend) = backend_or_skip("kinds") else {
        return;
    };
    let input = adversarial_input();

    for kind in kinds() {
        let got = run_activation(&backend, kind, &input, 64);
        assert_activation(&got, &input, kind, &format!("{kind}"));
    }
}

#[test]
fn test_signed_zero_and_nan_contract_explicitly() {
    let Some(backend) = backend_or_skip("signed_zero") else {
        return;
    };
    let input = vec![-0.0f32, 0.0, f32::NAN, f32::NEG_INFINITY, f32::INFINITY];

    // Identity preserves -0.0 exactly.
    let id = run_activation(&backend, ActivationKind::Identity, &input, 64);
    assert!(
        id[0].is_sign_negative() && id[0] == 0.0,
        "identity lost -0.0"
    );
    assert!(id[2].is_nan(), "identity lost NaN");

    // ReLU collapses -0.0 and NaN to +0.0, and -Inf to +0.0.
    let relu = run_activation(&backend, ActivationKind::Relu, &input, 64);
    assert_eq!(
        relu[0].to_bits(),
        0.0f32.to_bits(),
        "relu(-0.0) must be +0.0"
    );
    assert_eq!(
        relu[1].to_bits(),
        0.0f32.to_bits(),
        "relu(+0.0) must be +0.0"
    );
    assert_eq!(
        relu[2].to_bits(),
        0.0f32.to_bits(),
        "relu(NaN) must be +0.0"
    );
    assert_eq!(
        relu[3].to_bits(),
        0.0f32.to_bits(),
        "relu(-Inf) must be +0.0"
    );
    assert!(
        relu[4].is_infinite() && relu[4] > 0.0,
        "relu(+Inf) must be +Inf"
    );

    // LeakyReLU keeps the sign of zero and propagates NaN.
    let leaky = run_activation(
        &backend,
        ActivationKind::LeakyRelu { alpha: 0.25 },
        &input,
        64,
    );
    assert!(
        leaky[0].is_sign_negative() && leaky[0] == 0.0,
        "leaky_relu(-0.0) must stay -0.0 for alpha > 0, got {}",
        leaky[0]
    );
    assert!(leaky[2].is_nan(), "leaky_relu(NaN) must be NaN");
    assert!(
        leaky[3].is_infinite() && leaky[3] < 0.0,
        "leaky_relu(-Inf) must be -Inf for alpha > 0"
    );
}

#[test]
fn test_subnormal_handling_is_characterised_not_assumed() {
    // Pins what this adapter actually does with subnormals, so the exception in
    // `subnormal_flushed_to_zero` rests on a measurement rather than folklore.
    let Some(backend) = backend_or_skip("subnormal") else {
        return;
    };

    let smallest_normal = f32::MIN_POSITIVE; // 1.1754944e-38
    let subnormal = f32::from_bits(0x0000_0001); // 1.4e-45, smallest subnormal
    let input = vec![smallest_normal, -smallest_normal, subnormal, -subnormal];

    // Identity: does the GPU preserve a subnormal it merely copies?
    let id = run_activation(&backend, ActivationKind::Identity, &input, 64);
    eprintln!(
        "[subnormal] identity: in={:?} out={:?}",
        input.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        id.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );

    // LeakyReLU(0.01) drives a normal input into the subnormal range.
    let kind = ActivationKind::LeakyRelu { alpha: 0.01 };
    let leaky = run_activation(&backend, kind, &input, 64);
    for (i, (&g, &v)) in leaky.iter().zip(input.iter()).enumerate() {
        let want = cpu_activation(kind, v);
        let ok = g.to_bits() == want.to_bits() || subnormal_flushed_to_zero(g, want);
        assert!(
            ok,
            "subnormal handling at {i}: input={v:e} gpu={g:e} ({:#x}) cpu={want:e} ({:#x}) \
             -- neither exact nor a correctly-signed flush",
            g.to_bits(),
            want.to_bits()
        );
        if want.is_subnormal() {
            eprintln!(
                "[subnormal] leaky at {i}: cpu={want:e} subnormal -> gpu={g:e} ({})",
                if g == 0.0 { "FLUSHED" } else { "preserved" }
            );
        }
    }

    // The sign of a flushed zero still matters and is not negotiable.
    let negative_subnormal_result = leaky[1]; // 0.01 * -MIN_POSITIVE
    assert!(
        negative_subnormal_result <= 0.0,
        "a negative subnormal must not flush to +0.0, got {negative_subnormal_result}"
    );
}

#[test]
fn test_empty_and_singleton() {
    let Some(backend) = backend_or_skip("empty") else {
        return;
    };
    let res = backend.resources();

    // Empty: no dispatch may be issued.
    let empty_in = res.upload_tensor("e", TensorShape::new(0, 4), &[]).unwrap();
    let empty_out = res.zeroed_tensor("eo", TensorShape::new(0, 4)).unwrap();
    let op = backend
        .operations()
        .activation()
        .kind(ActivationKind::Relu)
        .input(&empty_in)
        .output(&empty_out)
        .prepare()
        .expect("empty must prepare");
    assert_eq!(op.dispatch_groups(), 0, "empty must not dispatch");
    let mut enc = backend.encoder(None);
    op.encode(&mut enc).unwrap();
    backend.wait(backend.submit(enc)).unwrap();
    assert!(backend.read_tensor(&empty_out).unwrap().is_empty());

    // Singleton.
    let got = run_activation(&backend, ActivationKind::Relu, &[-3.0], 64);
    assert_eq!(got, vec![0.0]);
    let got = run_activation(&backend, ActivationKind::Relu, &[3.0], 64);
    assert_eq!(got, vec![3.0]);
}

#[test]
fn test_lengths_not_divisible_by_workgroup() {
    let Some(backend) = backend_or_skip("ragged") else {
        return;
    };
    // Deliberately coprime with 64 / 128 / 256.
    for len in [1usize, 63, 65, 127, 129, 255, 257, 1000, 4097] {
        let input: Vec<f32> = (0..len).map(|i| (i as f32) - (len as f32) / 2.0).collect();
        for wg in [64u32, 128, 256] {
            let got = run_activation(&backend, ActivationKind::Relu, &input, wg);
            assert_activation(
                &got,
                &input,
                ActivationKind::Relu,
                &format!("len={len} wg={wg}"),
            );
        }
    }
}

#[test]
fn test_all_workgroups_agree() {
    let Some(backend) = backend_or_skip("workgroups") else {
        return;
    };
    let input: Vec<f32> = (0..5000).map(|i| ((i % 97) as f32) - 48.0).collect();
    let reference = run_activation(&backend, ActivationKind::Relu, &input, 64);
    for wg in [128u32, 256] {
        let got = run_activation(&backend, ActivationKind::Relu, &input, wg);
        assert_eq!(got, reference, "workgroup {wg} disagreed with 64");
    }
}

// ---------------------------------------------------------------------------
// validation
// ---------------------------------------------------------------------------

#[test]
fn test_shape_mismatch_and_missing_operands_rejected() {
    let Some(backend) = backend_or_skip("validation") else {
        return;
    };
    let res = backend.resources();
    let a = res
        .upload_tensor("a", TensorShape::new(4, 2), &[1.0; 8])
        .unwrap();
    let b = res.zeroed_tensor("b", TensorShape::new(4, 2)).unwrap();
    let wrong = res.zeroed_tensor("w", TensorShape::new(2, 4)).unwrap();

    assert!(
        backend
            .operations()
            .activation()
            .input(&a)
            .output(&b)
            .prepare()
            .is_ok()
    );

    // Same element count, different shape -> still rejected.
    assert!(matches!(
        backend
            .operations()
            .activation()
            .input(&a)
            .output(&wrong)
            .prepare(),
        Err(ComputeError::ShapeMismatch { .. })
    ));

    assert!(
        backend
            .operations()
            .activation()
            .output(&b)
            .prepare()
            .is_err()
    );
    assert!(
        backend
            .operations()
            .activation()
            .input(&a)
            .prepare()
            .is_err()
    );

    // Oversized workgroup.
    let limit = backend.capabilities().max_compute_invocations_per_workgroup;
    let err = backend
        .operations()
        .activation()
        .input(&a)
        .output(&b)
        .workgroup(limit * 2)
        .prepare()
        .unwrap_err();
    assert!(format!("{err}").contains("workgroup"), "got: {err}");

    // Zero workgroup.
    assert!(
        backend
            .operations()
            .activation()
            .input(&a)
            .output(&b)
            .workgroup(0)
            .prepare()
            .is_err()
    );
}

// ---------------------------------------------------------------------------
// in-place
// ---------------------------------------------------------------------------

#[test]
fn test_in_place_activation() {
    // The phase brief asks whether in-place is legal. It is -- but only with a
    // SINGLE read_write binding. Binding one buffer as both the read and the
    // read_write operand of the out-of-place shader violates WebGPU's
    // usage-scope rule, so the builder selects a dedicated single-binding
    // pipeline when input and output are the same buffer.
    let Some(backend) = backend_or_skip("in_place") else {
        return;
    };
    let res = backend.resources();
    let data: Vec<f32> = (0..1000).map(|i| (i as f32) - 500.0).collect();
    let shape = TensorShape::new(data.len(), 1);
    let tensor = res.upload_tensor("t", shape, &data).unwrap();

    let op = backend
        .operations()
        .activation()
        .kind(ActivationKind::Relu)
        .in_place(&tensor)
        .prepare()
        .expect("in-place must prepare");
    assert!(
        op.is_in_place(),
        "same buffer must select the in-place path"
    );

    let mut enc = backend.encoder(Some("inplace"));
    op.encode(&mut enc).unwrap();
    backend.wait(backend.submit(enc)).unwrap();

    let got = backend.read_tensor(&tensor).unwrap();
    assert_activation(&got, &data, ActivationKind::Relu, "in-place relu");

    // Idempotent: ReLU applied twice equals ReLU applied once.
    let mut enc = backend.encoder(None);
    op.encode(&mut enc).unwrap();
    backend.wait(backend.submit(enc)).unwrap();
    assert_eq!(backend.read_tensor(&tensor).unwrap(), got);
}

#[test]
fn test_in_place_and_out_of_place_agree() {
    let Some(backend) = backend_or_skip("inplace_vs_out") else {
        return;
    };
    let res = backend.resources();
    let data = adversarial_input();
    let shape = TensorShape::new(data.len(), 1);

    for kind in kinds() {
        let out_of_place = run_activation(&backend, kind, &data, 64);

        let t = res.upload_tensor("t", shape, &data).unwrap();
        let op = backend
            .operations()
            .activation()
            .kind(kind)
            .in_place(&t)
            .workgroup(64)
            .prepare()
            .unwrap();
        let mut enc = backend.encoder(None);
        op.encode(&mut enc).unwrap();
        backend.wait(backend.submit(enc)).unwrap();
        let in_place = backend.read_tensor(&t).unwrap();

        // Bit-for-bit: the two shaders must not drift apart.
        for i in 0..in_place.len() {
            assert_eq!(
                in_place[i].to_bits(),
                out_of_place[i].to_bits(),
                "{kind}: in-place and out-of-place differ at {i}: {} vs {}",
                in_place[i],
                out_of_place[i]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// reuse, batching, cache
// ---------------------------------------------------------------------------

#[test]
fn test_repeated_dispatch_input_rewrite_and_poison() {
    let Some(backend) = backend_or_skip("reuse") else {
        return;
    };
    let res = backend.resources();
    let d1: Vec<f32> = (0..256).map(|i| (i as f32) - 128.0).collect();
    let shape = TensorShape::new(d1.len(), 1);
    let input = res.upload_tensor("in", shape, &d1).unwrap();
    let output = res
        .upload_tensor("out", shape, &vec![POISON; d1.len()])
        .unwrap();

    let op = backend
        .operations()
        .activation()
        .kind(ActivationKind::LeakyRelu { alpha: 0.1 })
        .input(&input)
        .output(&output)
        .prepare()
        .unwrap();

    let mut enc = backend.encoder(None);
    op.encode(&mut enc).unwrap();
    backend.wait(backend.submit(enc)).unwrap();
    let first = backend.read_tensor(&output).unwrap();
    assert!(first.iter().all(|v| *v != POISON), "poison survived");
    assert_activation(
        &first,
        &d1,
        ActivationKind::LeakyRelu { alpha: 0.1 },
        "run 1",
    );

    // Re-dispatch is idempotent.
    let mut enc = backend.encoder(None);
    op.encode(&mut enc).unwrap();
    backend.wait(backend.submit(enc)).unwrap();
    assert_eq!(backend.read_tensor(&output).unwrap(), first);

    // Rewrite the input in place; the same prepared op sees the new data.
    let d2: Vec<f32> = d1.iter().map(|v| -v).collect();
    res.write_tensor(&input, &d2).unwrap();
    let mut enc = backend.encoder(None);
    op.encode(&mut enc).unwrap();
    backend.wait(backend.submit(enc)).unwrap();
    assert_activation(
        &backend.read_tensor(&output).unwrap(),
        &d2,
        ActivationKind::LeakyRelu { alpha: 0.1 },
        "after rewrite",
    );
}

#[test]
fn test_two_activations_one_encoder() {
    let Some(backend) = backend_or_skip("two_acts") else {
        return;
    };
    let res = backend.resources();
    let data: Vec<f32> = (0..512).map(|i| (i as f32) - 256.0).collect();
    let shape = TensorShape::new(data.len(), 1);

    let a_in = res.upload_tensor("a", shape, &data).unwrap();
    let a_out = res.zeroed_tensor("ao", shape).unwrap();
    let b_out = res.zeroed_tensor("bo", shape).unwrap();

    let relu = backend
        .operations()
        .activation()
        .kind(ActivationKind::Relu)
        .input(&a_in)
        .output(&a_out)
        .prepare()
        .unwrap();
    // Chained: the second reads what the first wrote, in the same encoder.
    let tanh = backend
        .operations()
        .activation()
        .kind(ActivationKind::Tanh)
        .input(&a_out)
        .output(&b_out)
        .prepare()
        .unwrap();

    let mut enc = backend.encoder(Some("two-acts"));
    relu.encode(&mut enc).unwrap();
    tanh.encode(&mut enc).unwrap();
    backend.wait(backend.submit(enc)).unwrap();

    let mid = backend.read_tensor(&a_out).unwrap();
    assert_activation(&mid, &data, ActivationKind::Relu, "chain step 1");
    let end = backend.read_tensor(&b_out).unwrap();
    assert_activation(&end, &mid, ActivationKind::Tanh, "chain step 2");
}

#[test]
fn test_pipeline_cache_reuse_across_kinds_and_workgroups() {
    let Some(backend) = backend_or_skip("cache") else {
        return;
    };
    let res = backend.resources();
    let shape = TensorShape::new(64, 1);
    let a = res.upload_tensor("a", shape, &[1.0; 64]).unwrap();
    let b = res.zeroed_tensor("b", shape).unwrap();
    let c = res.zeroed_tensor("c", shape).unwrap();

    assert!(backend.pipelines().is_empty());

    // A function rather than a closure: `PreparedActivation<'a>` borrows its
    // operands (so the arena cannot recycle a buffer an operation still uses),
    // and a closure cannot express that its return value borrows its argument.
    fn mk<'a>(
        backend: &'a WgpuBackend,
        input: &'a GpuTensor<f32>,
        kind: ActivationKind,
        out: &'a GpuTensor<f32>,
        wg: u32,
    ) -> PreparedActivation<'a> {
        backend
            .operations()
            .activation()
            .kind(kind)
            .input(input)
            .output(out)
            .workgroup(wg)
            .prepare()
            .unwrap()
    }

    let _relu_b_64 = mk(&backend, &a, ActivationKind::Relu, &b, 64);
    assert_eq!(backend.pipelines().len(), 1);

    // Same kind + workgroup, different buffers -> cache hit.
    let _relu_c_64 = mk(&backend, &a, ActivationKind::Relu, &c, 64);
    assert_eq!(
        backend.pipelines().len(),
        1,
        "buffers are not part of the key"
    );

    // Different kind -> different entry point -> new pipeline (same module).
    let _tanh_b_64 = mk(&backend, &a, ActivationKind::Tanh, &b, 64);
    assert_eq!(backend.pipelines().len(), 2);

    // Different workgroup -> different module -> new pipeline.
    let _relu_b_128 = mk(&backend, &a, ActivationKind::Relu, &b, 128);
    assert_eq!(backend.pipelines().len(), 3);

    // LeakyRelu alpha is a uniform value, NOT a specialisation: changing it
    // must not create a pipeline.
    let before = backend.pipelines().len();
    let _leaky_001 = mk(
        &backend,
        &a,
        ActivationKind::LeakyRelu { alpha: 0.01 },
        &b,
        64,
    );
    let _leaky_05 = mk(
        &backend,
        &a,
        ActivationKind::LeakyRelu { alpha: 0.5 },
        &c,
        64,
    );
    assert_eq!(
        backend.pipelines().len(),
        before + 1,
        "alpha must be a uniform, not a pipeline variant"
    );
}

// ---------------------------------------------------------------------------
// the point of Phase 3: SpMM -> activation, one encoder, one submit
// ---------------------------------------------------------------------------

fn handmade() -> TensorCsr<f32> {
    TensorCsr {
        num_rows: 4,
        num_cols: 5,
        row_ptr: vec![0, 2, 5, 5, 8],
        col_ind: vec![0, 3, 1, 2, 4, 4, 0, 4],
        val: vec![1.5, -2.0, 0.25, 3.0, 1.0, 2.0, -1.0, 0.5],
    }
}

#[test]
fn test_spmm_then_activation_in_one_encoder() {
    let Some(backend) = backend_or_skip("chain") else {
        return;
    };
    let res = backend.resources();
    let csr = handmade();
    let fd = 8;

    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x: Vec<f32> = (0..csr.num_cols * fd)
        .map(|t| (t as f32) * 0.37 - 5.0)
        .collect();
    let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
    let intermediate = res
        .upload_tensor(
            "mid",
            shape.output_shape(fd),
            &vec![POISON; csr.num_rows * fd],
        )
        .unwrap();
    let activated = res
        .upload_tensor(
            "act",
            shape.output_shape(fd),
            &vec![POISON; csr.num_rows * fd],
        )
        .unwrap();

    let spmm = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&intermediate)
        .prepare()
        .unwrap();
    let activation = backend
        .operations()
        .activation()
        .kind(ActivationKind::Relu)
        .input(&intermediate)
        .output(&activated)
        .prepare()
        .unwrap();

    // ONE encoder, ONE submit, intermediate never leaves the device.
    let mut encoder = backend.encoder(Some("spmm-activation"));
    spmm.encode(&mut encoder).unwrap();
    activation.encode(&mut encoder).unwrap();
    backend.wait(backend.submit(encoder)).unwrap();

    // CPU reference for the whole chain.
    let mut cpu_mid = vec![0.0f32; csr.num_rows * fd];
    csr.spmm_into_serial(&x, fd, &mut cpu_mid);
    let cpu_end: Vec<f32> = cpu_mid
        .iter()
        .map(|&v| cpu_activation(ActivationKind::Relu, v))
        .collect();

    let gpu_end = backend.read_tensor(&activated).unwrap();
    assert_eq!(gpu_end.len(), cpu_end.len());
    for i in 0..cpu_end.len() {
        let (g, c) = (gpu_end[i], cpu_end[i]);
        let abs = (g - c).abs();
        let rel = if c.abs() > 0.0 { abs / c.abs() } else { abs };
        assert!(
            abs <= 1e-5 || rel <= 1e-5,
            "chain mismatch at {i}: gpu={g} cpu={c}"
        );
    }
    // ReLU output must be non-negative, and the intermediate must have had
    // negatives for the test to mean anything.
    assert!(gpu_end.iter().all(|v| *v >= 0.0));
    assert!(
        cpu_mid.iter().any(|v| *v < 0.0),
        "fixture produced no negative intermediate; the ReLU is untested"
    );
}

#[test]
fn test_spmm_then_in_place_activation() {
    // The pattern that matters for a real forward pass: activate the SpMM
    // output in place, so no third tensor is allocated.
    let Some(backend) = backend_or_skip("chain_inplace") else {
        return;
    };
    let res = backend.resources();
    let spec = SparseSpec::small();
    let csr = spec.build();
    let fd = 8;

    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x = spec.dense_block(fd);
    let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
    let out = res
        .upload_tensor(
            "y",
            shape.output_shape(fd),
            &vec![POISON; csr.num_rows * fd],
        )
        .unwrap();

    let spmm = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&out)
        .prepare()
        .unwrap();
    let activation = backend
        .operations()
        .activation()
        .kind(ActivationKind::LeakyRelu { alpha: 0.01 })
        .in_place(&out)
        .prepare()
        .unwrap();
    assert!(activation.is_in_place());

    let mut encoder = backend.encoder(Some("spmm-inplace-act"));
    spmm.encode(&mut encoder).unwrap();
    activation.encode(&mut encoder).unwrap();
    backend.wait(backend.submit(encoder)).unwrap();

    let mut cpu = vec![0.0f32; csr.num_rows * fd];
    csr.spmm_into_serial(&x, fd, &mut cpu);
    let cpu: Vec<f32> = cpu
        .iter()
        .map(|&v| cpu_activation(ActivationKind::LeakyRelu { alpha: 0.01 }, v))
        .collect();

    let gpu = backend.read_tensor(&out).unwrap();
    for i in 0..cpu.len() {
        let abs = (gpu[i] - cpu[i]).abs();
        let rel = if cpu[i].abs() > 0.0 {
            abs / cpu[i].abs()
        } else {
            abs
        };
        assert!(
            abs <= 1e-5 || rel <= 1e-5,
            "in-place chain mismatch at {i}: gpu={} cpu={}",
            gpu[i],
            cpu[i]
        );
    }
}

#[test]
fn test_two_spmm_activation_chains_one_submit() {
    let Some(backend) = backend_or_skip("two_chains") else {
        return;
    };
    let res = backend.resources();
    let csr = handmade();
    let fd = 4;
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();

    let mk_chain = |tag: &str, scale: f32| {
        let x: Vec<f32> = (0..csr.num_cols * fd)
            .map(|t| ((t as f32) - 6.0) * scale)
            .collect();
        let xin = res
            .upload_tensor(&format!("x{tag}"), shape.input_shape(fd), &x)
            .unwrap();
        let y = res
            .zeroed_tensor(&format!("y{tag}"), shape.output_shape(fd))
            .unwrap();
        (x, xin, y)
    };

    let (xa, in_a, out_a) = mk_chain("a", 1.0);
    let (xb, in_b, out_b) = mk_chain("b", -0.5);

    let s_a = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&in_a)
        .output(&out_a)
        .prepare()
        .unwrap();
    let a_a = backend
        .operations()
        .activation()
        .kind(ActivationKind::Relu)
        .in_place(&out_a)
        .prepare()
        .unwrap();
    let s_b = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&in_b)
        .output(&out_b)
        .prepare()
        .unwrap();
    let a_b = backend
        .operations()
        .activation()
        .kind(ActivationKind::Relu)
        .in_place(&out_b)
        .prepare()
        .unwrap();

    // Four dispatches, one encoder, one submit.
    let mut encoder = backend.encoder(Some("two-chains"));
    s_a.encode(&mut encoder).unwrap();
    a_a.encode(&mut encoder).unwrap();
    s_b.encode(&mut encoder).unwrap();
    a_b.encode(&mut encoder).unwrap();
    backend.wait(backend.submit(encoder)).unwrap();

    for (x, out, tag) in [(xa, &out_a, "a"), (xb, &out_b, "b")] {
        let mut cpu = vec![0.0f32; csr.num_rows * fd];
        csr.spmm_into_serial(&x, fd, &mut cpu);
        let cpu: Vec<f32> = cpu
            .iter()
            .map(|&v| cpu_activation(ActivationKind::Relu, v))
            .collect();
        let gpu = backend.read_tensor(out).unwrap();
        for i in 0..cpu.len() {
            assert!(
                (gpu[i] - cpu[i]).abs() <= 1e-5,
                "chain {tag} mismatch at {i}: gpu={} cpu={}",
                gpu[i],
                cpu[i]
            );
        }
    }
}

/// The grid-stride path: more elements than one dispatch grid can address.
///
/// A device caps workgroups per dimension at 65 535, so a 64-wide group reaches
/// only 4 194 240 elements in a single grid. Everything above that relies on
/// each invocation striding through the remainder — a path none of the other
/// tests touch, because they all fit in one grid.
///
/// This is the case that made `chain_bench` fail at 200k rows × 128 features
/// (25.6M elements), so it is asserted rather than assumed. The check is
/// elementwise against the CPU oracle: a stride that skipped or double-visited
/// an element shows up as a wrong value, not merely a wrong count.
#[test]
fn test_count_exceeding_one_dispatch_grid() {
    let Some(backend) = backend_or_skip("grid_stride") else {
        return;
    };
    // 65_535 * 64 = 4_194_240 is the single-grid ceiling at wg=64. Take a
    // length just past it, and coprime with all three workgroup sizes, so the
    // final partial iteration is ragged too.
    let len = 4_194_240usize + 4097;
    let input: Vec<f32> = (0..len).map(|i| ((i % 211) as f32) - 105.0).collect();

    let got = run_activation(&backend, ActivationKind::Relu, &input, 64);
    assert_eq!(got.len(), len, "grid-stride run returned the wrong length");
    assert_activation(&got, &input, ActivationKind::Relu, "grid-stride wg=64");

    // Same data through a grid that *does* fit (256-wide needs 16 384 groups):
    // the stride path must agree bit-for-bit with the single-pass path.
    let single_pass = run_activation(&backend, ActivationKind::Relu, &input, 256);
    assert_eq!(
        got, single_pass,
        "strided dispatch disagreed with a dispatch that fits in one grid"
    );
}

// ---------------------------------------------------------------------------
// cell H: the hand-written fused SpMM + activation kernel
// ---------------------------------------------------------------------------

/// The fused kernel must produce what the two-dispatch chain produces.
///
/// This is what keeps the duplicated accumulation loop in
/// `spmm_activation.wgsl` honest: if it drifts from `spmm.wgsl`, this fails
/// rather than silently returning different numbers in the fused path only.
///
/// Compared **exactly**, not with a tolerance: both kernels run the same
/// accumulation in the same order on the same hardware, so the only way they
/// can differ is if the code differs. A tolerance here would hide exactly the
/// drift the test exists to catch. (The CPU comparison elsewhere does need a
/// tolerance, because of FMA contraction — that is a different question.)
#[test]
fn test_fused_matches_unfused_chain() {
    let Some(backend) = backend_or_skip("fused") else {
        return;
    };
    let res = backend.resources();
    let csr = handmade();

    for fd in [1usize, 8, 32] {
        let matrix = res.upload_csr_from("a", &csr).unwrap();
        let shape = matrix.shape();
        let x: Vec<f32> = (0..csr.num_cols * fd)
            .map(|t| (t as f32) * 0.37 - 5.0)
            .collect();
        let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();

        for kind in [
            ActivationKind::Relu,
            ActivationKind::LeakyRelu { alpha: 0.01 },
            ActivationKind::Sigmoid,
            ActivationKind::Tanh,
        ] {
            // --- unfused: two dispatches, one encoder -----------------------
            let mid = res
                .upload_tensor(
                    "mid",
                    shape.output_shape(fd),
                    &vec![POISON; csr.num_rows * fd],
                )
                .unwrap();
            let spmm = backend
                .operations()
                .spmm()
                .matrix(&matrix)
                .input(&input)
                .output(&mid)
                .prepare()
                .unwrap();
            let act = backend
                .operations()
                .activation()
                .kind(kind)
                .in_place(&mid)
                .prepare()
                .unwrap();
            let mut enc = backend.encoder(Some("unfused"));
            spmm.encode(&mut enc).unwrap();
            act.encode(&mut enc).unwrap();
            backend.wait(backend.submit(enc)).unwrap();
            let unfused = backend.read_tensor(&mid).unwrap();

            // --- fused: one dispatch ----------------------------------------
            let out = res
                .upload_tensor(
                    "out",
                    shape.output_shape(fd),
                    &vec![POISON; csr.num_rows * fd],
                )
                .unwrap();
            let fused = backend
                .operations()
                .spmm()
                .matrix(&matrix)
                .input(&input)
                .output(&out)
                .fused_activation(kind)
                .prepare()
                .unwrap();
            let mut enc = backend.encoder(Some("fused"));
            fused.encode(&mut enc).unwrap();
            backend.wait(backend.submit(enc)).unwrap();
            let got = backend.read_tensor(&out).unwrap();

            assert_eq!(
                got.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                unfused.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                "fused {kind} at fd={fd} differs from the unfused chain"
            );
            // Guard against both paths being trivially zero.
            assert!(
                got.iter().any(|v| *v != 0.0),
                "fixture gave an all-zero result for {kind} at fd={fd}; nothing was tested"
            );
        }
    }
}

/// Fusing `Identity` is exactly the unfused kernel, so it is rejected rather
/// than quietly compiling a second pipeline with identical behaviour.
#[test]
fn test_fused_identity_is_rejected() {
    let Some(backend) = backend_or_skip("fused_identity") else {
        return;
    };
    let res = backend.resources();
    let csr = handmade();
    let fd = 4;
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x = vec![1.0f32; csr.num_cols * fd];
    let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
    let out = res.zeroed_tensor("y", shape.output_shape(fd)).unwrap();

    let err = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&out)
        .fused_activation(ActivationKind::Identity)
        .prepare()
        .unwrap_err();
    assert!(
        matches!(err, ComputeError::Backend { .. }),
        "expected a Backend error for fused Identity, got {err:?}"
    );
}

/// Fusion must not silently change which shape the workgroup rule picks, and
/// the fused kernel is a distinct pipeline from the unfused one.
#[test]
fn test_fused_is_a_distinct_pipeline() {
    let Some(backend) = backend_or_skip("fused_pipeline") else {
        return;
    };
    let res = backend.resources();
    let csr = handmade();
    let fd = 8;
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x = vec![1.0f32; csr.num_cols * fd];
    let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
    let out = res.zeroed_tensor("y", shape.output_shape(fd)).unwrap();

    let build = |fused: Option<ActivationKind>| {
        let b = backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&input)
            .output(&out);
        match fused {
            Some(k) => b.fused_activation(k),
            None => b,
        }
        .prepare()
        .unwrap()
    };

    let plain = build(None);
    let n_after_plain = backend.pipelines().len();
    let relu = build(Some(ActivationKind::Relu));
    assert_eq!(
        backend.pipelines().len(),
        n_after_plain + 1,
        "the fused kernel must be its own pipeline"
    );
    // Same workgroup shape either way: fusion is not a workgroup decision.
    assert_eq!(plain.workgroup(), relu.workgroup());
}
