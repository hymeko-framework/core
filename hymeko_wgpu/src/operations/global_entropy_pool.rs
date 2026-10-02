//! Prepared spatial global-entropy pooling inference.
//!
//! The canonical tensor shape is `(K,H,W)`. Device storage uses one row per
//! channel and `H*W` columns; `H` and `W` remain explicit validated parameters.

use std::sync::Arc;

use hymeko_tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use super::OperandCheck;
use super::OperationFactory;
use crate::GpuTensor;
use crate::operations::dispatch::PreparedDispatch;
use crate::pipeline::KernelId;

/// WGSL source owned by [`KernelId::GlobalEntropyPool`].
pub const GLOBAL_ENTROPY_POOL_SHADER: &str = include_str!("../../shaders/global_entropy_pool.wgsl");

/// Default one-dimensional workgroup for channel-parallel entropy pooling.
pub const DEFAULT_GLOBAL_ENTROPY_POOL_WORKGROUP: u32 = 64;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct GlobalEntropyPoolParams {
    channels: u32,
    height: u32,
    width: u32,
    area: u32,
    stride: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

const _: () = assert!(std::mem::size_of::<GlobalEntropyPoolParams>() == 32);

/// Builds one validated spatial global-entropy pooling dispatch.
pub struct GlobalEntropyPoolBuilder<'a> {
    factory: OperationFactory<'a>,
    response: Option<&'a GpuTensor<f32>>,
    output: Option<&'a GpuTensor<f32>>,
    spatial: Option<(usize, usize)>,
    workgroup: u32,
    label: Option<String>,
}

impl<'a> GlobalEntropyPoolBuilder<'a> {
    pub(crate) fn new(factory: OperationFactory<'a>) -> Self {
        Self {
            factory,
            response: None,
            output: None,
            spatial: None,
            workgroup: DEFAULT_GLOBAL_ENTROPY_POOL_WORKGROUP,
            label: None,
        }
    }

    #[must_use]
    pub fn response(mut self, value: &'a GpuTensor<f32>) -> Self {
        self.response = Some(value);
        self
    }

    #[must_use]
    pub fn output(mut self, value: &'a GpuTensor<f32>) -> Self {
        self.output = Some(value);
        self
    }

    #[must_use]
    pub fn spatial_shape(mut self, height: usize, width: usize) -> Self {
        self.spatial = Some((height, width));
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
    pub fn prepare(self) -> Result<PreparedGlobalEntropyPool<'a>, ComputeError> {
        let response = OperandCheck("global entropy response").supplied(self.response)?;
        let output = OperandCheck("global entropy output").supplied(self.output)?;
        let (height, width) = self.spatial.ok_or(ComputeError::ShapeMismatch {
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
        if output.shape() != TensorShape::new(channels, 3) {
            return OperandCheck("global entropy output shape").mismatch(
                channels.saturating_mul(3),
                output.shape().checked_elements().unwrap_or(usize::MAX),
            );
        }
        if std::ptr::eq(response.buffer(), output.buffer()) {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: "global entropy response and output alias the same buffer".to_owned(),
            });
        }
        if self.workgroup == 0 || self.workgroup > self.factory.limits.max_invocations_per_workgroup
        {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "global entropy workgroup {} outside 1..={}",
                    self.workgroup, self.factory.limits.max_invocations_per_workgroup
                ),
            });
        }

        let count = try_u32("global entropy channel count", channels)?;
        let area = try_u32("global entropy raster area", area)?;
        let groups = count
            .div_ceil(self.workgroup)
            .min(self.factory.limits.max_workgroups_per_dimension);
        let stride = groups
            .checked_mul(self.workgroup)
            .ok_or(ComputeError::IndexOverflow {
                what: "global entropy dispatch stride",
                value: groups as usize,
            })?;
        let params = GlobalEntropyPoolParams {
            channels: count,
            height: try_u32("global entropy height", height)?,
            width: try_u32("global entropy width", width)?,
            area,
            stride,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        };
        let kernel = KernelId::GlobalEntropyPool;
        let bundle = self
            .factory
            .cache
            .bundle(self.factory.device, kernel, self.workgroup, 1)?;
        let params_buffer = self.factory.uniform("global_entropy_pool.params", &params);
        let bind_group = self
            .factory
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("global_entropy_pool.bind_group"),
                layout: bundle.bind_group_layout(),
                entries: &[
                    response.binding(0),
                    output.binding(1),
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: params_buffer.as_entire_binding(),
                    },
                ],
            });
        Ok(PreparedGlobalEntropyPool {
            _operands: core::marker::PhantomData,
            dispatch: PreparedDispatch::new(
                Arc::clone(bundle.pipeline()),
                bind_group,
                params_buffer,
                [groups, 1, 1],
                false,
                self.label
                    .unwrap_or_else(|| "global_entropy_pool".to_owned()),
            ),
            kernel,
            elements: channels * 3,
        })
    }
}

/// A validated, repeatedly dispatchable spatial global-entropy operation.
#[derive(Debug)]
pub struct PreparedGlobalEntropyPool<'a> {
    _operands: core::marker::PhantomData<&'a ()>,
    dispatch: PreparedDispatch,
    kernel: KernelId,
    elements: usize,
}

impl PreparedGlobalEntropyPool<'_> {
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
