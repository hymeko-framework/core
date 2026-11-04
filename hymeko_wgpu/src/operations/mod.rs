//! Operation construction.
//!
//! [`OperationFactory`] is the Abstract Factory counterpart to
//! [`crate::ResourceFactory`]: resources need a device, and so do the pipelines
//! and bind groups an operation owns. Both are borrowed from a
//! [`crate::WgpuBackend`], which is the only thing that owns a device.
//!
//! The factory owns the concrete SpMM, activation and signed-HSiKAN builders;
//! it is not a dynamic operation registry.

/// Declares a kernel's params uniform: `#[repr(C)]` plain old data whose size is checked at
/// compile time against the size of the struct the WGSL source declares.
macro_rules! uniform_params {
    (
        $(#[$meta:meta])*
        $name:ident($size:literal) { $( $(#[$field_meta:meta])* $field:ident: $ty:ty, )* }
    ) => {
        $(#[$meta])*
        #[repr(C)]
        #[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
        struct $name {
            $( $(#[$field_meta])* $field: $ty, )*
        }

        const _: () = assert!(std::mem::size_of::<$name>() == $size);
    };
}

/// Implements a one-dispatch operation builder declared as `pub struct Name<'a>(Fields<'a>);`:
/// generates `Fields` (the factory, one slot per GPU operand, the operation's scalar settings with
/// their defaults, the workgroup and the label), `new`, one setter per operand, the `workgroup` /
/// `label` setters, and `operands`, which returns every supplied operand or names
/// (`"<operation> <operand>"`) the first one missing.
///
/// The public type stays a hand-written item so the symbol inventory, which reads source and does
/// not expand macros, still lists it. Validation (`prepare`) and setters that take more than one
/// operand reference stay hand-written beside the invocation. The type-state builders (SpMM,
/// activation) carry an encode scope and are written out in full.
macro_rules! operation_builder {
    (
        $name:ident($fields:ident, $operation:literal) {
            $( $(#[$operand_meta:meta])* $operand:ident: $operand_ty:ty, )*
        }
        $( settings { $( $setting:ident: $setting_ty:ty = $default:expr, )* } )?
        workgroup: $workgroup:expr;
    ) => {
        /// The builder's state: the factory, the operands supplied so far and the settings.
        struct $fields<'a> {
            factory: $crate::operations::OperationFactory<'a>,
            $( $operand: Option<&'a $operand_ty>, )*
            $($( $setting: $setting_ty, )*)?
            workgroup: u32,
            label: Option<String>,
        }

        impl<'a> $name<'a> {
            pub(crate) fn new(factory: $crate::operations::OperationFactory<'a>) -> Self {
                Self($fields {
                    factory,
                    $( $operand: None, )*
                    $($( $setting: $default, )*)?
                    workgroup: $workgroup,
                    label: None,
                })
            }

            $(
                $(#[$operand_meta])*
                #[must_use]
                pub fn $operand(mut self, value: &'a $operand_ty) -> Self {
                    self.0.$operand = Some(value);
                    self
                }
            )*

            /// Invocations per workgroup; `prepare` checks it against the device.
            #[must_use]
            pub fn workgroup(mut self, value: u32) -> Self {
                self.0.workgroup = value;
                self
            }

            /// The dispatch's debug label.
            #[must_use]
            pub fn label(mut self, value: impl Into<String>) -> Self {
                self.0.label = Some(value.into());
                self
            }

            /// Every GPU operand, in declaration order.
            fn operands(
                &self,
            ) -> Result<($(&'a $operand_ty,)*), hymeko_tensor::compute::ComputeError> {
                Ok(($(
                    $crate::operations::OperandCheck(concat!($operation, " ", stringify!($operand)))
                        .supplied(self.0.$operand)?,
                )*))
            }
        }
    };
}

/// What every one-dispatch prepared operation holds: its dispatch, kernel and output element
/// count, borrowed for the operands' lifetime.
#[derive(Debug)]
struct PreparedOne<'a> {
    _operands: core::marker::PhantomData<&'a ()>,
    dispatch: PreparedDispatch,
    kernel: KernelId,
    elements: usize,
}

/// Implements a one-dispatch prepared operation declared as `pub struct Name<'a>(PreparedOne<'a>);`:
/// its constructor and the accessors every such operation exposes.
macro_rules! prepared_operation {
    ($name:ident) => {
        impl $name<'_> {
            fn new(
                dispatch: $crate::operations::dispatch::PreparedDispatch,
                kernel: $crate::pipeline::KernelId,
                elements: usize,
            ) -> Self {
                Self($crate::operations::PreparedOne {
                    _operands: core::marker::PhantomData,
                    dispatch,
                    kernel,
                    elements,
                })
            }

            /// Record the dispatch without submitting it.
            ///
            /// # Errors
            /// Whatever the underlying dispatch reports; a zero-element operation records nothing.
            pub fn encode(
                &self,
                encoder: &mut wgpu::CommandEncoder,
            ) -> Result<(), $crate::ComputeError> {
                self.0.dispatch.encode_into(encoder, None)
            }

            /// The kernel this operation dispatches.
            #[must_use]
            pub const fn kernel(&self) -> $crate::pipeline::KernelId {
                self.0.kernel
            }

            /// Output element count.
            #[must_use]
            pub const fn elements(&self) -> usize {
                self.0.elements
            }

            /// Workgroups dispatched along x; zero for an empty operation.
            #[must_use]
            pub const fn dispatch_groups(&self) -> u32 {
                self.0.dispatch.groups()[0]
            }
        }
    };
}

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

use std::sync::Arc;

use hymeko_tensor::compute::{BackendKind, ComputeError};

use crate::pipeline::{KernelId, PipelineCache};
use dispatch::PreparedDispatch;

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

    /// `tensor` has exactly `expected`; a mismatch reports both element counts.
    fn shape<T>(
        self,
        tensor: &crate::GpuTensor<T>,
        expected: hymeko_tensor::compute::TensorShape,
    ) -> Result<(), ComputeError> {
        if tensor.shape() == expected {
            Ok(())
        } else {
            self.mismatch(
                expected.checked_elements().unwrap_or(usize::MAX),
                tensor.shape().checked_elements().unwrap_or(usize::MAX),
            )
        }
    }

    /// `tensor`'s element count, and the same count as the `u32` a shader indexes with.
    fn elements<T>(self, tensor: &crate::GpuTensor<T>) -> Result<(usize, u32), ComputeError> {
        let elements = tensor
            .shape()
            .checked_elements()
            .ok_or(ComputeError::IndexOverflow {
                what: self.0,
                value: usize::MAX,
            })?;
        Ok((elements, hymeko_tensor::compute::try_u32(self.0, elements)?))
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

    /// The workgroup and the storage buffers `kernel`'s layout binds, checked against the device.
    fn require_device(&self, kernel: KernelId, workgroup: u32) -> Result<(), ComputeError> {
        let operation = kernel.shader_name();
        let storage = kernel.layout().storage_buffers();
        if self.limits.max_storage_buffers_per_stage < storage {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "{operation} requires {storage} storage buffers, device exposes {}",
                    self.limits.max_storage_buffers_per_stage
                ),
            });
        }
        if workgroup == 0 || workgroup > self.limits.max_invocations_per_workgroup {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "{operation} workgroup {workgroup} outside 1..={}",
                    self.limits.max_invocations_per_workgroup
                ),
            });
        }
        Ok(())
    }

    /// The groups and the grid stride covering `count` invocations at `workgroup`.
    fn grid_stride(&self, workgroup: u32, count: u32) -> Result<(u32, u32), ComputeError> {
        let groups = count
            .div_ceil(workgroup)
            .min(self.limits.max_workgroups_per_dimension);
        let stride = groups
            .checked_mul(workgroup)
            .ok_or(ComputeError::IndexOverflow {
                what: "dispatch stride",
                value: groups as usize,
            })?;
        Ok((groups, stride))
    }

    /// A one-dimensional grid-stride dispatch of `kernel` over `count` invocations: the device is
    /// checked, the grid sized, and the params built from the grid stride bound after `entries`.
    /// Without a label the kernel's shader name labels it.
    fn grid_dispatch<P: bytemuck::Pod>(
        &self,
        kernel: KernelId,
        workgroup: u32,
        count: u32,
        params: impl FnOnce(u32) -> Result<P, ComputeError>,
        entries: &[wgpu::BindGroupEntry<'_>],
        label: Option<String>,
    ) -> Result<PreparedDispatch, ComputeError> {
        self.require_device(kernel, workgroup)?;
        let (groups, stride) = self.grid_stride(workgroup, count)?;
        let label = label.unwrap_or_else(|| kernel.shader_name().to_owned());
        self.dispatch(kernel, workgroup, groups, &params(stride)?, entries, label)
    }

    /// A dispatch of `kernel` over `entries`, the params uniform bound last at `entries.len()`.
    /// It records nothing when `groups` is zero.
    fn dispatch<P: bytemuck::Pod>(
        &self,
        kernel: KernelId,
        workgroup: u32,
        groups: u32,
        params: &P,
        entries: &[wgpu::BindGroupEntry<'_>],
        label: String,
    ) -> Result<PreparedDispatch, ComputeError> {
        let bundle = self.cache.bundle(self.device, kernel, workgroup, 1)?;
        let params_buffer = self.uniform(&format!("{label}.params"), params);
        let mut bound = entries.to_vec();
        bound.push(wgpu::BindGroupEntry {
            binding: entries.len() as u32,
            resource: params_buffer.as_entire_binding(),
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&label),
            layout: bundle.bind_group_layout(),
            entries: &bound,
        });
        Ok(PreparedDispatch::new(
            Arc::clone(bundle.pipeline()),
            bind_group,
            params_buffer,
            [groups, 1, 1],
            groups == 0,
            label,
        ))
    }
}
