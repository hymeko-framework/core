//! Phase 5 §5: a real fan-out (diamond) topology.
//!
//! # Why this lives in `src/` rather than `tests/`
//!
//! The merge kernel needs the raw `wgpu::Buffer` behind a pooled tensor to build
//! its bind group. After the Phase 5 corrective, no public API hands that out --
//! `ArenaTensor::gpu` is `pub(crate)` precisely so a pooled buffer cannot be
//! laundered into an untracked operation.
//!
//! That means an external crate cannot write its own kernel against arena
//! tensors, which is a real and deliberate limitation: custom kernels belong
//! inside the crate, where the layering is visible. This file is the first
//! consequence of that rule, and it is the correct side of the trade -- the
//! alternative was a public raw-buffer accessor that reopens the hole.
//!
//! ```text
//!            input
//!              |
//!            SpMM
//!              |
//!           shared
//!           /     \
//!       ReLU       LeakyReLU
//!         |            |
//!       left         right
//!           \     /
//!            merge
//!              |
//!           output
//! ```
//!
//! # Why this topology and not a longer chain
//!
//! Every Phase 3 and Phase 4 measurement used a **linear** chain, where liveness
//! is trivial: two buffers suffice for any length, and the arena reaching that
//! minimum proves little about harder cases. The ComputeGraph decision document
//! named branching as the one untested condition that could still justify a
//! graph — so this is the test that either supports or undermines that verdict.
//!
//! A diamond is the smallest topology with the two properties a chain lacks:
//!
//! * one value (`shared`) is read by **two** consumers, so its live range spans
//!   both branches rather than ending at the next operation;
//! * two values (`left`, `right`) are live **simultaneously**, so they cannot
//!   share an allocation however clever the pool is.
//!
//! # Computed liveness minimum
//!
//! | after | live set | count |
//! |---|---|---:|
//! | SpMM | `shared` | 1 |
//! | ReLU | `shared`, `left` | 2 |
//! | LeakyReLU | `shared`, `left`, `right` | **3** |
//! | merge | `left`, `right`, `output` | 3 |
//!
//! Peak is **3**. `shared` dies once both branches have read it, so `output` may
//! reuse its allocation — which is what distinguishes an arena that tracks
//! liveness from one that merely caches.
//!
//! Four logical tensors, three allocations. That number is asserted below, not
//! assumed.
//!
//! # The merge operation
//!
//! `merge_add.wgsl` is **test-only** and deliberately not a library operation:
//! the brief forbids growing this phase into a kernel framework, and a merge
//! kernel with no measured consumer would be precisely that.

use crate::testing::SparseSpec;
use crate::testing::fresh_gpu;
use crate::{ActivationKind, ArenaTensor, TensorArenaConfig, TrackedSession, WgpuBackend};
use hymeko_tensor::representations::tensor_csr::TensorCsr;

const POISON: f32 = -98765.5;
const MERGE_SHADER: &str = include_str!("../tests/shaders/merge_add.wgsl");
const MERGE_WORKGROUP: u32 = 64;

// ---------------------------------------------------------------------------
// test-only merge operation
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MergeParams {
    count: u32,
    _pad: [u32; 3],
}

/// Adds two pooled tensors into a third.
///
/// Takes `ArenaTensor` and encodes into a `TrackedSession` by construction —
/// the test-scale equivalent of the library's `Tracked` scope. Written this way
/// rather than against raw `GpuTensor` so the test cannot accidentally
/// demonstrate the very bypass Phase 5 closes.
struct MergeAdd {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    groups: u32,
    _params: wgpu::Buffer,
}

impl MergeAdd {
    fn prepare(
        backend: &WgpuBackend,
        lhs: &ArenaTensor<'_>,
        rhs: &ArenaTensor<'_>,
        out: &ArenaTensor<'_>,
    ) -> Self {
        let device = backend.device();
        let count = lhs.shape().elements();
        assert_eq!(
            count,
            rhs.shape().elements(),
            "merge operands differ in length"
        );
        assert_eq!(
            count,
            out.shape().elements(),
            "merge output differs in length"
        );

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("merge_add"),
            source: wgpu::ShaderSource::Wgsl(MERGE_SHADER.into()),
        });

        let entries: Vec<wgpu::BindGroupLayoutEntry> = (0..4)
            .map(|i| wgpu::BindGroupLayoutEntry {
                binding: i,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: if i == 3 {
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    }
                } else {
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: i != 2 },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    }
                },
                count: None,
            })
            .collect();

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("merge_add.bgl"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("merge_add.pl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("merge_add.pipeline"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let params = MergeParams {
            count: u32::try_from(count).expect("merge count exceeds u32"),
            _pad: [0; 3],
        };
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("merge_add.params"),
            size: std::mem::size_of::<MergeParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        backend
            .queue()
            .write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("merge_add.bg"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: lhs.gpu().buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: rhs.gpu().buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: out.gpu().buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            pipeline,
            bind_group,
            groups: (count as u32).div_ceil(MERGE_WORKGROUP).max(1),
            _params: params_buffer,
        }
    }

    fn encode(&self, session: &mut TrackedSession<'_>) {
        let mut pass = session
            .encoder()
            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("merge_add.pass"),
                timestamp_writes: None,
            });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.dispatch_workgroups(self.groups, 1, 1);
    }
}

