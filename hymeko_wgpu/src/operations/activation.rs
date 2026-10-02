//! Elementwise activation: `Y = f(X)`.
//!
//! The second GPU operation, and the one that makes composition observable:
//! an [`crate::PreparedSpmm`] and a [`PreparedActivation`] encode into the same
//! `CommandEncoder` and execute in one submit, with the intermediate tensor
//! never leaving the device.
//!
//! # Where `ActivationKind` lives
//!
//! In this crate, **not** in `hymeko_tensor::compute`. The core
//! vocabulary rule is that a type earns its place there only with a concrete
//! CPU *and* GPU consumer. The CPU layers apply ReLU and LeakyReLU inline
//! (`conv/hgnn.rs`, `conv/signed_hgnn.rs`) with the slope hard-coded; none of
//! them takes an activation-kind parameter, so there is no CPU consumer to
//! share with. Promoting it would be an abstraction with one user.

use std::sync::Arc;

use hymeko_tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use super::OperationFactory;
use crate::operations::dispatch::PreparedDispatch;
use crate::pipeline::{ActivationEntry, KernelId};
use crate::resources::GpuTensor;
use crate::scope::{EncodeScope, JoinScope, Joined, TensorOperand, Tracked, Untracked};

/// WGSL source for the out-of-place activation kernel.
///
/// `activation_formulas.wgsl` prepended by compile-time concatenation, so each
/// formula has one definition shared with the in-place and fused-SpMM kernels.
pub const ACTIVATION_SHADER: &str = concat!(
    include_str!("../../shaders/activation_formulas.wgsl"),
    include_str!("../../shaders/activation.wgsl")
);
/// WGSL source for the in-place activation kernel. Same shared formulas.
pub const ACTIVATION_INPLACE_SHADER: &str = concat!(
    include_str!("../../shaders/activation_formulas.wgsl"),
    include_str!("../../shaders/activation_inplace.wgsl")
);

/// Which elementwise function to apply.
///
/// # Numerical contract
///
/// Defined per kind rather than left to the shading language, because `max`
/// with a NaN operand is implementation-defined in WGSL:
///
/// | kind | `NaN` | `-0.0` | `-Inf` | `+Inf` |
/// |---|---|---|---|---|
/// | `Identity` | `NaN` (bit-preserved) | `-0.0` | `-Inf` | `+Inf` |
/// | `Relu` | `+0.0` | `+0.0` | `+0.0` | `+Inf` |
/// | `LeakyRelu { alpha }` | `NaN` | `-0.0` (α>0) | `-Inf` (α>0) | `+Inf` |
/// | `Sigmoid` | `NaN` | `0.5` | `0.0` | `1.0` |
/// | `Tanh` | `NaN` | `-0.0` | `-1.0` | `1.0` |
///
/// `Relu` and `LeakyRelu` reproduce exactly what the CPU convolution layers
/// already do, so a future CPU/GPU A/B of a full forward pass compares like
/// with like.
///
/// # Subnormals — measured, not assumed
///
/// On the reference adapter (RTX 5080 / Vulkan), characterised by
/// `test_subnormal_handling_is_characterised_not_assumed`:
///
/// * **storage and copy preserve subnormals exactly** — `Identity` round-trips
///   even `0x0000_0001` (1.4e-45) bit-for-bit;
/// * **arithmetic flushes a subnormal *result* to zero, with the sign kept** —
///   `LeakyRelu { alpha: 0.01 }` applied to `-f32::MIN_POSITIVE` gives
///   `-1.1755e-40` on the CPU and exactly `-0.0` on the GPU.
///
/// That is flush-to-zero on arithmetic, not denormals-are-zero on input, and
/// WGSL guarantees neither. Callers whose values live within a couple of orders
/// of magnitude of `f32::MIN_POSITIVE` should expect the GPU to zero them where
/// the CPU would not. The test admits this as a **narrow, signed exception**
/// rather than by relaxing a tolerance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ActivationKind {
    Identity,
    Relu,
    /// `v > 0 ? v : alpha * v`. The CPU Signed-HGNN uses `alpha = 0.01`.
    LeakyRelu {
        alpha: f32,
    },
    Sigmoid,
    Tanh,
}

