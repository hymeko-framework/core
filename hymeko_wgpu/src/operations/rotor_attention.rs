//! Prepared sparse single-head quaternion rotor-attention inference.
//!
//! This is the deliberately narrow accelerated subset needed by Nagare's
//! canonical Slice 3: four-wide query/key/value rows, one preferred rotor, and
//! untransported values. The CPU mathematical owner remains in Nagare.

use std::sync::Arc;

use hymeko_tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use super::OperandCheck;
use super::OperationFactory;
use crate::operations::dispatch::PreparedDispatch;
use crate::pipeline::KernelId;
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

/// Builds one validated sparse rotor-attention dispatch.
pub struct RotorAttentionBuilder<'a> {
    factory: OperationFactory<'a>,
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
    pub(crate) fn new(factory: OperationFactory<'a>) -> Self {
        Self {
            factory,
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
        let queries = OperandCheck("rotor-attention queries").supplied(self.queries)?;
        let keys = OperandCheck("rotor-attention keys").supplied(self.keys)?;
        let values = OperandCheck("rotor-attention values").supplied(self.values)?;
        let preferred = OperandCheck("rotor-attention preferred rotor").supplied(self.preferred)?;
        let adjacency = OperandCheck("rotor-attention adjacency").supplied(self.adjacency)?;
        let output = OperandCheck("rotor-attention output").supplied(self.output)?;
        let tokens = queries.shape().rows;
        let shape = TensorShape::new(tokens, 4);
        if tokens == 0 || queries.shape().cols != 4 {
            return OperandCheck("rotor-attention queries must be non-empty tokens x 4")
                .mismatch(4, queries.shape().cols);
        }
        for (name, actual) in [
            ("rotor-attention keys", keys.shape()),
            ("rotor-attention values", values.shape()),
            ("rotor-attention output", output.shape()),
        ] {
            if actual != shape {
                return OperandCheck(name).mismatch(
                    shape.checked_elements().unwrap_or(usize::MAX),
                    actual.checked_elements().unwrap_or(usize::MAX),
                );
            }
        }
        if preferred.shape() != TensorShape::new(1, 4) {
            return OperandCheck("rotor-attention preferred rotor").mismatch(
                4,
                preferred.shape().checked_elements().unwrap_or(usize::MAX),
            );
        }
        let sparse = adjacency.shape();
        if sparse.rows != tokens || sparse.cols != tokens {
            return OperandCheck("rotor-attention square adjacency").mismatch(
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
        if self.factory.limits.max_storage_buffers_per_stage < 7 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "rotor attention requires 7 storage buffers, device exposes {}",
                    self.factory.limits.max_storage_buffers_per_stage
                ),
            });
        }
        validate_workgroup(
            self.workgroup,
            self.factory.limits.max_invocations_per_workgroup,
        )?;
        let count = try_u32("rotor-attention token count", tokens)?;
        let groups = count
            .div_ceil(self.workgroup)
            .min(self.factory.limits.max_workgroups_per_dimension);
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
        let bundle = self
            .factory
            .cache
            .bundle(self.factory.device, kernel, self.workgroup, 1)?;
        let params_buffer = self.factory.uniform("rotor_attention.params", &params);
        let bind_group = self
            .factory
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rotor_attention.bind_group"),
                layout: bundle.bind_group_layout(),
                entries: &[
                    queries.binding(0),
                    keys.binding(1),
                    values.binding(2),
                    preferred.binding(3),
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: adjacency.row_ptr().as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: adjacency.col_ind().as_entire_binding(),
                    },
                    output.binding(6),
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
