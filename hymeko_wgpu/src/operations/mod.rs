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
pub mod global_entropy_pool;
pub mod hsikan;
pub mod rotor_attention;
pub mod spmm;
pub mod star_rotor_spike;

pub use activation::{
    ACTIVATION_INPLACE_SHADER, ACTIVATION_SHADER, ACTIVATION_WORKGROUP_SWEEP, ActivationBuilder,
    ActivationKind, DEFAULT_ACTIVATION_WORKGROUP, PreparedActivation,
};
pub use global_entropy_pool::{
    DEFAULT_GLOBAL_ENTROPY_POOL_WORKGROUP, GLOBAL_ENTROPY_POOL_SHADER, GlobalEntropyPoolBuilder,
    PreparedGlobalEntropyPool,
};
pub use hsikan::{DEFAULT_HSIKAN_WORKGROUP, HSIKAN_SHADER, HsikanBuilder, PreparedHsikan};
pub use rotor_attention::{
    DEFAULT_ROTOR_ATTENTION_WORKGROUP, PreparedRotorAttention, ROTOR_ATTENTION_SHADER,
    RotorAttentionBuilder,
};
pub use spmm::{PreparedSpmm, SPMM_SHADER, SpmmBuilder, WorkgroupShape, spmm_workgroup_for};
pub use star_rotor_spike::{
    DEFAULT_STAR_ROTOR_SPIKE_WORKGROUP, PreparedStarRotorSpike, STAR_ROTOR_SPIKE_SHADER,
    StarRotorSpikeBuilder,
};

use crate::pipeline::PipelineCache;
use activation::ActivationLimits;
use global_entropy_pool::GlobalEntropyPoolLimits;
use hsikan::HsikanLimits;
use rotor_attention::RotorAttentionLimits;
use spmm::SpmmLimits;
use star_rotor_spike::StarRotorSpikeLimits;

/// Creates prepared operations bound to one device.
#[derive(Clone, Copy)]
pub struct OperationFactory<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    cache: &'a PipelineCache,
    limits: SpmmLimits,
    activation_limits: ActivationLimits,
    global_entropy_pool_limits: GlobalEntropyPoolLimits,
    hsikan_limits: HsikanLimits,
    rotor_attention_limits: RotorAttentionLimits,
    star_rotor_spike_limits: StarRotorSpikeLimits,
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
            global_entropy_pool_limits: GlobalEntropyPoolLimits {
                max_workgroups_per_dimension,
                max_invocations_per_workgroup,
            },
            hsikan_limits: HsikanLimits {
                max_workgroups_per_dimension,
                max_invocations_per_workgroup,
                max_storage_buffers_per_stage,
            },
            rotor_attention_limits: RotorAttentionLimits {
                max_workgroups_per_dimension,
                max_invocations_per_workgroup,
                max_storage_buffers_per_stage,
            },
            star_rotor_spike_limits: StarRotorSpikeLimits {
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

    /// Begin building spatial global-entropy pooling over `(K,H,W)` responses.
    #[must_use]
    pub fn global_entropy_pool(&self) -> GlobalEntropyPoolBuilder<'a> {
        GlobalEntropyPoolBuilder::new(
            self.device,
            self.queue,
            self.cache,
            self.global_entropy_pool_limits,
        )
    }

    /// Begin building a fused signed HSiKAN inference dispatch.
    #[must_use]
    pub fn hsikan(&self) -> HsikanBuilder<'a> {
        HsikanBuilder::new(self.device, self.queue, self.cache, self.hsikan_limits)
    }

    /// Begin building sparse single-head quaternion rotor attention.
    #[must_use]
    pub fn rotor_attention(&self) -> RotorAttentionBuilder<'a> {
        RotorAttentionBuilder::new(
            self.device,
            self.queue,
            self.cache,
            self.rotor_attention_limits,
        )
    }

    /// Begin building a star rotor-spike orientation-code dispatch.
    #[must_use]
    pub fn star_rotor_spike(&self) -> StarRotorSpikeBuilder<'a> {
        StarRotorSpikeBuilder::new(
            self.device,
            self.queue,
            self.cache,
            self.star_rotor_spike_limits,
        )
    }
}
