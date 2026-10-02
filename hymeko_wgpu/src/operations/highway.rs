//! Identity-transport highway transition over GPU-resident, validated operands.
//!
//! `next[v, c] = t * step[v, c] + (1 - t) * carry[v, c]` with `t = sigmoid(gate[c])`.
//!
//! The mathematical owner remains Nagare's `learning::signed_hsikan` transition block; the shader
//! reproduces its expression and its operation order, and the sigmoid is the provider's existing
//! `act_sigmoid`, not a second definition. Identity transport only: a rotor-transported carry is a
//! different operator and is deliberately absent from this slice.

use std::sync::Arc;

use hymeko_tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use super::OperandCheck;
use super::OperationFactory;
use crate::GpuTensor;
use crate::operations::dispatch::PreparedDispatch;
use crate::pipeline::KernelId;

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

/// Builds one validated identity-highway transition dispatch.
pub struct HighwayIdentityBuilder<'a> {
    factory: OperationFactory<'a>,
    step: Option<&'a GpuTensor<f32>>,
    carry: Option<&'a GpuTensor<f32>>,
    gate: Option<&'a GpuTensor<f32>>,
    next: Option<&'a GpuTensor<f32>>,
    workgroup: u32,
    label: Option<String>,
}

impl<'a> HighwayIdentityBuilder<'a> {
    pub(crate) fn new(factory: OperationFactory<'a>) -> Self {
        Self {
            factory,
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
        let step = OperandCheck("highway step").supplied(self.step)?;
        let carry = OperandCheck("highway carry").supplied(self.carry)?;
        let gate = OperandCheck("highway gate").supplied(self.gate)?;
        let next = OperandCheck("highway next").supplied(self.next)?;

        let n_nodes = next.shape().rows;
        let hidden = next.shape().cols;
        if n_nodes == 0 || hidden == 0 {
            return OperandCheck("highway next must be non-empty").mismatch(1, 0);
        }
        for (operand, tensor) in [("highway step shape", step), ("highway carry shape", carry)] {
            if tensor.shape() != TensorShape::new(n_nodes, hidden) {
                return OperandCheck(operand).mismatch(
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
            return OperandCheck("highway gate length").mismatch(hidden, gate_len);
        }

        if self.factory.limits.max_storage_buffers_per_stage < 4 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "highway requires 4 storage buffers, device exposes {}",
                    self.factory.limits.max_storage_buffers_per_stage
                ),
            });
        }
        if self.workgroup == 0 || self.workgroup > self.factory.limits.max_invocations_per_workgroup
        {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "highway workgroup {} outside 1..={}",
                    self.workgroup, self.factory.limits.max_invocations_per_workgroup
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
        let groups = ideal_groups.min(self.factory.limits.max_workgroups_per_dimension);
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
        let bundle = self
            .factory
            .cache
            .bundle(self.factory.device, kernel, self.workgroup, 1)?;
        let params_buffer = self.factory.uniform("highway_identity.params", &params);
        let bind_group = self
            .factory
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("highway_identity.bind_group"),
                layout: bundle.bind_group_layout(),
                entries: &[
                    step.binding(0),
                    carry.binding(1),
                    gate.binding(2),
                    next.binding(3),
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
