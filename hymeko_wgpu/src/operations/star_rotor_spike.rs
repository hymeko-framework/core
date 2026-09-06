//! Prepared star-centred rotor-spike inference.
//!
//! The operation reads canonical star membership and evaluates Nagare's static
//! von-Mises orientation population code on a selected feature plane.

use std::sync::Arc;

use hymeko::tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use crate::GpuTensor;
use crate::operations::dispatch::PreparedDispatch;
use crate::pipeline::{KernelId, PipelineCache};

/// WGSL source owned by [`KernelId::StarRotorSpike`].
pub const STAR_ROTOR_SPIKE_SHADER: &str = include_str!("../../shaders/star_rotor_spike.wgsl");

/// Default one-dimensional workgroup for star rotor-spike.
pub const DEFAULT_STAR_ROTOR_SPIKE_WORKGROUP: u32 = 128;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct StarRotorSpikeParams {
    n_nodes: u32,
    n_edges: u32,
    arity: u32,
    hidden: u32,
    bins: u32,
    projection_x: u32,
    projection_y: u32,
    count: u32,
    kappa: f32,
    stride: u32,
    _pad0: u32,
    _pad1: u32,
}

const _: () = assert!(std::mem::size_of::<StarRotorSpikeParams>() == 48);

#[derive(Clone, Copy, Debug)]
pub(crate) struct StarRotorSpikeLimits {
    pub max_workgroups_per_dimension: u32,
    pub max_invocations_per_workgroup: u32,
    pub max_storage_buffers_per_stage: u32,
}

/// Builds one validated star rotor-spike dispatch.
pub struct StarRotorSpikeBuilder<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    cache: &'a PipelineCache,
    limits: StarRotorSpikeLimits,
    features: Option<&'a GpuTensor<f32>>,
    centres: Option<&'a GpuTensor<f32>>,
    vertices: Option<&'a GpuTensor<u32>>,
    output: Option<&'a GpuTensor<f32>>,
    projection: (usize, usize),
    kappa: f32,
    workgroup: u32,
    label: Option<String>,
}

