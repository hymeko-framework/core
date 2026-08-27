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
use hymeko_wgpu::{KernelId, WgpuBackend};

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
/// Pipeline layout for `fill.wgsl`.
///
/// wgpu 30 takes `Option<&BindGroupLayout>` per slot and replaced push
/// constants with `immediate_size`.
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

    let first = cache
        .bundle(device, KernelId::Fill, 64, 1)
        .expect("shader must compile - a compile failure is a defect, not a skip");
    assert_eq!(cache.len(), 1);

    // Same kernel and extent -> same bundle; the cache does not grow.
    let second = cache.bundle(device, KernelId::Fill, 64, 1).unwrap();
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
    let _wide = cache.bundle(device, KernelId::Fill, 128, 1).unwrap();
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

    let bundle = backend
        .pipelines()
        .bundle(device, KernelId::Fill, WORKGROUP, 1)
        .expect("shader must compile");
    let pipeline = bundle.pipeline();

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("fill.bind_group"),
        layout: bundle.bind_group_layout(),
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
        pass.set_pipeline(pipeline);
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

    let bundle = backend
        .pipelines()
        .bundle(device, KernelId::Fill, WORKGROUP, 1)
        .unwrap();
    let pipeline = bundle.pipeline();

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
            layout: bundle.bind_group_layout(),
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
        pass.set_pipeline(pipeline);
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

// ---------------------------------------------------------------------------
// Phase 9 Part A — the cache invariant
// ---------------------------------------------------------------------------

/// Distinct kernel identities must never share a cache slot.
///
/// The property the previous string-keyed design could not hold. It was not a
/// theoretical gap: registering two different WGSL sources under one key was
/// measured to return the first source's pipeline and execute it, silently
/// discarding the second (Phase 9 report, Part A). That is now impossible to
/// express, because source is no longer a parameter — `KernelId` owns it.
#[test]
fn distinct_kernels_get_distinct_pipelines() {
    let Some(backend) = backend_or_skip("kernel_identity") else {
        return;
    };
    let device = backend.device();
    let cache = backend.pipelines();

    let kernels = [
        KernelId::Spmm,
        KernelId::SpmmActivation(hymeko_wgpu::ActivationEntry::Relu),
        KernelId::ActivationOutOfPlace(hymeko_wgpu::ActivationEntry::Relu),
        KernelId::ActivationInPlace(hymeko_wgpu::ActivationEntry::Relu),
        KernelId::Fill,
    ];
    let mut bundles = Vec::new();
    for k in kernels {
        bundles.push((k, cache.bundle(device, k, 8, 8).expect("bundle")));
    }
    assert_eq!(
        cache.len(),
        kernels.len(),
        "each distinct kernel must occupy its own cache slot"
    );
    for (i, (ka, a)) in bundles.iter().enumerate() {
        for (j, (kb, b)) in bundles.iter().enumerate() {
            if i != j {
                assert!(
                    !std::sync::Arc::ptr_eq(a.pipeline(), b.pipeline()),
                    "{ka:?} and {kb:?} share a pipeline object"
                );
            }
        }
    }
}

/// Raw and fused SpMM share a module and a layout but not a pipeline.
#[test]
fn raw_and_fused_spmm_share_layout_but_not_pipeline() {
    let Some(backend) = backend_or_skip("spmm_layout_sharing") else {
        return;
    };
    let device = backend.device();
    let cache = backend.pipelines();

    let raw = cache.bundle(device, KernelId::Spmm, 16, 8).expect("raw");
    let fused = cache
        .bundle(
            device,
            KernelId::SpmmActivation(hymeko_wgpu::ActivationEntry::Relu),
            16,
            8,
        )
        .expect("fused");

    assert!(
        std::ptr::eq(raw.bind_group_layout(), fused.bind_group_layout()),
        "raw and fused SpMM must reuse one cached bind group layout"
    );
    assert!(
        !std::sync::Arc::ptr_eq(raw.pipeline(), fused.pipeline()),
        "raw and fused SpMM are different entry points and need different pipelines"
    );
    assert_eq!(
        cache.layout_count(),
        1,
        "two SpMM kernels must not build two layouts"
    );
}

