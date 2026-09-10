//! Identity-transport highway transition over GPU-resident, validated operands.
//!
//! `next[v, c] = t * step[v, c] + (1 - t) * carry[v, c]` with `t = sigmoid(gate[c])`.
//!
//! The mathematical owner remains Nagare's `learning::signed_hsikan` transition block; the shader
//! reproduces its expression and its operation order, and the sigmoid is the provider's existing
//! `act_sigmoid`, not a second definition. Identity transport only: a rotor-transported carry is a
//! different operator and is deliberately absent from this slice.

use std::sync::Arc;

use hymeko::tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use crate::GpuTensor;
use crate::operations::dispatch::PreparedDispatch;
use crate::pipeline::{KernelId, PipelineCache};

/// WGSL source owned by [`KernelId::HighwayIdentity`].
///
/// Composed with the activation formulas so the transition reuses `act_sigmoid` — the same
/// composition pattern the activation and SpMM operations use.
pub const HIGHWAY_IDENTITY_SHADER: &str = concat!(
    include_str!("../../shaders/activation_formulas.wgsl"),
    include_str!("../../shaders/highway_identity.wgsl")
);

/// Default one-dimensional workgroup for the transition.
pub const DEFAULT_HIGHWAY_WORKGROUP: u32 = 128;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct HighwayParams {
    n_nodes: u32,
    hidden: u32,
    count: u32,
    stride: u32,
}

const _: () = assert!(std::mem::size_of::<HighwayParams>() == 16);

#[derive(Clone, Copy, Debug)]
pub(crate) struct HighwayLimits {
    pub max_workgroups_per_dimension: u32,
    pub max_invocations_per_workgroup: u32,
    pub max_storage_buffers_per_stage: u32,
}

/// Builds one validated identity-highway transition dispatch.
pub struct HighwayIdentityBuilder<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    cache: &'a PipelineCache,
    limits: HighwayLimits,
    step: Option<&'a GpuTensor<f32>>,
    carry: Option<&'a GpuTensor<f32>>,
    gate: Option<&'a GpuTensor<f32>>,
    next: Option<&'a GpuTensor<f32>>,
    workgroup: u32,
    label: Option<String>,
}

