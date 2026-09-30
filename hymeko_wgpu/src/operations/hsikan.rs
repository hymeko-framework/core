//! Fused signed HSiKAN inference over GPU-resident, validated operands.
//!
//! The mathematical owner remains Nagare's closed-form HSiKAN operator. This
//! backend consumes Catmull--Rom control points already derived by that owner
//! and evaluates the same gather, highway, signed mean and outer-spline chain.

use std::sync::Arc;

use hymeko_tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use super::OperandCheck;
use super::OperationFactory;
use crate::GpuTensor;
use crate::operations::dispatch::PreparedDispatch;
use crate::pipeline::KernelId;

/// WGSL source owned by [`KernelId::Hsikan`].
pub const HSIKAN_SHADER: &str = include_str!("../../shaders/hsikan.wgsl");

/// Default one-dimensional workgroup for fused HSiKAN inference.
pub const DEFAULT_HSIKAN_WORKGROUP: u32 = 128;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct HsikanParams {
    n_nodes: u32,
    n_edges: u32,
    arity: u32,
    hidden: u32,
    branches: u32,
    grid: u32,
    count: u32,
    stride: u32,
}

const _: () = assert!(std::mem::size_of::<HsikanParams>() == 32);

/// Builds one validated signed HSiKAN inference dispatch.
pub struct HsikanBuilder<'a> {
    factory: OperationFactory<'a>,
    x: Option<&'a GpuTensor<f32>>,
    vertices: Option<&'a GpuTensor<u32>>,
    signs: Option<&'a GpuTensor<i32>>,
    inner_control: Option<&'a GpuTensor<f32>>,
    outer_control: Option<&'a GpuTensor<f32>>,
    gate_w: Option<&'a GpuTensor<f32>>,
    gate_b: Option<&'a GpuTensor<f32>>,
    output: Option<&'a GpuTensor<f32>>,
    workgroup: u32,
    label: Option<String>,
}

impl<'a> HsikanBuilder<'a> {
    pub(crate) fn new(factory: OperationFactory<'a>) -> Self {
        Self {
            factory,
            x: None,
            vertices: None,
            signs: None,
            inner_control: None,
            outer_control: None,
            gate_w: None,
            gate_b: None,
            output: None,
            workgroup: DEFAULT_HSIKAN_WORKGROUP,
            label: None,
        }
    }

    #[must_use]
    pub fn features(mut self, x: &'a GpuTensor<f32>) -> Self {
        self.x = Some(x);
        self
    }

    #[must_use]
    pub fn vertices(mut self, vertices: &'a GpuTensor<u32>) -> Self {
        self.vertices = Some(vertices);
        self
    }

    #[must_use]
    pub fn signs(mut self, signs: &'a GpuTensor<i32>) -> Self {
        self.signs = Some(signs);
        self
    }

    #[must_use]
    pub fn inner_control(mut self, control: &'a GpuTensor<f32>) -> Self {
        self.inner_control = Some(control);
        self
    }

    #[must_use]
    pub fn outer_control(mut self, control: &'a GpuTensor<f32>) -> Self {
        self.outer_control = Some(control);
        self
    }

    #[must_use]
    pub fn gate_weight(mut self, weight: &'a GpuTensor<f32>) -> Self {
        self.gate_w = Some(weight);
        self
    }

    #[must_use]
    pub fn gate_bias(mut self, bias: &'a GpuTensor<f32>) -> Self {
        self.gate_b = Some(bias);
        self
    }