impl<'a> StarRotorSpikeBuilder<'a> {
    pub(crate) fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        cache: &'a PipelineCache,
        limits: StarRotorSpikeLimits,
    ) -> Self {
        Self {
            device,
            queue,
            cache,
            limits,
            features: None,
            centres: None,
            vertices: None,
            output: None,
            projection: (0, 1),
            kappa: 0.0,
            workgroup: DEFAULT_STAR_ROTOR_SPIKE_WORKGROUP,
            label: None,
        }
    }

    #[must_use]
    pub fn features(mut self, value: &'a GpuTensor<f32>) -> Self {
        self.features = Some(value);
        self
    }
    #[must_use]
    pub fn centres(mut self, value: &'a GpuTensor<f32>) -> Self {
        self.centres = Some(value);
        self
    }
    #[must_use]
    pub fn vertices(mut self, value: &'a GpuTensor<u32>) -> Self {
        self.vertices = Some(value);
        self
    }
    #[must_use]
    pub fn output(mut self, value: &'a GpuTensor<f32>) -> Self {
        self.output = Some(value);
        self
    }
    #[must_use]
    pub fn projection(mut self, x: usize, y: usize) -> Self {
        self.projection = (x, y);
        self
    }
    #[must_use]
    pub fn kappa(mut self, value: f32) -> Self {
        self.kappa = value;
        self
    }
    #[must_use]
    pub fn workgroup(mut self, value: u32) -> Self {
        self.workgroup = value;
        self
    }
    #[must_use]
    pub fn label(mut self, value: impl Into<String>) -> Self {
        self.label = Some(value.into());
        self
    }

    /// Validate every operand and prepare a repeatedly encodable dispatch.
    pub fn prepare(self) -> Result<PreparedStarRotorSpike<'a>, ComputeError> {
        let features = supplied(self.features, "star rotor-spike features")?;
        let centres = supplied(self.centres, "star rotor-spike centres")?;
        let vertices = supplied(self.vertices, "star rotor-spike vertices")?;
        let output = supplied(self.output, "star rotor-spike output")?;
        let n_nodes = features.shape().rows;
        let hidden = features.shape().cols;
        let n_edges = vertices.shape().rows;
        let arity = vertices.shape().cols;
        let bins = output.shape().cols;
        if n_nodes == 0 || hidden == 0 || n_edges == 0 || arity == 0 || bins == 0 {
            return shape_error("star rotor-spike dimensions must be non-zero", 1, 0);
        }
        if centres.shape() != TensorShape::new(n_edges, hidden) {
            return shape_error(
                "star rotor-spike centre shape",
                n_edges.saturating_mul(hidden),
                centres.shape().checked_elements().unwrap_or(usize::MAX),
            );
        }
        if output.shape().rows != n_edges {
            return shape_error("star rotor-spike output rows", n_edges, output.shape().rows);
        }
        if self.projection.0 >= hidden
            || self.projection.1 >= hidden
            || self.projection.0 == self.projection.1
        {
            return shape_error(
                "star rotor-spike projection",
                hidden,
                self.projection.0.max(self.projection.1),
            );
        }
        if !self.kappa.is_finite() || self.kappa < 0.0 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: "star rotor-spike kappa must be finite and non-negative".to_owned(),
            });
        }
        if self.limits.max_storage_buffers_per_stage < 4 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "star rotor-spike requires 4 storage buffers, device exposes {}",
                    self.limits.max_storage_buffers_per_stage
                ),
            });
        }
        if self.workgroup == 0 || self.workgroup > self.limits.max_invocations_per_workgroup {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "star rotor-spike workgroup {} outside 1..={}",
                    self.workgroup, self.limits.max_invocations_per_workgroup
                ),
            });
        }
        let count = try_u32("star rotor-spike edge count", n_edges)?;
        let groups = count
            .div_ceil(self.workgroup)
            .min(self.limits.max_workgroups_per_dimension);
        let stride = groups
            .checked_mul(self.workgroup)
            .ok_or(ComputeError::IndexOverflow {
                what: "star rotor-spike dispatch stride",
                value: groups as usize,
            })?;
        let params = StarRotorSpikeParams {
            n_nodes: try_u32("star rotor-spike node count", n_nodes)?,
            n_edges: count,
            arity: try_u32("star rotor-spike arity", arity)?,
            hidden: try_u32("star rotor-spike hidden width", hidden)?,
            bins: try_u32("star rotor-spike bins", bins)?,
            projection_x: try_u32("star rotor-spike projection x", self.projection.0)?,
            projection_y: try_u32("star rotor-spike projection y", self.projection.1)?,
            count,
            kappa: self.kappa,
            stride,
            _pad0: 0,
            _pad1: 0,
        };
        let kernel = KernelId::StarRotorSpike;
        let bundle = self.cache.bundle(self.device, kernel, self.workgroup, 1)?;
        let params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("star_rotor_spike.params"),
            size: std::mem::size_of::<StarRotorSpikeParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("star_rotor_spike.bind_group"),
            layout: bundle.bind_group_layout(),
            entries: &[
                entry(0, features),
                entry(1, centres),
                entry(2, vertices),
                entry(3, output),
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });
        Ok(PreparedStarRotorSpike {
            _operands: core::marker::PhantomData,
            dispatch: PreparedDispatch::new(
                Arc::clone(bundle.pipeline()),
                bind_group,
                params_buffer,
                [groups, 1, 1],
                count == 0,
                self.label.unwrap_or_else(|| "star_rotor_spike".to_owned()),
            ),
            kernel,
            elements: n_edges * bins,
        })
    }
}

/// A validated, repeatedly dispatchable star rotor-spike operation.
#[derive(Debug)]
pub struct PreparedStarRotorSpike<'a> {
    _operands: core::marker::PhantomData<&'a ()>,
    dispatch: PreparedDispatch,
    kernel: KernelId,
    elements: usize,
}

impl PreparedStarRotorSpike<'_> {
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

fn entry<'a, T>(binding: u32, tensor: &'a GpuTensor<T>) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: tensor.buffer().as_entire_binding(),
    }
}
