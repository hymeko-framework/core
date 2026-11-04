//! Prepared sparse single-head quaternion rotor-attention inference.
//!
//! This is the deliberately narrow accelerated subset needed by Nagare's
//! canonical Slice 3: four-wide query/key/value rows, one preferred rotor, and
//! untransported values. The CPU mathematical owner remains in Nagare.

use hymeko_tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use super::OperandCheck;
use crate::pipeline::KernelId;
use crate::{GpuCsrMatrix, GpuTensor};

/// WGSL source owned by [`KernelId::RotorAttention`].
pub const ROTOR_ATTENTION_SHADER: &str = include_str!("../../shaders/rotor_attention.wgsl");

/// Default one-dimensional workgroup for rotor attention.
pub const DEFAULT_ROTOR_ATTENTION_WORKGROUP: u32 = 128;

uniform_params! {
    RotorAttentionParams(32) {
        tokens: u32,
        edge_count: u32,
        count: u32,
        stride: u32,
        kappa: f32,
        _pad0: u32,
        _pad1: u32,
        _pad2: u32,
    }
}

/// Builds one validated sparse rotor-attention dispatch.
pub struct RotorAttentionBuilder<'a>(RotorAttentionFields<'a>);

operation_builder! {
    RotorAttentionBuilder(RotorAttentionFields, "rotor-attention") {
        queries: GpuTensor<f32>,
        keys: GpuTensor<f32>,
        values: GpuTensor<f32>,
        preferred: GpuTensor<f32>,
        adjacency: GpuCsrMatrix<f32>,
        output: GpuTensor<f32>,
    }
    settings {
        kappa: f32 = 0.0,
    }
    workgroup: DEFAULT_ROTOR_ATTENTION_WORKGROUP;
}

impl<'a> RotorAttentionBuilder<'a> {
    #[must_use]
    pub fn kappa(mut self, value: f32) -> Self {
        self.0.kappa = value;
        self
    }

    /// Validate every operand and prepare a repeatedly encodable dispatch.
    pub fn prepare(self) -> Result<PreparedRotorAttention<'a>, ComputeError> {
        let (queries, keys, values, preferred, adjacency, output) = self.operands()?;
        let tokens = queries.shape().rows;
        let shape = TensorShape::new(tokens, 4);
        if tokens == 0 || queries.shape().cols != 4 {
            return OperandCheck("rotor-attention queries must be non-empty tokens x 4")
                .mismatch(4, queries.shape().cols);
        }
        for (name, tensor) in [
            ("rotor-attention keys", keys),
            ("rotor-attention values", values),
            ("rotor-attention output", output),
        ] {
            OperandCheck(name).shape(tensor, shape)?;
        }
        OperandCheck("rotor-attention preferred rotor").shape(preferred, TensorShape::new(1, 4))?;
        let sparse = adjacency.shape();
        if sparse.rows != tokens || sparse.cols != tokens {
            return OperandCheck("rotor-attention square adjacency").mismatch(
                tokens.saturating_mul(tokens),
                sparse.rows.saturating_mul(sparse.cols),
            );
        }
        if !self.0.kappa.is_finite() || self.0.kappa < 0.0 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: "rotor-attention kappa must be finite and non-negative".to_owned(),
            });
        }
        let kernel = KernelId::RotorAttention;
        let count = try_u32("rotor-attention token count", tokens)?;
        let edge_count = try_u32("rotor-attention edge count", sparse.nnz)?;
        let dispatch = self.0.factory.grid_dispatch(
            kernel,
            self.0.workgroup,
            count,
            |stride| {
                Ok(RotorAttentionParams {
                    tokens: count,
                    edge_count,
                    count,
                    stride,
                    kappa: self.0.kappa,
                    _pad0: 0,
                    _pad1: 0,
                    _pad2: 0,
                })
            },
            &[
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
            ],
            self.0.label,
        )?;
        Ok(PreparedRotorAttention::new(dispatch, kernel, tokens * 4))
    }
}

/// A validated, repeatedly dispatchable sparse rotor-attention operation.
#[derive(Debug)]
pub struct PreparedRotorAttention<'a>(super::PreparedOne<'a>);

prepared_operation!(PreparedRotorAttention);