// ---------------------------------------------------------------------------
// CPU reference for the whole diamond
// ---------------------------------------------------------------------------

fn cpu_diamond(csr: &TensorCsr<f32>, x: &[f32], fd: usize, alpha: f32) -> Vec<f32> {
    let mut shared = vec![0.0f32; csr.num_rows * fd];
    csr.spmm_into_serial(x, fd, &mut shared);
    shared
        .iter()
        .map(|&v| {
            let left = if v > 0.0 { v } else { 0.0 };
            let right = if v > 0.0 { v } else { alpha * v };
            left + right
        })
        .collect()
}

fn assert_close(got: &[f32], want: &[f32], ctx: &str) {
    assert_eq!(got.len(), want.len(), "{ctx}: length");
    for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
        let abs = (g - w).abs();
        let rel = if w.abs() > 0.0 { abs / w.abs() } else { abs };
        assert!(
            abs <= 1e-5 || rel <= 1e-5,
            "{ctx}: element {i} gpu={g} cpu={w}"
        );
    }
}

// ---------------------------------------------------------------------------
// the topology
// ---------------------------------------------------------------------------

/// The arena must reach the computed liveness minimum of **3** allocations for
/// a four-tensor diamond, and produce the CPU answer.
#[test]
fn test_diamond_reaches_liveness_minimum_and_matches_cpu() {
    let Some(backend) = fresh_gpu("diamond") else {
        return;
    };
    let alpha = 0.01f32;
    let spec = SparseSpec::small();
    let csr = spec.build();
    let fd = 16;
    let res = backend.resources();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    // Signed input, so the SpMM output straddles zero and the two branches
    // genuinely differ. `dense_block` is all-positive, which would make ReLU and
    // LeakyReLU coincide and the whole diamond vacuous.
    let x = signed_block(&csr, fd);
    let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
    let out_shape = shape.output_shape(fd);
    let elements = csr.num_rows * fd;

    let arena = backend.tensor_arena(TensorArenaConfig::default());

    let output_values = {
        // ---- acquire the two branch buffers and the shared one -------------
        let left = arena.acquire(out_shape).unwrap();
        let right = arena.acquire(out_shape).unwrap();

        let mut session = arena.session(Some("diamond"));

        // `shared` lives in an inner scope: dropping it is how the caller tells
        // the arena that both branches have finished reading it. That is the one
        // piece of liveness information the caller must supply, and getting it
        // wrong is the failure mode a graph would remove.
        {
            let shared = arena.acquire(out_shape).unwrap();
            res.write_tensor(&shared, &vec![POISON; elements]).unwrap();
            res.write_tensor(&left, &vec![POISON; elements]).unwrap();
            res.write_tensor(&right, &vec![POISON; elements]).unwrap();

            let spmm = backend
                .operations()
                .spmm()
                .matrix(&matrix)
                .input(&input)
                .output(&shared)
                .prepare()
                .unwrap();
            let relu = backend
                .operations()
                .activation()
                .kind(ActivationKind::Relu)
                .input(&shared)
                .output(&left)
                .prepare()
                .unwrap();
            let leaky = backend
                .operations()
                .activation()
                .kind(ActivationKind::LeakyRelu { alpha })
                .input(&shared)
                .output(&right)
                .prepare()
                .unwrap();

            spmm.encode(&mut session).unwrap();
            relu.encode(&mut session).unwrap();
            leaky.encode(&mut session).unwrap();

            // Three buffers live here — the computed peak.
            assert_eq!(
                arena.stats().buffers_created,
                3,
                "the fan-out peak should need exactly three allocations"
            );
            // `spmm`, `relu`, `leaky` and `shared` all drop at the end of this
            // block. The prepared operations MUST drop before `shared` does, and
            // the borrow checker enforces that ordering rather than trusting it.
        }

        // ---- merge: `output` should recycle `shared`'s allocation ----------
        // Acquired from the SESSION, not the arena: `shared` was released while
        // this session was open, so only a same-session acquire may reclaim it.
        let output = session.acquire(out_shape).unwrap();
        assert_eq!(
            arena.stats().buffers_created,
            3,
            "output must reuse the dead `shared` allocation, not allocate a fourth"
        );
        assert_eq!(
            arena.stats().reuse_hits,
            1,
            "exactly one reuse: output taking over shared"
        );

        let merge = MergeAdd::prepare(&backend, &left, &right, &output);
        merge.encode(&mut session);

        session.submit_and_wait().unwrap();
        backend.read_tensor(&output).unwrap()
    };

    let want = cpu_diamond(&csr, &x, fd, alpha);
    assert_close(&output_values, &want, "diamond");
    assert!(
        !output_values.contains(&POISON),
        "poison leaked into the diamond output"
    );
    // The fixture must actually exercise both branches, or the test is vacuous.
    let mut shared_ref = vec![0.0f32; elements];
    csr.spmm_into_serial(&x, fd, &mut shared_ref);
    assert!(
        shared_ref.iter().any(|v| *v > 0.0) && shared_ref.iter().any(|v| *v < 0.0),
        "fixture produced no sign variety; ReLU and LeakyReLU would be indistinguishable"
    );
}

