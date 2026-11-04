//! Prepared spatial global-entropy pooling inference.
//!
//! The canonical tensor shape is `(K,H,W)`. Device storage uses one row per
//! channel and `H*W` columns; `H` and `W` remain explicit validated parameters.

use hymeko_tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use super::OperandCheck;
use crate::GpuTensor;
use crate::pipeline::KernelId;

/// WGSL source owned by [`KernelId::GlobalEntropyPool`].
pub const GLOBAL_ENTROPY_POOL_SHADER: &str = include_str!("../../shaders/global_entropy_pool.wgsl");

/// Default one-dimensional workgroup for channel-parallel entropy pooling.
pub const DEFAULT_GLOBAL_ENTROPY_POOL_WORKGROUP: u32 = 64;

uniform_params! {
    GlobalEntropyPoolParams(32) {
        channels: u32,
        height: u32,
        width: u32,
        area: u32,
        stride: u32,
        _pad0: u32,
        _pad1: u32,
        _pad2: u32,
    }
}

/// Builds one validated spatial global-entropy pooling dispatch.
pub struct GlobalEntropyPoolBuilder<'a>(GlobalEntropyPoolFields<'a>);

operation_builder! {
    GlobalEntropyPoolBuilder(GlobalEntropyPoolFields, "global entropy") {
        response: GpuTensor<f32>,
        output: GpuTensor<f32>,
    }
    settings {
        spatial: Option<(usize, usize)> = None,
    }
    workgroup: DEFAULT_GLOBAL_ENTROPY_POOL_WORKGROUP;
}

impl<'a> GlobalEntropyPoolBuilder<'a> {
    #[must_use]
    pub fn spatial_shape(mut self, height: usize, width: usize) -> Self {
        self.0.spatial = Some((height, width));
        self
    }

    /// Validate every operand and prepare a repeatedly encodable dispatch.
    pub fn prepare(self) -> Result<PreparedGlobalEntropyPool<'a>, ComputeError> {
        let (response, output) = self.operands()?;
        let (height, width) = self.0.spatial.ok_or(ComputeError::ShapeMismatch {
            operand: "global entropy spatial shape",
            expected: 2,
            actual: 0,
        })?;
        let channels = response.shape().rows;
        let area = height
            .checked_mul(width)
            .ok_or(ComputeError::IndexOverflow {
                what: "global entropy raster area",
                value: height,
            })?;
        if channels == 0 || height == 0 || width == 0 {
            return OperandCheck("global entropy dimensions").mismatch(1, 0);
        }
        if response.shape().cols != area {
            return OperandCheck("global entropy response columns")
                .mismatch(area, response.shape().cols);
        }
        OperandCheck("global entropy output shape").shape(output, TensorShape::new(channels, 3))?;
        if std::ptr::eq(response.buffer(), output.buffer()) {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: "global entropy response and output alias the same buffer".to_owned(),
            });
        }
        let kernel = KernelId::GlobalEntropyPool;
        let count = try_u32("global entropy channel count", channels)?;
        let area = try_u32("global entropy raster area", area)?;
        let dispatch = self.0.factory.grid_dispatch(
            kernel,
            self.0.workgroup,
            count,
            |stride| {
                Ok(GlobalEntropyPoolParams {
                    channels: count,
                    height: try_u32("global entropy height", height)?,
                    width: try_u32("global entropy width", width)?,
                    area,
                    stride,
                    _pad0: 0,
                    _pad1: 0,
                    _pad2: 0,
                })
            },
            &[response.binding(0), output.binding(1)],
            self.0.label,
        )?;
        Ok(PreparedGlobalEntropyPool::new(
            dispatch,
            kernel,
            channels * 3,
        ))
    }
}

/// A validated, repeatedly dispatchable spatial global-entropy operation.
#[derive(Debug)]
pub struct PreparedGlobalEntropyPool<'a>(super::PreparedOne<'a>);

prepared_operation!(PreparedGlobalEntropyPool);
