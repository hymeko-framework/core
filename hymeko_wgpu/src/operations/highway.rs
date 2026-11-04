//! Identity-transport highway transition over GPU-resident, validated operands.
//!
//! `next[v, c] = t * step[v, c] + (1 - t) * carry[v, c]` with `t = sigmoid(gate[c])`.
//!
//! The mathematical owner remains Nagare's `learning::signed_hsikan` transition block; the shader
//! reproduces its expression and its operation order, and the sigmoid is the provider's existing
//! `act_sigmoid`, not a second definition. Identity transport only: a rotor-transported carry is a
//! different operator and is deliberately absent from this slice.

use hymeko_tensor::compute::{ComputeError, TensorShape, try_u32};

use super::OperandCheck;
use crate::GpuTensor;
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

uniform_params! {
    HighwayParams(16) {
        n_nodes: u32,
        hidden: u32,
        count: u32,
        stride: u32,
    }
}

/// Builds one validated identity-highway transition dispatch.
pub struct HighwayIdentityBuilder<'a>(HighwayIdentityFields<'a>);

operation_builder! {
    HighwayIdentityBuilder(HighwayIdentityFields, "highway") {
        /// The scattered vertex state this layer produced.
        step: GpuTensor<f32>,
        /// The previous vertex state carried along the skip path.
        carry: GpuTensor<f32>,
        /// The per-channel gate bias, `hidden` entries. The sigmoid is applied on device.
        gate: GpuTensor<f32>,
        next: GpuTensor<f32>,
    }
    workgroup: DEFAULT_HIGHWAY_WORKGROUP;
}

impl<'a> HighwayIdentityBuilder<'a> {
    /// Validate every operand and prepare a repeatedly encodable dispatch.
    ///
    /// # Errors
    /// [`ComputeError::ShapeMismatch`] when the three state operands disagree or the gate is not
    /// `hidden` long; [`ComputeError::IndexOverflow`] for a count that does not fit `u32`;
    /// [`ComputeError::Backend`] for a device that cannot satisfy the layout or workgroup.
    pub fn prepare(self) -> Result<PreparedHighwayIdentity<'a>, ComputeError> {
        let (step, carry, gate, next) = self.operands()?;

        let n_nodes = next.shape().rows;
        let hidden = next.shape().cols;
        if n_nodes == 0 || hidden == 0 {
            return OperandCheck("highway next must be non-empty").mismatch(1, 0);
        }
        for (operand, tensor) in [("highway step shape", step), ("highway carry shape", carry)] {
            OperandCheck(operand).shape(tensor, TensorShape::new(n_nodes, hidden))?;
        }
        let (gate_len, _) = OperandCheck("highway gate length").elements(gate)?;
        if gate_len != hidden {
            return OperandCheck("highway gate length").mismatch(hidden, gate_len);
        }

        let kernel = KernelId::HighwayIdentity;
        let (elements, count) = OperandCheck("highway output element count").elements(next)?;
        let dispatch = self.0.factory.grid_dispatch(
            kernel,
            self.0.workgroup,
            count,
            |stride| {
                Ok(HighwayParams {
                    n_nodes: try_u32("highway node count", n_nodes)?,
                    hidden: try_u32("highway hidden", hidden)?,
                    count,
                    stride,
                })
            },
            &[
                step.binding(0),
                carry.binding(1),
                gate.binding(2),
                next.binding(3),
            ],
            self.0.label,
        )?;
        Ok(PreparedHighwayIdentity::new(dispatch, kernel, elements))
    }
}

/// A validated, repeatedly dispatchable identity-highway transition.
#[derive(Debug)]
pub struct PreparedHighwayIdentity<'a>(super::PreparedOne<'a>);

prepared_operation!(PreparedHighwayIdentity);