impl<'a> HighwayIdentityBuilder<'a> {
    pub(crate) fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        cache: &'a PipelineCache,
        limits: HighwayLimits,
    ) -> Self {
        Self {
            device,
            queue,
            cache,
            limits,
            step: None,
            carry: None,
            gate: None,
            next: None,
            workgroup: DEFAULT_HIGHWAY_WORKGROUP,
            label: None,
        }
    }

    /// The scattered vertex state this layer produced.
    #[must_use]
    pub fn step(mut self, step: &'a GpuTensor<f32>) -> Self {
        self.step = Some(step);
        self
    }

    /// The previous vertex state carried along the skip path.
    #[must_use]
    pub fn carry(mut self, carry: &'a GpuTensor<f32>) -> Self {
        self.carry = Some(carry);
        self
    }

    /// The per-channel gate bias, `hidden` entries. The sigmoid is applied on device.
    #[must_use]
    pub fn gate(mut self, gate: &'a GpuTensor<f32>) -> Self {
        self.gate = Some(gate);
        self
    }

    #[must_use]
    pub fn next(mut self, next: &'a GpuTensor<f32>) -> Self {
        self.next = Some(next);
        self
    }

    #[must_use]
    pub fn workgroup(mut self, workgroup: u32) -> Self {
        self.workgroup = workgroup;
        self
    }

    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Validate every operand and prepare a repeatedly encodable dispatch.
    ///
    /// # Errors
    /// [`ComputeError::ShapeMismatch`] when the three state operands disagree or the gate is not
    /// `hidden` long; [`ComputeError::IndexOverflow`] for a count that does not fit `u32`;
    /// [`ComputeError::Backend`] for a device that cannot satisfy the layout or workgroup.
    pub fn prepare(self) -> Result<PreparedHighwayIdentity<'a>, ComputeError> {
        let step = supplied(self.step, "highway step")?;
        let carry = supplied(self.carry, "highway carry")?;
        let gate = supplied(self.gate, "highway gate")?;
        let next = supplied(self.next, "highway next")?;

        let n_nodes = next.shape().rows;
        let hidden = next.shape().cols;
        if n_nodes == 0 || hidden == 0 {
            return shape_error("highway next must be non-empty", 1, 0);
        }
        for (operand, tensor) in [("highway step shape", step), ("highway carry shape", carry)] {
            if tensor.shape() != TensorShape::new(n_nodes, hidden) {
                return shape_error(
                    operand,
                    n_nodes.saturating_mul(hidden),
                    tensor.shape().checked_elements().unwrap_or(usize::MAX),
                );
            }
        }
        let gate_len = gate
            .shape()
            .checked_elements()
            .ok_or(ComputeError::IndexOverflow {
                what: "highway gate length",
                value: usize::MAX,
            })?;
        if gate_len != hidden {
            return shape_error("highway gate length", hidden, gate_len);
        }

        if self.limits.max_storage_buffers_per_stage < 4 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "highway requires 4 storage buffers, device exposes {}",
                    self.limits.max_storage_buffers_per_stage
                ),
            });
        }
        if self.workgroup == 0 || self.workgroup > self.limits.max_invocations_per_workgroup {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "highway workgroup {} outside 1..={}",
                    self.workgroup, self.limits.max_invocations_per_workgroup
                ),
            });
        }

        let elements = next
            .shape()
            .checked_elements()
            .ok_or(ComputeError::IndexOverflow {
                what: "highway output element count",
                value: usize::MAX,
            })?;
        let count = try_u32("highway output element count", elements)?;
        let ideal_groups = count.div_ceil(self.workgroup);
        let groups = ideal_groups.min(self.limits.max_workgroups_per_dimension);
        let stride = groups
            .checked_mul(self.workgroup)
            .ok_or(ComputeError::IndexOverflow {
                what: "highway dispatch stride",
                value: groups as usize,
            })?;
        let params = HighwayParams {
            n_nodes: try_u32("highway node count", n_nodes)?,
            hidden: try_u32("highway hidden", hidden)?,
            count,
            stride,
        };

        let kernel = KernelId::HighwayIdentity;
        let bundle = self.cache.bundle(self.device, kernel, self.workgroup, 1)?;
        let params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("highway_identity.params"),
            size: std::mem::size_of::<HighwayParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("highway_identity.bind_group"),
            layout: bundle.bind_group_layout(),
            entries: &[
                entry(0, step),
                entry(1, carry),
                entry(2, gate),
                entry(3, next),
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });
        Ok(PreparedHighwayIdentity {
            _operands: core::marker::PhantomData,
            dispatch: PreparedDispatch::new(
                Arc::clone(bundle.pipeline()),
                bind_group,
                params_buffer,
                [groups, 1, 1],
                count == 0,
                self.label.unwrap_or_else(|| "highway_identity".to_owned()),
            ),
            kernel,
            elements,
        })
    }
}

fn supplied<'a, T>(value: Option<&'a T>, operand: &'static str) -> Result<&'a T, ComputeError> {
    value.ok_or(ComputeError::ShapeMismatch {
        operand,
        expected: 1,
        actual: 0,
    })
}

fn shape_error<T>(
    operand: &'static str,
    expected: usize,
    actual: usize,
) -> Result<T, ComputeError> {
    Err(ComputeError::ShapeMismatch {
        operand,
        expected,
        actual,
    })
}

fn entry<T>(binding: u32, tensor: &GpuTensor<T>) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: tensor.buffer().as_entire_binding(),
    }
}

/// A validated, repeatedly dispatchable identity-highway transition.
#[derive(Debug)]
pub struct PreparedHighwayIdentity<'a> {
    _operands: core::marker::PhantomData<&'a ()>,
    dispatch: PreparedDispatch,
    kernel: KernelId,
    elements: usize,
}

impl PreparedHighwayIdentity<'_> {
    /// Record the dispatch without submitting it.
    ///
    /// # Errors
    /// Whatever the underlying dispatch reports; a zero-element operation records nothing.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) -> Result<(), ComputeError> {
        self.dispatch.encode_into(encoder, None)
    }

    /// The kernel this operation dispatches.
    #[must_use]
    pub const fn kernel(&self) -> KernelId {
        self.kernel
    }

    /// Output element count, `n_nodes * hidden`.
    #[must_use]
    pub const fn elements(&self) -> usize {
        self.elements
    }

    #[must_use]
    pub const fn dispatch_groups(&self) -> u32 {
        self.dispatch.groups()[0]
    }
}
