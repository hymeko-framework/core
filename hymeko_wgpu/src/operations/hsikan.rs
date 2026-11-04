//! Fused signed HSiKAN inference over GPU-resident, validated operands.
//!
//! The mathematical owner remains Nagare's closed-form HSiKAN operator. This
//! backend consumes Catmull--Rom control points already derived by that owner
//! and evaluates the same gather, highway, signed mean and outer-spline chain.

use hymeko_tensor::compute::{ComputeError, TensorShape, try_u32};

use super::OperandCheck;
use crate::GpuTensor;
use crate::pipeline::KernelId;

/// WGSL source owned by [`KernelId::Hsikan`].
pub const HSIKAN_SHADER: &str = concat!(
    include_str!("../../shaders/catmull_rom_formulas.wgsl"),
    include_str!("../../shaders/hsikan.wgsl")
);

/// Default one-dimensional workgroup for fused HSiKAN inference.
pub const DEFAULT_HSIKAN_WORKGROUP: u32 = 128;

uniform_params! {
    HsikanParams(32) {
        n_nodes: u32,
        n_edges: u32,
        arity: u32,
        hidden: u32,
        branches: u32,
        grid: u32,
        count: u32,
        stride: u32,
    }
}

/// Builds one validated signed HSiKAN inference dispatch.
pub struct HsikanBuilder<'a>(HsikanFields<'a>);

operation_builder! {
    HsikanBuilder(HsikanFields, "hsikan") {
        features: GpuTensor<f32>,
        vertices: GpuTensor<u32>,
        signs: GpuTensor<i32>,
        inner_control: GpuTensor<f32>,
        outer_control: GpuTensor<f32>,
        gate_weight: GpuTensor<f32>,
        gate_bias: GpuTensor<f32>,
        output: GpuTensor<f32>,
    }
    workgroup: DEFAULT_HSIKAN_WORKGROUP;
}

impl<'a> HsikanBuilder<'a> {
    /// Validate every operand and prepare a repeatedly encodable dispatch.
    pub fn prepare(self) -> Result<PreparedHsikan<'a>, ComputeError> {
        let (x, vertices, signs, inner, outer, gate_w, gate_b, output) = self.operands()?;

        let hidden = x.shape().cols;
        let n_nodes = x.shape().rows;
        let n_edges = vertices.shape().rows;
        let arity = vertices.shape().cols;
        if hidden == 0 || arity == 0 {
            return OperandCheck("hsikan hidden and arity must be non-zero").mismatch(1, 0);
        }
        OperandCheck("hsikan signs shape").shape(signs, vertices.shape())?;
        OperandCheck("hsikan output shape").shape(output, TensorShape::new(n_edges, hidden))?;
        let grid = inner.shape().cols;
        if grid < 3 || inner.shape().rows % hidden != 0 {
            return OperandCheck("hsikan control shape").mismatch(hidden.saturating_mul(3), 0);
        }
        let branches = inner.shape().rows / hidden;
        if !matches!(branches, 1 | 2) {
            return OperandCheck("hsikan branch count").mismatch(2, branches);
        }
        OperandCheck("hsikan outer control shape").shape(outer, inner.shape())?;
        OperandCheck("hsikan gate weight shape").shape(gate_w, TensorShape::new(hidden, hidden))?;
        OperandCheck("hsikan gate bias shape").shape(gate_b, TensorShape::new(hidden, 1))?;
        let kernel = KernelId::Hsikan;
        let (elements, count) = OperandCheck("hsikan output element count").elements(output)?;
        let dispatch = self.0.factory.grid_dispatch(
            kernel,
            self.0.workgroup,
            count,
            |stride| {
                Ok(HsikanParams {
                    n_nodes: try_u32("hsikan node count", n_nodes)?,
                    n_edges: try_u32("hsikan edge count", n_edges)?,
                    arity: try_u32("hsikan arity", arity)?,
                    hidden: try_u32("hsikan hidden", hidden)?,
                    branches: try_u32("hsikan branches", branches)?,
                    grid: try_u32("hsikan grid", grid)?,
                    count,
                    stride,
                })
            },
            &[
                x.binding(0),
                vertices.binding(1),
                signs.binding(2),
                inner.binding(3),
                outer.binding(4),
                gate_w.binding(5),
                gate_b.binding(6),
                output.binding(7),
            ],
            self.0.label,
        )?;
        Ok(PreparedHsikan::new(dispatch, kernel, elements))
    }
}

/// A validated, repeatedly dispatchable signed HSiKAN inference operation.
#[derive(Debug)]
pub struct PreparedHsikan<'a>(super::PreparedOne<'a>);

prepared_operation!(PreparedHsikan);