impl ActivationKind {
    /// The compiled-code identity of this kind, without the LeakyReLU slope.
    ///
    /// `alpha` is a runtime uniform: two LeakyReLU operations with different
    /// slopes share one pipeline. Folding it into kernel identity would compile
    /// a pipeline per slope value.
    #[must_use]
    pub const fn entry(self) -> ActivationEntry {
        match self {
            Self::Identity => ActivationEntry::Identity,
            Self::Relu => ActivationEntry::Relu,
            Self::LeakyRelu { .. } => ActivationEntry::LeakyRelu,
            Self::Sigmoid => ActivationEntry::Sigmoid,
            Self::Tanh => ActivationEntry::Tanh,
        }
    }

    /// Short label for benchmark ids and error messages.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Relu => "relu",
            Self::LeakyRelu { .. } => "leaky_relu",
            Self::Sigmoid => "sigmoid",
            Self::Tanh => "tanh",
        }
    }

    /// The `alpha` this kind sends to the shader (0 where unused).
    #[must_use]
    pub const fn alpha(self) -> f32 {
        match self {
            Self::LeakyRelu { alpha } => alpha,
            _ => 0.0,
        }
    }

    /// Whether results are expected to be bit-reproducible against a CPU
    /// reference.
    ///
    /// `Sigmoid` and `Tanh` call transcendental intrinsics whose precision is
    /// only bounded, not specified, so they must be compared with a looser
    /// tolerance. The others are exact selects and copies.
    #[must_use]
    pub const fn is_transcendental(self) -> bool {
        matches!(self, Self::Sigmoid | Self::Tanh)
    }
}

impl core::fmt::Display for ActivationKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::LeakyRelu { alpha } => write!(f, "leaky_relu(a={alpha})"),
            other => f.write_str(other.as_str()),
        }
    }
}

/// Uniform parameter block. Mirrors `struct Params` in both activation shaders.
///
/// 16 bytes with explicit tail padding, matching the uniform address space's
/// alignment rule. No `usize` field; `count` arrives through checked
/// [`try_u32`].
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct ActivationParams {
    count: u32,
    alpha: f32,
    /// Total invocations in the dispatch (`groups * workgroup`).
    ///
    /// The kernel strides by this, so a grid smaller than `count` still covers
    /// every element. See the `groups` computation in `prepare`.
    stride: u32,
    _pad1: u32,
}

const _: () = assert!(std::mem::size_of::<ActivationParams>() == 16);

/// Builds and validates a [`PreparedActivation`].
pub struct ActivationBuilder<'a, S: EncodeScope = Untracked> {
    factory: OperationFactory<'a>,
    kind: ActivationKind,
    input: Option<&'a GpuTensor<f32>>,
    output: Option<&'a GpuTensor<f32>>,
    workgroup: u32,
    label: Option<String>,
    /// Provenance of the operands seen so far; see [`crate::scope`].
    _scope: core::marker::PhantomData<S>,
}

/// Workgroup sizes the phase brief requires comparing.
pub const ACTIVATION_WORKGROUP_SWEEP: [u32; 3] = [64, 128, 256];

/// Default workgroup extent.
///
/// 256 is a placeholder pending the sweep, chosen because an elementwise kernel
/// is bandwidth-bound and larger groups amortise launch overhead. **Not**
/// asserted as optimal.
pub const DEFAULT_ACTIVATION_WORKGROUP: u32 = 256;

impl<'a> ActivationBuilder<'a, Untracked> {
    pub(crate) fn new(factory: OperationFactory<'a>) -> Self {
        Self {
            factory,
            kind: ActivationKind::Relu,
            input: None,
            output: None,
            workgroup: DEFAULT_ACTIVATION_WORKGROUP,
            label: None,
            _scope: core::marker::PhantomData,
        }
    }
}