    #[must_use]
    pub fn output(mut self, output: &'a GpuTensor<f32>) -> Self {
        self.output = Some(output);
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
    pub fn prepare(self) -> Result<PreparedHsikan<'a>, ComputeError> {
        let x = OperandCheck("hsikan features").supplied(self.x)?;
        let vertices = OperandCheck("hsikan vertices").supplied(self.vertices)?;
        let signs = OperandCheck("hsikan signs").supplied(self.signs)?;
        let inner = OperandCheck("hsikan inner control").supplied(self.inner_control)?;
        let outer = OperandCheck("hsikan outer control").supplied(self.outer_control)?;
        let gate_w = OperandCheck("hsikan gate weight").supplied(self.gate_w)?;
        let gate_b = OperandCheck("hsikan gate bias").supplied(self.gate_b)?;
        let output = OperandCheck("hsikan output").supplied(self.output)?;

        let hidden = x.shape().cols;
        let n_nodes = x.shape().rows;
        let n_edges = vertices.shape().rows;
        let arity = vertices.shape().cols;
        if hidden == 0 || arity == 0 {
            return OperandCheck("hsikan hidden and arity must be non-zero").mismatch(1, 0);
        }
        if signs.shape() != vertices.shape() {
            return OperandCheck("hsikan signs shape").mismatch(
                vertices.shape().checked_elements().unwrap_or(usize::MAX),
                signs.shape().checked_elements().unwrap_or(usize::MAX),
            );
        }
        if output.shape() != TensorShape::new(n_edges, hidden) {
            return OperandCheck("hsikan output shape").mismatch(
                n_edges.saturating_mul(hidden),
                output.shape().checked_elements().unwrap_or(usize::MAX),
            );
        }
        let grid = inner.shape().cols;
        if grid < 3 || inner.shape().rows % hidden != 0 {
            return OperandCheck("hsikan control shape").mismatch(hidden.saturating_mul(3), 0);
        }
        let branches = inner.shape().rows / hidden;
        if !matches!(branches, 1 | 2) {
            return OperandCheck("hsikan branch count").mismatch(2, branches);
        }
        if outer.shape() != inner.shape() {
            return OperandCheck("hsikan outer control shape").mismatch(
                inner.shape().checked_elements().unwrap_or(usize::MAX),
                outer.shape().checked_elements().unwrap_or(usize::MAX),
            );
        }
        if gate_w.shape() != TensorShape::new(hidden, hidden) {
            return OperandCheck("hsikan gate weight shape").mismatch(
                hidden.saturating_mul(hidden),
                gate_w.shape().checked_elements().unwrap_or(usize::MAX),
            );
        }
        if gate_b.shape() != TensorShape::new(hidden, 1) {
            return OperandCheck("hsikan gate bias shape").mismatch(
                hidden,
                gate_b.shape().checked_elements().unwrap_or(usize::MAX),
            );
        }
        if self.factory.limits.max_storage_buffers_per_stage < 8 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "hsikan requires 8 storage buffers, device exposes {}",
                    self.factory.limits.max_storage_buffers_per_stage
                ),
            });
        }
        if self.workgroup == 0 || self.workgroup > self.factory.limits.max_invocations_per_workgroup
        {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "hsikan workgroup {} outside 1..={}",
                    self.workgroup, self.factory.limits.max_invocations_per_workgroup
                ),
            });
        }

        let elements = output
            .shape()
            .checked_elements()
            .ok_or(ComputeError::IndexOverflow {
                what: "hsikan output element count",
                value: usize::MAX,
            })?;
        let count = try_u32("hsikan output element count", elements)?;
        let ideal_groups = count.div_ceil(self.workgroup);
        let groups = ideal_groups.min(self.factory.limits.max_workgroups_per_dimension);
        let stride = groups
            .checked_mul(self.workgroup)
            .ok_or(ComputeError::IndexOverflow {
                what: "hsikan dispatch stride",
                value: groups as usize,
            })?;
        let params = HsikanParams {
            n_nodes: try_u32("hsikan node count", n_nodes)?,
            n_edges: try_u32("hsikan edge count", n_edges)?,
            arity: try_u32("hsikan arity", arity)?,
            hidden: try_u32("hsikan hidden", hidden)?,
            branches: try_u32("hsikan branches", branches)?,
            grid: try_u32("hsikan grid", grid)?,
            count,
            stride,
        };

        let kernel = KernelId::Hsikan;
        let bundle = self
            .factory
            .cache
            .bundle(self.factory.device, kernel, self.workgroup, 1)?;
        let params_buffer = self.factory.uniform("hsikan.params", &params);
        let bind_group = self
            .factory
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hsikan.bind_group"),
                layout: bundle.bind_group_layout(),
                entries: &[
                    x.binding(0),
                    vertices.binding(1),
                    signs.binding(2),
                    inner.binding(3),
                    outer.binding(4),
                    gate_w.binding(5),
                    gate_b.binding(6),
                    output.binding(7),
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: params_buffer.as_entire_binding(),
                    },
                ],
            });
        Ok(PreparedHsikan {
            _operands: core::marker::PhantomData,
            dispatch: PreparedDispatch::new(
                Arc::clone(bundle.pipeline()),
                bind_group,
                params_buffer,
                [groups, 1, 1],
                count == 0,
                self.label.unwrap_or_else(|| "hsikan".to_owned()),
            ),
            kernel,
            elements,
        })
    }
}

/// A validated, repeatedly dispatchable signed HSiKAN inference operation.
#[derive(Debug)]
pub struct PreparedHsikan<'a> {
    _operands: core::marker::PhantomData<&'a ()>,
    dispatch: PreparedDispatch,
    kernel: KernelId,
    elements: usize,
}

impl PreparedHsikan<'_> {
    /// Record the dispatch without submitting it.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) -> Result<(), ComputeError> {
        self.dispatch.encode_into(encoder, None)
    }

    #[must_use]
    pub const fn kernel(&self) -> KernelId {
        self.kernel
    }

    #[must_use]
    pub const fn elements(&self) -> usize {
        self.elements
    }

    #[must_use]
    pub const fn dispatch_groups(&self) -> u32 {
        self.dispatch.groups()[0]
    }
}
