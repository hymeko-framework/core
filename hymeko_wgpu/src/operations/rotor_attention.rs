//! Prepared sparse single-head quaternion rotor-attention inference.
//!
//! This is the deliberately narrow accelerated subset needed by Nagare's
//! canonical Slice 3: four-wide query/key/value rows, one preferred rotor, and
//! untransported values. The CPU mathematical owner remains in Nagare.

use std::sync::Arc;

use hymeko::tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use crate::operations::dispatch::PreparedDispatch;
use crate::pipeline::{KernelId, PipelineCache};
use crate::{GpuCsrMatrix, GpuTensor};

/// WGSL source owned by [`KernelId::RotorAttention`].
pub const ROTOR_ATTENTION_SHADER: &str = include_str!("../../shaders/rotor_attention.wgsl");

/// Default one-dimensional workgroup for rotor attention.
pub const DEFAULT_ROTOR_ATTENTION_WORKGROUP: u32 = 128;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct RotorAttentionParams {
    tokens: u32,
    edge_count: u32,
    count: u32,
    stride: u32,
    kappa: f32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

const _: () = assert!(std::mem::size_of::<RotorAttentionParams>() == 32);

#[derive(Clone, Copy, Debug)]
pub(crate) struct RotorAttentionLimits {
    pub max_workgroups_per_dimension: u32,
    pub max_invocations_per_workgroup: u32,
    pub max_storage_buffers_per_stage: u32,
}

/// Builds one validated sparse rotor-attention dispatch.
pub struct RotorAttentionBuilder<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    cache: &'a PipelineCache,
    limits: RotorAttentionLimits,
    queries: Option<&'a GpuTensor<f32>>,
    keys: Option<&'a GpuTensor<f32>>,
    values: Option<&'a GpuTensor<f32>>,
    preferred: Option<&'a GpuTensor<f32>>,
    adjacency: Option<&'a GpuCsrMatrix<f32>>,
    output: Option<&'a GpuTensor<f32>>,
    kappa: f32,
    workgroup: u32,
    label: Option<String>,
}

