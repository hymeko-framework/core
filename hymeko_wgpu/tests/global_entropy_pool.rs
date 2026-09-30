use std::f32::consts::LN_2;

use hymeko_wgpu::testing::shared_backend;
use hymeko_wgpu::{KernelId, TensorShape};

const ABS_BOUND: f32 = 2.0e-5;
const REL_BOUND: f32 = 2.0e-4;

fn close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    assert!(
        expected.iter().any(|value| value.abs() > 1.0e-6),
        "the comparison fixture must be non-vacuous"
    );
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

#[test]
fn blank_impulse_and_corner_channels_match_the_analytical_contract() {
    let Some(backend) = shared_backend("suite") else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let mut response = vec![0.0f32; 3 * 4 * 4];
    response[22] = 1.0;
    for index in [32, 35, 44, 47] {
        response[index] = 1.0;
    }
    let expected = vec![
        0.0,
        0.0,
        0.0,
        1.0 / 16.0,
        1.0e-6,
        0.0,
        4.0 / 16.0,
        0.5,
        LN_2,
    ];

    let resources = backend.resources();
    let response_gpu = resources
        .upload_tensor("response", TensorShape::new(3, 16), &response)
        .unwrap();
    let output = resources
        .zeroed_tensor("features", TensorShape::new(3, 3))
        .unwrap();
    let operation = backend
        .operations()
        .global_entropy_pool()
        .response(&response_gpu)
        .output(&output)
        .spatial_shape(4, 4)
        .prepare()
        .unwrap();
    assert_eq!(operation.kernel(), KernelId::GlobalEntropyPool);
    assert_eq!(operation.elements(), expected.len());

    let mut encoder = backend.encoder(Some("global-entropy-analytical"));
    operation.encode(&mut encoder).unwrap();
    let submission = backend.submit(encoder);
    backend.wait(submission).unwrap();
    let actual = backend.read_tensor(&output).unwrap();
    eprintln!("adapter={}", backend.capabilities().summary());
    close(&actual, &expected);
    assert_eq!(&actual[..3], &[0.0; 3], "blank channel must be exact zero");
}

#[test]
fn spatial_and_output_shape_defects_are_rejected_before_dispatch() {
    let Some(backend) = shared_backend("suite") else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let resources = backend.resources();
    let response = resources
        .zeroed_tensor("response", TensorShape::new(2, 16))
        .unwrap();
    let wrong_response = resources
        .zeroed_tensor("wrong-response", TensorShape::new(2, 15))
        .unwrap();
    let output = resources
        .zeroed_tensor("features", TensorShape::new(2, 3))
        .unwrap();
    let wrong_output = resources
        .zeroed_tensor("wrong-features", TensorShape::new(3, 2))
        .unwrap();

    assert!(
        backend
            .operations()
            .global_entropy_pool()
            .response(&response)
            .output(&output)
            .prepare()
            .is_err(),
        "missing canonical spatial shape must be rejected"
    );
    assert!(
        backend
            .operations()
            .global_entropy_pool()
            .response(&wrong_response)
            .output(&output)
            .spatial_shape(4, 4)
            .prepare()
            .is_err(),
        "flattened storage inconsistent with K,H,W must be rejected"
    );
    assert!(
        backend
            .operations()
            .global_entropy_pool()
            .response(&response)
            .output(&wrong_output)
            .spatial_shape(4, 4)
            .prepare()
            .is_err(),
        "output must be K by three"
    );
    assert!(
        backend
            .operations()
            .global_entropy_pool()
            .response(&response)
            .output(&output)
            .spatial_shape(4, 4)
            .workgroup(0)
            .prepare()
            .is_err(),
        "a zero workgroup must be rejected"
    );

    let alias = resources
        .zeroed_tensor("alias", TensorShape::new(1, 3))
        .unwrap();
    assert!(
        backend
            .operations()
            .global_entropy_pool()
            .response(&alias)
            .output(&alias)
            .spatial_shape(1, 3)
            .prepare()
            .is_err(),
        "read/write aliasing must be rejected before binding"
    );
}
