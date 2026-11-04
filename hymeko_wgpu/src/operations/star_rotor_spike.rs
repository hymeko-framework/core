//! Prepared star-centred rotor-spike inference.
//!
//! The operation reads canonical star membership and evaluates Nagare's static
//! von-Mises orientation population code on a selected feature plane.

use hymeko_tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use super::OperandCheck;
use crate::GpuTensor;
use crate::pipeline::KernelId;

/// WGSL source owned by [`KernelId::StarRotorSpike`].
pub const STAR_ROTOR_SPIKE_SHADER: &str = include_str!("../../shaders/star_rotor_spike.wgsl");

/// Default one-dimensional workgroup for star rotor-spike.
pub const DEFAULT_STAR_ROTOR_SPIKE_WORKGROUP: u32 = 128;

uniform_params! {
    StarRotorSpikeParams(48) {
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
}

/// Builds one validated star rotor-spike dispatch.
pub struct StarRotorSpikeBuilder<'a>(StarRotorSpikeFields<'a>);

operation_builder! {
    StarRotorSpikeBuilder(StarRotorSpikeFields, "star rotor-spike") {
        features: GpuTensor<f32>,
        centres: GpuTensor<f32>,
        vertices: GpuTensor<u32>,
        output: GpuTensor<f32>,
    }
    settings {
        projection: (usize, usize) = (0, 1),
        kappa: f32 = 0.0,
    }
    workgroup: DEFAULT_STAR_ROTOR_SPIKE_WORKGROUP;
}

impl<'a> StarRotorSpikeBuilder<'a> {
    #[must_use]
    pub fn projection(mut self, x: usize, y: usize) -> Self {
        self.0.projection = (x, y);
        self
    }
    #[must_use]
    pub fn kappa(mut self, value: f32) -> Self {
        self.0.kappa = value;
        self
    }

    /// Validate every operand and prepare a repeatedly encodable dispatch.
    pub fn prepare(self) -> Result<PreparedStarRotorSpike<'a>, ComputeError> {
        let (features, centres, vertices, output) = self.operands()?;
        let n_nodes = features.shape().rows;
        let hidden = features.shape().cols;
        let n_edges = vertices.shape().rows;
        let arity = vertices.shape().cols;
        let bins = output.shape().cols;
        if n_nodes == 0 || hidden == 0 || n_edges == 0 || arity == 0 || bins == 0 {
            return OperandCheck("star rotor-spike dimensions must be non-zero").mismatch(1, 0);
        }
        OperandCheck("star rotor-spike centre shape")
            .shape(centres, TensorShape::new(n_edges, hidden))?;
        if output.shape().rows != n_edges {
            return OperandCheck("star rotor-spike output rows")
                .mismatch(n_edges, output.shape().rows);
        }
        if self.0.projection.0 >= hidden
            || self.0.projection.1 >= hidden
            || self.0.projection.0 == self.0.projection.1
        {
            return OperandCheck("star rotor-spike projection")
                .mismatch(hidden, self.0.projection.0.max(self.0.projection.1));
        }
        if !self.0.kappa.is_finite() || self.0.kappa < 0.0 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: "star rotor-spike kappa must be finite and non-negative".to_owned(),
            });
        }
        let kernel = KernelId::StarRotorSpike;
        let count = try_u32("star rotor-spike edge count", n_edges)?;
        let (projection_x, projection_y) = self.0.projection;
        let dispatch = self.0.factory.grid_dispatch(
            kernel,
            self.0.workgroup,
            count,
            |stride| {
                Ok(StarRotorSpikeParams {
                    n_nodes: try_u32("star rotor-spike node count", n_nodes)?,
                    n_edges: count,
                    arity: try_u32("star rotor-spike arity", arity)?,
                    hidden: try_u32("star rotor-spike hidden width", hidden)?,
                    bins: try_u32("star rotor-spike bins", bins)?,
                    projection_x: try_u32("star rotor-spike projection x", projection_x)?,
                    projection_y: try_u32("star rotor-spike projection y", projection_y)?,
                    count,
                    kappa: self.0.kappa,
                    stride,
                    _pad0: 0,
                    _pad1: 0,
                })
            },
            &[
                features.binding(0),
                centres.binding(1),
                vertices.binding(2),
                output.binding(3),
            ],
            self.0.label,
        )?;
        Ok(PreparedStarRotorSpike::new(
            dispatch,
            kernel,
            n_edges * bins,
        ))
    }
}

/// A validated, repeatedly dispatchable star rotor-spike operation.
#[derive(Debug)]
pub struct PreparedStarRotorSpike<'a>(super::PreparedOne<'a>);

prepared_operation!(PreparedStarRotorSpike);