impl<'a, S: EncodeScope> ActivationBuilder<'a, S> {
    /// Carry every field across a scope change; see `SpmmBuilder::retype`.
    fn retype<S2: EncodeScope>(self) -> ActivationBuilder<'a, S2> {
        ActivationBuilder {
            factory: self.factory,
            kind: self.kind,
            input: self.input,
            output: self.output,
            workgroup: self.workgroup,
            label: self.label,
            _scope: core::marker::PhantomData,
        }
    }

    #[must_use]
    pub fn kind(mut self, kind: ActivationKind) -> Self {
        self.kind = kind;
        self
    }

    #[must_use]
    pub fn input<T>(mut self, input: &'a T) -> ActivationBuilder<'a, Joined<S, T>>
    where
        T: TensorOperand,
        S: JoinScope<T::Scope>,
    {
        self.input = Some(input.gpu_tensor());
        self.retype()
    }

    /// Destination. Pass the **same tensor** as `input` for in-place execution;
    /// see [`PreparedActivation::is_in_place`].
    #[must_use]
    pub fn output<T>(mut self, output: &'a T) -> ActivationBuilder<'a, Joined<S, T>>
    where
        T: TensorOperand,
        S: JoinScope<T::Scope>,
    {
        self.output = Some(output.gpu_tensor());
        self.retype()
    }

    /// Run in place on the input tensor. Equivalent to passing it as both
    /// `input` and `output`, but states the intent at the call site.
    #[must_use]
    pub fn in_place<T>(mut self, tensor: &'a T) -> ActivationBuilder<'a, Joined<S, T>>
    where
        T: TensorOperand,
        S: JoinScope<T::Scope>,
    {
        let t = tensor.gpu_tensor();
        self.input = Some(t);
        self.output = Some(t);
        self.retype()
    }

    #[must_use]
    pub fn workgroup(mut self, workgroup: u32) -> Self {
        self.workgroup = workgroup;
        self
    }

    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Validate and build.
    ///
    /// # Errors
    /// * [`ComputeError::ShapeMismatch`] — operand missing, or input and output
    ///   shapes differ;
    /// * [`ComputeError::IndexOverflow`] — element count or dispatch extent
    ///   beyond `u32` / the device limit;
    /// * [`ComputeError::Backend`] — workgroup exceeds the device's invocation
    ///   limit, or shader compilation fails.
    pub fn prepare(self) -> Result<PreparedActivation<'a, S>, ComputeError> {
        let input = self.input.ok_or(ComputeError::ShapeMismatch {
            operand: "activation input (not supplied)",
            expected: 1,
            actual: 0,
        })?;
        let output = self.output.ok_or(ComputeError::ShapeMismatch {
            operand: "activation output (not supplied)",
            expected: 1,
            actual: 0,
        })?;

        let in_shape: TensorShape = input.shape();
        let out_shape: TensorShape = output.shape();

        // Elementwise: shapes must match exactly, not merely in element count,
        // so a [4x8] is not silently accepted for a [8x4].
        if in_shape != out_shape {
            return Err(ComputeError::ShapeMismatch {
                operand: "activation output shape (must equal input shape)",
                expected: in_shape.checked_elements().unwrap_or(usize::MAX),
                actual: out_shape.checked_elements().unwrap_or(usize::MAX),
            });
        }

        let elements = in_shape
            .checked_elements()
            .ok_or(ComputeError::IndexOverflow {
                what: "activation element count",
                value: usize::MAX,
            })?;
        let count = try_u32("activation element count", elements)?;

        // Aliasing is not an error here -- it selects the in-place kernel.
        let in_place = std::ptr::eq(input.buffer(), output.buffer());

        if self.workgroup == 0 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: "activation workgroup size must be non-zero".to_owned(),
            });
        }
        if self.workgroup > self.factory.limits.max_invocations_per_workgroup {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "activation workgroup {} exceeds the device limit of {} invocations",
                    self.workgroup, self.factory.limits.max_invocations_per_workgroup
                ),
            });
        }

        // A device caps workgroups per dimension (65 535 here), so a 256-wide
        // group reaches only ~16.7M elements in one grid. Rather than failing —
        // 200k rows x fd 128 is 25.6M elements, an ordinary workload — the grid
        // is clamped and the kernel strides through the remainder.
        //
        // This is the "more than one element per invocation" case the operation
        // is allowed to take, and it costs nothing when the grid already fits:
        // `stride >= count` makes the loop body run exactly once.
        let ideal_groups = count.div_ceil(self.workgroup);
        let groups = ideal_groups.min(self.factory.limits.max_workgroups_per_dimension);
        let stride = groups
            .checked_mul(self.workgroup)
            .ok_or(ComputeError::IndexOverflow {
                what: "activation dispatch stride",
                value: groups as usize,
            })?;

        // Zero elements: a zero-extent dispatch is invalid, and there is
        // nothing to compute. Recorded here, skipped in `encode`.
        let empty = count == 0;

        let kernel = if in_place {
            KernelId::ActivationInPlace(self.kind.entry())
        } else {
            KernelId::ActivationOutOfPlace(self.kind.entry())
        };
        let bundle = self
            .factory
            .cache
            .bundle(self.factory.device, kernel, self.workgroup, 1)?;

        let params = ActivationParams {
            count,
            alpha: self.kind.alpha(),
            stride,
            _pad1: 0,
        };
        let params_buffer = self.factory.uniform("activation.params", &params);

        let bind_group = if in_place {
            self.factory
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("activation.bind_group.in_place"),
                    layout: bundle.bind_group_layout(),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: output.buffer().as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: params_buffer.as_entire_binding(),
                        },
                    ],
                })
        } else {
            self.factory
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("activation.bind_group"),
                    layout: bundle.bind_group_layout(),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: input.buffer().as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: output.buffer().as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: params_buffer.as_entire_binding(),
                        },
                    ],
                })
        };

        let label = self
            .label
            .unwrap_or_else(|| format!("activation.{}", self.kind.as_str()));

        Ok(PreparedActivation {
            _operands: core::marker::PhantomData,
            _scope: core::marker::PhantomData,
            dispatch: PreparedDispatch::new(
                Arc::clone(bundle.pipeline()),
                bind_group,
                params_buffer,
                [groups, 1, 1],
                empty,
                label,
            ),
            in_place,
            workgroup: self.workgroup,
            kind: self.kind,
            kernel,
            elements,
        })
    }
}

