//! Phase 1 skeleton tests: device acquisition, resource upload, pipeline
//! caching, and the encode → submit → readback boundary.
//!
//! # Adapter policy
//!
//! A missing GPU adapter is an **environment** fact and is reported as a skip.
//! Anything else — a shader that fails to compile, a readback that returns the
//! wrong data, a pipeline that is rebuilt when it should have been cached — is
//! a **defect** and fails the test. That asymmetry is deliberate: it is the
//! difference between "this machine has no GPU" and "the backend is broken",
//! and collapsing the two would let real regressions pass silently in CI.

use hymeko::tensor::compute::{ComputeError, SparseCsrShape, TensorShape};
use hymeko_wgpu::{FILL_SHADER, PipelineKey, WgpuBackend};

/// Acquire a backend, or return `None` if this environment has no adapter.
///
/// Only [`ComputeError::BackendUnavailable`] is treated as skippable.
fn backend_or_skip(test: &str) -> Option<WgpuBackend> {
    match WgpuBackend::builder().label("hymeko-test").build() {
        Ok(b) => {
            eprintln!("[{test}] adapter: {}", b.capabilities().summary());
            Some(b)
        }
        Err(ComputeError::BackendUnavailable { reason, .. }) => {
            eprintln!("[{test}] SKIPPED - no GPU adapter available: {reason}");
            None
        }
        Err(other) => panic!("[{test}] backend init failed (not an availability problem): {other}"),
    }
}