/// Concurrent misses on one key converge to one object.
///
/// No lock is held across driver compilation, so two threads can both miss and
/// both compile. The contract is that insertion is idempotent: whoever lands
/// first wins and everyone — including the loser — ends up with that `Arc`.
#[test]
fn pipeline_cache_converges_under_concurrent_misses() {
    let Some(backend) = backend_or_skip("cache_concurrency") else {
        return;
    };
    let device = backend.device();
    let cache = backend.pipelines();

    let bundles: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                s.spawn(|| {
                    cache
                        .bundle(device, KernelId::Fill, 64, 1)
                        .expect("bundle under contention")
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("thread"))
            .collect()
    });

    assert_eq!(
        cache.len(),
        1,
        "concurrent misses on one key must leave exactly one cache entry"
    );
    for (i, b) in bundles.iter().enumerate() {
        assert!(
            std::sync::Arc::ptr_eq(b.pipeline(), bundles[0].pipeline()),
            "thread {i} received a different pipeline object than thread 0"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 9 Part C — CSR structural validation
// ---------------------------------------------------------------------------

/// A malformed CSR must be refused by name, not left to the shader.
///
/// WGPU clamps an out-of-bounds storage access rather than faulting, so an
/// invalid `col_ind` or `row_ptr` would produce a silently wrong result instead
/// of an error. These are cheap host-side checks paid once per upload.
#[test]
fn malformed_csr_is_refused_with_a_named_defect() {
    let Some(backend) = backend_or_skip("csr_validation") else {
        return;
    };
    let res = backend.resources();
    // Valid reference: 3x3, nnz 3, one entry per row.
    let shape = hymeko_wgpu::SparseCsrShape::new(3, 3, 3);
    assert!(
        res.upload_csr("ok", shape, &[0, 1, 2, 3], &[0, 1, 2], &[1.0, 2.0, 3.0])
            .is_ok(),
        "the reference CSR must be accepted"
    );

    let cases: [(&str, Vec<usize>, Vec<usize>); 5] = [
        // row_ptr does not start at 0
        ("start at 0", vec![1, 2, 3, 4], vec![0, 1, 2]),
        // row_ptr goes backwards
        ("non-decreasing", vec![0, 2, 1, 3], vec![0, 1, 2]),
        // row_ptr entry exceeds nnz
        ("exceeds nnz", vec![0, 1, 9, 3], vec![0, 1, 2]),
        // row_ptr does not end at nnz
        ("end at nnz", vec![0, 1, 2, 2], vec![0, 1, 2]),
        // col_ind out of range
        ("valid column", vec![0, 1, 2, 3], vec![0, 1, 7]),
    ];
    for (needle, rp, ci) in cases {
        let err = res
            .upload_csr("bad", shape, &rp, &ci, &[1.0, 2.0, 3.0])
            .expect_err("malformed CSR must be refused");
        match err {
            hymeko_wgpu::ComputeError::ShapeMismatch { operand, .. } => assert!(
                operand.contains(needle),
                "expected an error naming {needle:?}, got {operand:?}"
            ),
            other => panic!("expected ShapeMismatch for {needle:?}, got {other:?}"),
        }
    }
}

/// A zero-row CSR is legal and must not trip the structural checks.
#[test]
fn degenerate_but_valid_csr_shapes_are_accepted() {
    let Some(backend) = backend_or_skip("csr_degenerate") else {
        return;
    };
    let res = backend.resources();
    // No rows, no nonzeros.
    assert!(
        res.upload_csr(
            "empty",
            hymeko_wgpu::SparseCsrShape::new(0, 0, 0),
            &[0],
            &[],
            &[]
        )
        .is_ok(),
        "an empty CSR must be accepted"
    );
    // Rows present, all empty.
    assert!(
        res.upload_csr(
            "all_empty_rows",
            hymeko_wgpu::SparseCsrShape::new(3, 3, 0),
            &[0, 0, 0, 0],
            &[],
            &[]
        )
        .is_ok(),
        "a CSR whose rows are all empty must be accepted"
    );
}

/// Oversized allocations are refused by the shared size check without ever
/// asking the driver for the memory.
#[test]
fn an_allocation_beyond_the_device_limit_is_refused_cheaply() {
    let Some(backend) = backend_or_skip("size_limit") else {
        return;
    };
    let max = backend.capabilities().max_buffer_size;
    let Ok(too_many) = usize::try_from(max / 4 + 1) else {
        eprintln!("[size_limit] SKIPPED - device limit exceeds usize");
        return;
    };
    let err = backend
        .resources()
        .zeroed_tensor("huge", TensorShape::new(too_many, 1))
        .expect_err("an allocation past max_buffer_size must be refused");
    assert!(
        matches!(err, hymeko_wgpu::ComputeError::ShapeMismatch { .. }),
        "expected ShapeMismatch naming the device limit, got {err:?}"
    );
}