/// A validated, repeatedly dispatchable elementwise activation.
#[derive(Debug)]
pub struct PreparedActivation<'a, S: EncodeScope = Untracked> {
    /// Ties this operation to the borrows of its operands; see
    /// [`crate::PreparedSpmm`] for the argument.
    _operands: core::marker::PhantomData<&'a ()>,
    _scope: core::marker::PhantomData<S>,
    /// Pipeline, bind group, params buffer, extent and label — shared with
    /// every other prepared operation.
    dispatch: PreparedDispatch,
    in_place: bool,
    workgroup: u32,
    kind: ActivationKind,
    kernel: KernelId,
    elements: usize,
}

impl<S: EncodeScope> PreparedActivation<'_, S> {
    /// Record the dispatch. Does **not** submit.
    ///
    /// Safe to encode after a [`crate::PreparedSpmm`] into the same encoder:
    /// wgpu inserts the barrier between compute passes, so the activation reads
    /// what the SpMM wrote.
    ///
    /// # Errors
    /// Currently infallible; the `Result` keeps the operation contract uniform.
    fn encode_into(&self, encoder: &mut wgpu::CommandEncoder) -> Result<(), ComputeError> {
        self.dispatch.encode_into(encoder, None)
    }

    /// Whether this runs in place (input and output are the same buffer).
    #[must_use]
    pub const fn is_in_place(&self) -> bool {
        self.in_place
    }

    /// Workgroups dispatched — `0` when there is nothing to do.
    #[must_use]
    pub const fn dispatch_groups(&self) -> u32 {
        self.dispatch.groups()[0]
    }

    #[must_use]
    pub const fn workgroup(&self) -> u32 {
        self.workgroup
    }

    #[must_use]
    pub const fn kind(&self) -> ActivationKind {
        self.kind
    }

    /// Which kernel this operation compiled to.
    ///
    /// Replaces the former `pipeline_key()`; see [`crate::PreparedSpmm::kernel`].
    #[must_use]
    pub const fn kernel(&self) -> KernelId {
        self.kernel
    }
    #[must_use]
    pub const fn elements(&self) -> usize {
        self.elements
    }
}

// ---------------------------------------------------------------------------
// encode, split by scope — see the same block in `spmm.rs` for the argument
// ---------------------------------------------------------------------------

impl PreparedActivation<'_, Untracked> {
    /// Record the dispatch into `encoder`. Does **not** submit.
    ///
    /// Available only when every operand is caller-owned.
    ///
    /// # Errors
    /// Currently infallible; the `Result` is part of the operation contract.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) -> Result<(), ComputeError> {
        self.encode_into(encoder)
    }
}

impl PreparedActivation<'_, Tracked> {
    /// Record the dispatch into a tracked session. Does **not** submit.
    ///
    /// Required because at least one operand is a pooled [`crate::ArenaTensor`].
    ///
    /// # Errors
    /// Currently infallible; the `Result` is part of the operation contract.
    pub fn encode(&self, session: &mut crate::TrackedSession<'_>) -> Result<(), ComputeError> {
        self.encode_into(session.note_encoded())
    }
}