/// Bind group layout for `fill.wgsl`: one read-write storage buffer + params.
fn fill_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("fill.layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

/// Pipeline layout for `fill.wgsl`.
///
/// wgpu 30 takes `Option<&BindGroupLayout>` per slot and replaced push
/// constants with `immediate_size`.
fn fill_pipeline_layout(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::PipelineLayout {
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("fill.pipeline_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    })
}

#[test]
fn test_backend_reports_capabilities() {
    let Some(backend) = backend_or_skip("capabilities") else {
        return;
    };
    let caps = backend.capabilities();

    assert!(
        !caps.adapter_name.is_empty(),
        "adapter name must not be empty"
    );
    assert!(
        caps.backend == "Vulkan" || caps.backend == "Dx12",
        "expected a Windows compute backend, got {}",
        caps.backend
    );
    assert!(caps.max_buffer_size > 0, "max_buffer_size must be positive");
    assert!(
        caps.max_compute_workgroup_size_x >= 64,
        "a compute-capable adapter should allow at least 64 invocations per workgroup, got {}",
        caps.max_compute_workgroup_size_x
    );
    assert!(
        caps.max_storage_buffers_per_shader_stage >= 4,
        "SpMM will need 4 storage bindings; adapter allows {}",
        caps.max_storage_buffers_per_shader_stage
    );
}

#[test]
fn test_upload_tensor_validates_shape() {
    let Some(backend) = backend_or_skip("upload_shape") else {
        return;
    };
    let res = backend.resources();

    // Correct length succeeds.
    let ok = res.upload_tensor("x", TensorShape::new(2, 3), &[1.0; 6]);
    assert!(ok.is_ok(), "6 elements for a 2x3 shape must be accepted");

    // Wrong length is a ShapeMismatch, not a panic.
    let err = res
        .upload_tensor("bad", TensorShape::new(2, 3), &[1.0; 5])
        .unwrap_err();
    match err {
        ComputeError::ShapeMismatch {
            expected, actual, ..
        } => {
            assert_eq!(expected, 6);
            assert_eq!(actual, 5);
        }
        other => panic!("expected ShapeMismatch, got {other}"),
    }
}

#[test]
fn test_upload_csr_validates_all_three_arrays() {
    let Some(backend) = backend_or_skip("upload_csr") else {
        return;
    };
    let res = backend.resources();
    let shape = SparseCsrShape::new(2, 2, 3);

    let good = res.upload_csr("a", shape, &[0, 1, 3], &[0, 0, 1], &[1.0, 2.0, 3.0]);
    assert!(good.is_ok(), "well-formed CSR must upload");
    let m = good.unwrap();
    assert_eq!(m.shape().nnz, 3);
    assert!(m.size_bytes() > 0);

    // row_ptr must be rows + 1
    assert!(matches!(
        res.upload_csr("a", shape, &[0, 1], &[0, 0, 1], &[1.0, 2.0, 3.0]),
        Err(ComputeError::ShapeMismatch {
            operand: "csr row_ptr",
            ..
        })
    ));
    // col_ind must be nnz
    assert!(matches!(
        res.upload_csr("a", shape, &[0, 1, 3], &[0, 0], &[1.0, 2.0, 3.0]),
        Err(ComputeError::ShapeMismatch {
            operand: "csr col_ind",
            ..
        })
    ));
    // values must be nnz
    assert!(matches!(
        res.upload_csr("a", shape, &[0, 1, 3], &[0, 0, 1], &[1.0, 2.0]),
        Err(ComputeError::ShapeMismatch {
            operand: "csr values",
            ..
        })
    ));
}

#[test]
fn test_empty_shapes_do_not_allocate_zero_sized_buffers() {
    let Some(backend) = backend_or_skip("empty_shapes") else {
        return;
    };
    let res = backend.resources();

    // wgpu rejects a zero-sized buffer; the factory pads to one element while
    // keeping the shape authoritative. The CPU kernels accept empty input, so
    // the GPU path must not diverge by panicking.
    let t = res
        .upload_tensor("empty", TensorShape::new(0, 4), &[])
        .expect("empty tensor must be accepted");
    assert_eq!(t.shape().rows, 0);
    assert!(
        t.size_bytes() > 0,
        "backing buffer is padded, never zero-sized"
    );

    let z = res
        .zeroed_tensor("empty-out", TensorShape::new(0, 0))
        .expect("empty output must be accepted");
    assert!(z.size_bytes() > 0);

    let csr = res
        .upload_csr("empty", SparseCsrShape::new(0, 0, 0), &[0], &[], &[])
        .expect("empty CSR must be accepted");
    assert!(csr.shape().is_empty());

    // Reading back an empty tensor is an empty Vec, not an error.
    assert!(backend.read_tensor(&t).unwrap().is_empty());
}

#[test]
fn test_pipeline_cache_builds_once_and_is_reused() {
    let Some(backend) = backend_or_skip("pipeline_cache") else {
        return;
    };
    let device = backend.device();
    let cache = backend.pipelines();
    assert!(cache.is_empty(), "cache starts empty");

    let bgl = fill_layout(device);
    let layout = fill_pipeline_layout(device, &bgl);

    let key = PipelineKey::new("fill", "main", 64);
    let first = cache
        .get_or_create(device, key.clone(), FILL_SHADER, Some(&layout))
        .expect("shader must compile - a compile failure is a defect, not a skip");
    assert_eq!(cache.len(), 1);

    // Same key -> same pipeline object, cache does not grow.
    let second = cache
        .get_or_create(device, key, FILL_SHADER, Some(&layout))
        .unwrap();
    assert_eq!(
        cache.len(),
        1,
        "identical key must not create a second pipeline"
    );
    assert!(
        std::sync::Arc::ptr_eq(&first, &second),
        "cache must hand back the same pipeline instance"
    );

    // A different workgroup size IS a different module, so it must add an entry.
    let _wide = cache
        .get_or_create(
            device,
            PipelineKey::new("fill", "main", 128),
            FILL_SHADER,
            Some(&layout),
        )
        .unwrap();
    assert_eq!(
        cache.len(),
        2,
        "a different workgroup size is a different pipeline"
    );
}

#[test]
fn test_encode_submit_readback_roundtrip() {
    let Some(backend) = backend_or_skip("roundtrip") else {
        return;
    };
    let device = backend.device();

    // Deliberately not a multiple of the workgroup size (64), so the shader's
    // bounds check is exercised rather than assumed.
    const COUNT: usize = 100;
    const WORKGROUP: u32 = 64;
    const VALUE: f32 = 2.5;

    let shape = TensorShape::new(COUNT, 1);
    let out = backend
        .resources()
        .zeroed_tensor("out", shape)
        .expect("output allocation");

    let params = [COUNT as u32, VALUE.to_bits()];
    let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fill.params"),
        size: 8,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    backend
        .queue()
        .write_buffer(&params_buf, 0, bytemuck::cast_slice(&params));

    let bgl = fill_layout(device);
    let layout = fill_pipeline_layout(device, &bgl);
    let pipeline = backend
        .pipelines()
        .get_or_create(
            device,
            PipelineKey::new("fill", "main", WORKGROUP),
            FILL_SHADER,
            Some(&layout),
        )
        .expect("shader must compile");

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("fill.bind_group"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: out.buffer().as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: params_buf.as_entire_binding(),
            },
        ],
    });

    // Command pattern: encode, then one explicit submit.
    let mut encoder = backend.encoder(Some("fill.pass"));
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("fill"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        let groups = (COUNT as u32).div_ceil(WORKGROUP);
        pass.dispatch_workgroups(groups, 1, 1);
    }
    let index = backend.submit(encoder);
    backend.wait(index).expect("submission must complete");

    // Explicit readback boundary.
    let host = backend.read_tensor(&out).expect("readback");
    assert_eq!(host.len(), COUNT);
    for (i, v) in host.iter().enumerate() {
        assert_eq!(
            *v, VALUE,
            "element {i} was {v}, expected {VALUE} - the bounds check or dispatch size is wrong"
        );
    }
}