impl<'a> RotorAttentionBuilder<'a> {
    pub(crate) fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        cache: &'a PipelineCache,
        limits: RotorAttentionLimits,
    ) -> Self {
        Self {
            device,
            queue,
            cache,
            limits,
            queries: None,
            keys: None,
            values: None,
            preferred: None,
            adjacency: None,
            output: None,
            kappa: 0.0,
            workgroup: DEFAULT_ROTOR_ATTENTION_WORKGROUP,
            label: None,
        }
    }

    #[must_use]
    pub fn queries(mut self, value: &'a GpuTensor<f32>) -> Self {
        self.queries = Some(value);
        self
    }

    #[must_use]
    pub fn keys(mut self, value: &'a GpuTensor<f32>) -> Self {
        self.keys = Some(value);
        self
    }

    #[must_use]
    pub fn values(mut self, value: &'a GpuTensor<f32>) -> Self {
        self.values = Some(value);
        self
    }

    #[must_use]
    pub fn preferred(mut self, value: &'a GpuTensor<f32>) -> Self {
        self.preferred = Some(value);
        self
    }

    #[must_use]
    pub fn adjacency(mut self, value: &'a GpuCsrMatrix<f32>) -> Self {
        self.adjacency = Some(value);
        self
    }

    #[must_use]
    pub fn output(mut self, value: &'a GpuTensor<f32>) -> Self {
        self.output = Some(value);
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
    pub fn prepare(self) -> Result<PreparedRotorAttention<'a>, ComputeError> {
        let queries = supplied(self.queries, "rotor-attention queries")?;
        let keys = supplied(self.keys, "rotor-attention keys")?;
        let values = supplied(self.values, "rotor-attention values")?;
        let preferred = supplied(self.preferred, "rotor-attention preferred rotor")?;
        let adjacency = supplied(self.adjacency, "rotor-attention adjacency")?;
        let output = supplied(self.output, "rotor-attention output")?;
        let tokens = queries.shape().rows;
        let shape = TensorShape::new(tokens, 4);
        if tokens == 0 || queries.shape().cols != 4 {
            return shape_error(
                "rotor-attention queries must be non-empty tokens x 4",
                4,
                queries.shape().cols,
            );
        }
        for (name, actual) in [
            ("rotor-attention keys", keys.shape()),
            ("rotor-attention values", values.shape()),
            ("rotor-attention output", output.shape()),
        ] {
            if actual != shape {
                return shape_error(
                    name,
                    shape.checked_elements().unwrap_or(usize::MAX),
                    actual.checked_elements().unwrap_or(usize::MAX),
                );
            }
        }
        if preferred.shape() != TensorShape::new(1, 4) {
            return shape_error(
                "rotor-attention preferred rotor",
                4,
                preferred.shape().checked_elements().unwrap_or(usize::MAX),
            );
        }
        let sparse = adjacency.shape();
        if sparse.rows != tokens || sparse.cols != tokens {
            return shape_error(
                "rotor-attention square adjacency",
                tokens.saturating_mul(tokens),
                sparse.rows.saturating_mul(sparse.cols),
            );
        }
        if !self.kappa.is_finite() || self.kappa < 0.0 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: "rotor-attention kappa must be finite and non-negative".to_owned(),
            });
        }
        if self.limits.max_storage_buffers_per_stage < 7 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "rotor attention requires 7 storage buffers, device exposes {}",
                    self.limits.max_storage_buffers_per_stage
                ),
            });
        }
        validate_workgroup(self.workgroup, self.limits.max_invocations_per_workgroup)?;
        let count = try_u32("rotor-attention token count", tokens)?;
        let groups = count
            .div_ceil(self.workgroup)
            .min(self.limits.max_workgroups_per_dimension);
        let stride = groups
            .checked_mul(self.workgroup)
            .ok_or(ComputeError::IndexOverflow {
                what: "rotor-attention dispatch stride",
                value: groups as usize,
            })?;
        let params = RotorAttentionParams {
            tokens: count,
            edge_count: try_u32("rotor-attention edge count", sparse.nnz)?,
            count,
            stride,
            kappa: self.kappa,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        };
        let kernel = KernelId::RotorAttention;
        let bundle = self.cache.bundle(self.device, kernel, self.workgroup, 1)?;
        let params_buffer =
            uniform_buffer(self.device, self.queue, "rotor_attention.params", &params);
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rotor_attention.bind_group"),
            layout: bundle.bind_group_layout(),
            entries: &[
                entry(0, queries),
                entry(1, keys),
                entry(2, values),
                entry(3, preferred),
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: adjacency.row_ptr().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: adjacency.col_ind().as_entire_binding(),
                },
                entry(6, output),
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });
        Ok(PreparedRotorAttention {
            _operands: core::marker::PhantomData,
            dispatch: PreparedDispatch::new(
                Arc::clone(bundle.pipeline()),
                bind_group,
                params_buffer,
                [groups, 1, 1],
                count == 0,
                self.label.unwrap_or_else(|| "rotor_attention".to_owned()),
            ),
            kernel,
            elements: tokens * 4,
        })
    }
}

/// A validated, repeatedly dispatchable sparse rotor-attention operation.
#[derive(Debug)]
pub struct PreparedRotorAttention<'a> {
    _operands: core::marker::PhantomData<&'a ()>,
    dispatch: PreparedDispatch,
    kernel: KernelId,
    elements: usize,
}

impl PreparedRotorAttention<'_> {
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

fn validate_workgroup(workgroup: u32, maximum: u32) -> Result<(), ComputeError> {
    if workgroup == 0 || workgroup > maximum {
        Err(ComputeError::Backend {
            backend: BackendKind::Wgpu,
            message: format!("rotor-attention workgroup {workgroup} outside 1..={maximum}"),
        })
    } else {
        Ok(())
    }
}

fn entry<'a, T>(binding: u32, tensor: &'a GpuTensor<T>) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: tensor.buffer().as_entire_binding(),
    }
}

fn uniform_buffer<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    value: &T,
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: std::mem::size_of::<T>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytemuck::bytes_of(value));
    buffer
}