/// The naive assignment — one buffer per logical tensor — needs four. This is
/// the baseline the arena's three is measured against.
#[test]
fn test_naive_diamond_needs_four_buffers() {
    let Some(backend) = fresh_gpu("diamond_naive") else {
        return;
    };
    let alpha = 0.01f32;
    let spec = SparseSpec::small();
    let csr = spec.build();
    let fd = 16;
    let res = backend.resources();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x = signed_block(&csr, fd);
    let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
    let out_shape = shape.output_shape(fd);

    // One arena tensor per logical tensor, all held to the end: four.
    let arena = backend.tensor_arena(TensorArenaConfig::default());
    let shared = arena.acquire(out_shape).unwrap();
    let left = arena.acquire(out_shape).unwrap();
    let right = arena.acquire(out_shape).unwrap();
    let output = arena.acquire(out_shape).unwrap();

    assert_eq!(
        arena.stats().buffers_created,
        4,
        "holding every logical tensor should cost four allocations"
    );

    let mut session = arena.session(Some("diamond-naive"));
    let spmm = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&shared)
        .prepare()
        .unwrap();
    let relu = backend
        .operations()
        .activation()
        .kind(ActivationKind::Relu)
        .input(&shared)
        .output(&left)
        .prepare()
        .unwrap();
    let leaky = backend
        .operations()
        .activation()
        .kind(ActivationKind::LeakyRelu { alpha })
        .input(&shared)
        .output(&right)
        .prepare()
        .unwrap();
    spmm.encode(&mut session).unwrap();
    relu.encode(&mut session).unwrap();
    leaky.encode(&mut session).unwrap();
    let merge = MergeAdd::prepare(&backend, &left, &right, &output);
    merge.encode(&mut session);
    session.submit_and_wait().unwrap();

    let got = backend.read_tensor(&output).unwrap();
    let want = cpu_diamond(&csr, &x, fd, alpha);
    assert_close(&got, &want, "naive diamond");
}

/// The shared tensor must stay intact for the **second** branch: if the arena
/// recycled it after the first consumer, the second branch would read whatever
/// the new tenant wrote.
#[test]
fn test_shared_tensor_survives_both_branches() {
    let Some(backend) = fresh_gpu("shared_liveness") else {
        return;
    };
    let spec = SparseSpec::small();
    let csr = spec.build();
    let fd = 8;
    let res = backend.resources();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x = signed_block(&csr, fd);
    let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
    let out_shape = shape.output_shape(fd);
    let arena = backend.tensor_arena(TensorArenaConfig::default());

    let shared = arena.acquire(out_shape).unwrap();
    let left = arena.acquire(out_shape).unwrap();
    let right = arena.acquire(out_shape).unwrap();

    let mut session = arena.session(Some("branches"));
    let spmm = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&shared)
        .prepare()
        .unwrap();
    // Identity on both branches: each branch output must equal `shared` exactly,
    // which is only true if `shared` was untouched between the two reads.
    let a = backend
        .operations()
        .activation()
        .kind(ActivationKind::Identity)
        .input(&shared)
        .output(&left)
        .prepare()
        .unwrap();
    let b = backend
        .operations()
        .activation()
        .kind(ActivationKind::Identity)
        .input(&shared)
        .output(&right)
        .prepare()
        .unwrap();
    spmm.encode(&mut session).unwrap();
    a.encode(&mut session).unwrap();
    b.encode(&mut session).unwrap();
    session.submit_and_wait().unwrap();

    let l = backend.read_tensor(&left).unwrap();
    let r = backend.read_tensor(&right).unwrap();
    let s = backend.read_tensor(&shared).unwrap();

    assert_eq!(
        l.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        s.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        "first branch did not observe the shared tensor"
    );
    assert_eq!(
        r.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        s.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        "second branch observed a different shared tensor; it was recycled early"
    );
}