#[test]
fn test_two_dispatches_share_one_encoder_and_submit() {
    let Some(backend) = backend_or_skip("single_submit") else {
        return;
    };
    let device = backend.device();
    const COUNT: usize = 32;
    const WORKGROUP: u32 = 64;

    let bgl = fill_layout(device);
    let layout = fill_pipeline_layout(device, &bgl);
    let pipeline = backend
        .pipelines()
        .get_or_create(
            device,
            PipelineKey::new("fill", "main", WORKGROUP),
            FILL_SHADER,
            Some(&layout),
        )
        .unwrap();

    let shape = TensorShape::new(COUNT, 1);
    let mut outs = Vec::new();
    let mut binds = Vec::new();
    for (i, value) in [1.0f32, 9.0f32].into_iter().enumerate() {
        let t = backend
            .resources()
            .zeroed_tensor(&format!("out{i}"), shape)
            .unwrap();
        let p = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 8,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        backend.queue().write_buffer(
            &p,
            0,
            bytemuck::cast_slice(&[COUNT as u32, value.to_bits()]),
        );
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: t.buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: p.as_entire_binding(),
                },
            ],
        });
        outs.push(t);
        binds.push((bg, p));
    }

    // The point of the test: TWO dispatches, ONE encoder, ONE submit.
    let mut encoder = backend.encoder(Some("two-ops"));
    for (bg, _) in &binds {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, bg, &[]);
        pass.dispatch_workgroups((COUNT as u32).div_ceil(WORKGROUP), 1, 1);
    }
    let index = backend.submit(encoder);
    backend.wait(index).unwrap();

    assert!(
        backend
            .read_tensor(&outs[0])
            .unwrap()
            .iter()
            .all(|v| *v == 1.0)
    );
    assert!(
        backend
            .read_tensor(&outs[1])
            .unwrap()
            .iter()
            .all(|v| *v == 9.0)
    );
}

#[test]
fn test_write_tensor_allows_redispatch_without_new_resources() {
    let Some(backend) = backend_or_skip("rewrite_input") else {
        return;
    };
    let res = backend.resources();
    let shape = TensorShape::new(4, 1);
    let t = res
        .upload_tensor("x", shape, &[1.0, 2.0, 3.0, 4.0])
        .unwrap();

    assert_eq!(backend.read_tensor(&t).unwrap(), vec![1.0, 2.0, 3.0, 4.0]);

    // Updating in place is what lets a prepared operation be re-dispatched
    // against new input without rebuilding its bind group.
    res.write_tensor(&t, &[5.0, 6.0, 7.0, 8.0]).unwrap();
    assert_eq!(backend.read_tensor(&t).unwrap(), vec![5.0, 6.0, 7.0, 8.0]);

    // A wrong-length write is rejected, not silently truncated.
    assert!(matches!(
        res.write_tensor(&t, &[1.0, 2.0]),
        Err(ComputeError::ShapeMismatch { .. })
    ));
}
