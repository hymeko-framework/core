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
pub mod catmull_rom;
pub(crate) mod dispatch;
pub mod global_entropy_pool;
pub mod highway;
pub mod hsikan;
pub mod rotor_attention;
pub mod scatter_mean;
pub mod spmm;
pub mod star_rotor_spike;

pub use activation::{
    ACTIVATION_INPLACE_SHADER, ACTIVATION_SHADER, ACTIVATION_WORKGROUP_SWEEP, ActivationBuilder,
    ActivationKind, DEFAULT_ACTIVATION_WORKGROUP, PreparedActivation,
};
pub use catmull_rom::{
    CATMULL_ROM_FORWARD_SHADER, CATMULL_ROM_PULLBACK_SHADER, CATMULL_ROM_REDUCE_SHADER,
    CatmullRomForwardBuilder, CatmullRomPullbackBuilder, DEFAULT_CATMULL_ROM_CHUNK_ROWS,
    DEFAULT_CATMULL_ROM_WORKGROUP, MAX_CATMULL_ROM_GRID, MIN_CATMULL_ROM_GRID,
    PreparedCatmullRomForward, PreparedCatmullRomPullback,
};
pub use global_entropy_pool::{
    DEFAULT_GLOBAL_ENTROPY_POOL_WORKGROUP, GLOBAL_ENTROPY_POOL_SHADER, GlobalEntropyPoolBuilder,
    PreparedGlobalEntropyPool,
};
pub use highway::{
    DEFAULT_HIGHWAY_WORKGROUP, HIGHWAY_IDENTITY_SHADER, HighwayIdentityBuilder,
    PreparedHighwayIdentity,
};
pub use hsikan::{DEFAULT_HSIKAN_WORKGROUP, HSIKAN_SHADER, HsikanBuilder, PreparedHsikan};
pub use rotor_attention::{
    DEFAULT_ROTOR_ATTENTION_WORKGROUP, PreparedRotorAttention, ROTOR_ATTENTION_SHADER,
    RotorAttentionBuilder,
};
pub use scatter_mean::{
    DEFAULT_SCATTER_MEAN_WORKGROUP, PreparedScatterMean, SCATTER_MEAN_CSR_SHADER,
    ScatterMeanBuilder,
};
pub use spmm::{PreparedSpmm, SPMM_SHADER, SpmmBuilder, WorkgroupShape, spmm_workgroup_for};
pub use star_rotor_spike::{
    DEFAULT_STAR_ROTOR_SPIKE_WORKGROUP, PreparedStarRotorSpike, STAR_ROTOR_SPIKE_SHADER,
    StarRotorSpikeBuilder,
};

use crate::pipeline::PipelineCache;

/// Creates prepared operations bound to one device.
#[derive(Clone, Copy)]
pub struct OperationFactory<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    cache: &'a PipelineCache,
    limits: OperationLimits,
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
            limits: OperationLimits {
                max_workgroups_per_dimension,
                max_invocations_per_workgroup,
                max_storage_buffers_per_stage,
            },
        }
    }

    /// Begin building a CSR × dense SpMM.
    #[must_use]
    pub fn spmm(&self) -> SpmmBuilder<'a> {
        SpmmBuilder::new(*self)
    }

    /// Begin building an elementwise activation.
    #[must_use]
    pub fn activation(&self) -> ActivationBuilder<'a> {
        ActivationBuilder::new(*self)
    }

    /// Begin building a Catmull-Rom membership forward over `[rows, channels]`.
    #[must_use]
    pub fn catmull_rom_forward(&self) -> CatmullRomForwardBuilder<'a> {
        CatmullRomForwardBuilder::new(*self)
    }

    /// Begin building a Catmull-Rom membership pullback: input and control-point gradients.
    #[must_use]
    pub fn catmull_rom_pullback(&self) -> CatmullRomPullbackBuilder<'a> {
        CatmullRomPullbackBuilder::new(*self)
    }

    /// Begin building spatial global-entropy pooling over `(K,H,W)` responses.
    #[must_use]
    pub fn global_entropy_pool(&self) -> GlobalEntropyPoolBuilder<'a> {
        GlobalEntropyPoolBuilder::new(*self)
    }

    /// Begin building a fused signed HSiKAN inference dispatch.
    #[must_use]
    pub fn hsikan(&self) -> HsikanBuilder<'a> {
        HsikanBuilder::new(*self)
    }

    /// Begin building an edge-to-node mean scatter.
    ///
    /// Layout-independent by name: the operation is the pooling, not the storage. The first — and
    /// currently only — physical lowering is the node-indexed CSR gather.
    #[must_use]
    pub fn scatter_mean(&self) -> ScatterMeanBuilder<'a> {
        ScatterMeanBuilder::new(*self)
    }

    /// Begin building an identity-transport highway transition.
    #[must_use]
    pub fn highway_identity(&self) -> HighwayIdentityBuilder<'a> {
        HighwayIdentityBuilder::new(*self)
    }

    /// Begin building sparse single-head quaternion rotor attention.
    #[must_use]
    pub fn rotor_attention(&self) -> RotorAttentionBuilder<'a> {
        RotorAttentionBuilder::new(*self)
    }

    /// Begin building a star rotor-spike orientation-code dispatch.
    #[must_use]
    pub fn star_rotor_spike(&self) -> StarRotorSpikeBuilder<'a> {
        StarRotorSpikeBuilder::new(*self)
    }
}

/// Device capabilities shared by every operation builder.
#[derive(Clone, Copy, Debug)]
struct OperationLimits {
    max_workgroups_per_dimension: u32,
    max_invocations_per_workgroup: u32,
    max_storage_buffers_per_stage: u32,
}

/// Keeps operand diagnostics attached to the operand being validated.
struct OperandCheck(&'static str);

impl OperandCheck {
    fn supplied<T>(self, value: Option<T>) -> Result<T, hymeko_tensor::compute::ComputeError> {
        value.ok_or(hymeko_tensor::compute::ComputeError::ShapeMismatch {
            operand: self.0,
            expected: 1,
            actual: 0,
        })
    }

    fn mismatch<T>(
        self,
        expected: usize,
        actual: usize,
    ) -> Result<T, hymeko_tensor::compute::ComputeError> {
        Err(hymeko_tensor::compute::ComputeError::ShapeMismatch {
            operand: self.0,
            expected,
            actual,
        })
    }
}

impl OperationFactory<'_> {
    fn uniform<T: bytemuck::Pod>(&self, label: &str, value: &T) -> wgpu::Buffer {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: std::mem::size_of::<T>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&buffer, 0, bytemuck::bytes_of(value));
        buffer
    }
}