/// Both branch outputs are live at the same time, so they can never share an
/// allocation — whatever the reuse policy.
#[test]
fn test_branch_outputs_never_alias() {
    let Some(backend) = fresh_gpu("branch_alias") else {
        return;
    };
    let spec = SparseSpec::small();
    let csr = spec.build();
    let fd = 8;
    let res = backend.resources();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x = signed_block(&csr, fd);
    let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
    let out_shape = shape.output_shape(fd);
    let arena = backend.tensor_arena(TensorArenaConfig::default());

    let shared = arena.acquire(out_shape).unwrap();
    let left = arena.acquire(out_shape).unwrap();
    let right = arena.acquire(out_shape).unwrap();

    let mut session = arena.session(None);
    let spmm = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&shared)
        .prepare()
        .unwrap();
    // ReLU clamps negatives to +0; LeakyReLU keeps them scaled. If the two
    // outputs aliased, one would overwrite the other and they would be equal.
    let relu = backend
        .operations()
        .activation()
        .kind(ActivationKind::Relu)
        .input(&shared)
        .output(&left)
        .prepare()
        .unwrap();
    let leaky = backend
        .operations()
        .activation()
        .kind(ActivationKind::LeakyRelu { alpha: 0.25 })
        .input(&shared)
        .output(&right)
        .prepare()
        .unwrap();
    spmm.encode(&mut session).unwrap();
    relu.encode(&mut session).unwrap();
    leaky.encode(&mut session).unwrap();
    session.submit_and_wait().unwrap();

    let l = backend.read_tensor(&left).unwrap();
    let r = backend.read_tensor(&right).unwrap();
    assert_ne!(
        l.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        r.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        "branch outputs are identical; they aliased onto one allocation"
    );
    assert!(
        l.iter().all(|v| *v >= 0.0),
        "ReLU branch produced a negative"
    );
    assert!(
        r.iter().any(|v| *v < 0.0),
        "LeakyReLU branch produced no negative"
    );
}

/// After the merge, both branch buffers are dead and the next operation must
/// reuse them rather than allocate.
#[test]
fn test_branch_buffers_are_reusable_after_merge() {
    let Some(backend) = fresh_gpu("post_merge") else {
        return;
    };
    let spec = SparseSpec::small();
    let csr = spec.build();
    let fd = 8;
    let res = backend.resources();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let shape = matrix.shape();
    let x = signed_block(&csr, fd);
    let input = res.upload_tensor("x", shape.input_shape(fd), &x).unwrap();
    let out_shape = shape.output_shape(fd);
    let arena = backend.tensor_arena(TensorArenaConfig::default());

    let output = arena.acquire(out_shape).unwrap();
    {
        let shared = arena.acquire(out_shape).unwrap();
        let left = arena.acquire(out_shape).unwrap();
        let right = arena.acquire(out_shape).unwrap();
        let mut session = arena.session(None);
        let spmm = backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&input)
            .output(&shared)
            .prepare()
            .unwrap();
        let relu = backend
            .operations()
            .activation()
            .kind(ActivationKind::Relu)
            .input(&shared)
            .output(&left)
            .prepare()
            .unwrap();
        let leaky = backend
            .operations()
            .activation()
            .kind(ActivationKind::LeakyRelu { alpha: 0.01 })
            .input(&shared)
            .output(&right)
            .prepare()
            .unwrap();
        spmm.encode(&mut session).unwrap();
        relu.encode(&mut session).unwrap();
        leaky.encode(&mut session).unwrap();
        let merge = MergeAdd::prepare(&backend, &left, &right, &output);
        merge.encode(&mut session);
        session.submit_and_wait().unwrap();
    }

    let created_before = arena.stats().buffers_created;
    assert_eq!(created_before, 4, "diamond with a live output needs four");

    // Three buffers are now dead. A follow-on stage must reuse them.
    let a = arena.acquire(out_shape).unwrap();
    let b = arena.acquire(out_shape).unwrap();
    let c = arena.acquire(out_shape).unwrap();
    assert_eq!(
        arena.stats().buffers_created,
        created_before,
        "the next stage allocated instead of reusing the freed branch buffers"
    );
    assert_eq!(arena.stats().reuse_hits, 3);
    drop((a, b, c));
}

/// A dense block whose values straddle zero.
///
/// `SparseSpec::dense_block` is all-positive, which makes ReLU and LeakyReLU
/// produce identical output and turns any branch test into a tautology. Every
/// diamond test here needs the two branches to be distinguishable.
fn signed_block(csr: &TensorCsr<f32>, fd: usize) -> Vec<f32> {
    (0..csr.num_cols * fd)
        .map(|t| ((t % 17) as f32) - 8.0)
        .collect()
}
