//! Operation construction.
//!
//! [`OperationFactory`] is the Abstract Factory counterpart to
//! [`crate::ResourceFactory`]: resources need a device, and so do the pipelines
//! and bind groups an operation owns. Both are borrowed from a
//! [`crate::WgpuBackend`], which is the only thing that owns a device.
//!
//! The factory owns the concrete SpMM, activation and signed-HSiKAN builders;
//! it is not a dynamic operation registry.

pub mod activation;
pub(crate) mod dispatch;
pub mod hsikan;
pub mod spmm;

pub use activation::{
    ACTIVATION_INPLACE_SHADER, ACTIVATION_SHADER, ACTIVATION_WORKGROUP_SWEEP, ActivationBuilder,
    ActivationKind, DEFAULT_ACTIVATION_WORKGROUP, PreparedActivation,
};
pub use hsikan::{DEFAULT_HSIKAN_WORKGROUP, HSIKAN_SHADER, HsikanBuilder, PreparedHsikan};
pub use spmm::{PreparedSpmm, SPMM_SHADER, SpmmBuilder, WorkgroupShape, spmm_workgroup_for};

use crate::pipeline::PipelineCache;
use activation::ActivationLimits;
use hsikan::HsikanLimits;
use spmm::SpmmLimits;

/// Creates prepared operations bound to one device.
#[derive(Clone, Copy)]
pub struct OperationFactory<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    cache: &'a PipelineCache,
    limits: SpmmLimits,
    activation_limits: ActivationLimits,
    hsikan_limits: HsikanLimits,
}

impl<'a> OperationFactory<'a> {
    pub(crate) fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        cache: &'a PipelineCache,
        max_workgroups_per_dimension: u32,
        max_invocations_per_workgroup: u32,
        max_storage_buffers_per_stage: u32,
    ) -> Self {
        Self {
            device,
            queue,
            cache,
            limits: SpmmLimits {
                max_workgroups_per_dimension,
                max_invocations_per_workgroup,
                max_storage_buffers_per_stage,
            },
            activation_limits: ActivationLimits {
                max_workgroups_per_dimension,
                max_invocations_per_workgroup,
            },
            hsikan_limits: HsikanLimits {
                max_workgroups_per_dimension,
                max_invocations_per_workgroup,
                max_storage_buffers_per_stage,
            },
        }
    }

    /// Begin building a CSR × dense SpMM.
    #[must_use]
    pub fn spmm(&self) -> SpmmBuilder<'a> {
        SpmmBuilder::new(self.device, self.queue, self.cache, self.limits)
    }

    /// Begin building an elementwise activation.
    #[must_use]
    pub fn activation(&self) -> ActivationBuilder<'a> {
        ActivationBuilder::new(self.device, self.queue, self.cache, self.activation_limits)
    }

    /// Begin building a fused signed HSiKAN inference dispatch.
    #[must_use]
    pub fn hsikan(&self) -> HsikanBuilder<'a> {
        HsikanBuilder::new(self.device, self.queue, self.cache, self.hsikan_limits)
    }
}
