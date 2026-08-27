//! Operation construction.
//!
//! [`OperationFactory`] is the Abstract Factory counterpart to
//! [`crate::ResourceFactory`]: resources need a device, and so do the pipelines
//! and bind groups an operation owns. Both are borrowed from a
//! [`crate::WgpuBackend`], which is the only thing that owns a device.
//!
//! One operation exists so far. The factory is introduced now — rather than in
//! Phase 1 — because there is finally a product to manufacture; adding it
//! earlier would have been a factory with nothing to make.

pub mod activation;
pub(crate) mod dispatch;
pub mod spmm;

pub use activation::{
    ACTIVATION_INPLACE_SHADER, ACTIVATION_SHADER, ACTIVATION_WORKGROUP_SWEEP, ActivationBuilder,
    ActivationKind, DEFAULT_ACTIVATION_WORKGROUP, PreparedActivation,
};
pub use spmm::{PreparedSpmm, SPMM_SHADER, SpmmBuilder, WorkgroupShape, spmm_workgroup_for};

use crate::pipeline::PipelineCache;
use activation::ActivationLimits;
use spmm::SpmmLimits;

/// Creates prepared operations bound to one device.
#[derive(Clone, Copy)]
pub struct OperationFactory<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    cache: &'a PipelineCache,
    limits: SpmmLimits,
    activation_limits: ActivationLimits,
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
}
