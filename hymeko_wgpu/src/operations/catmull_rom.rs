//! Catmull-Rom membership over GPU-resident, validated operands: the forward and the pullback of a
//! per-channel trainable Catmull-Rom spline over `x [rows, channels]` with control points
//! `[channels, grid]` (plan 2026-09-28-p47-device-training-request, Amendment 3, K1).
//!
//! The mathematical owner is `hymeko_spline::catmull_rom` in `tensor`. The kernels reproduce its
//! stencil through `catmull_rom_formulas.wgsl` (the one device definition, shared with the HSiKAN
//! kernel), its input gradient (zero outside the clamp interval) and its coefficient gradient
//! (accumulated everywhere). The coefficient gradient is summed per chunk of rows and then over the
//! chunks in order: deterministic, with one writer per partial and no float atomics, but in another
//! order than the CPU's per-segment sums, so parity is under a declared bound, not bit for bit.

use hymeko_tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use super::OperandCheck;
use crate::GpuTensor;
use crate::operations::dispatch::PreparedDispatch;
use crate::pipeline::KernelId;

/// WGSL source owned by [`KernelId::CatmullRomForward`], composed with the shared stencil.
pub const CATMULL_ROM_FORWARD_SHADER: &str = concat!(
    include_str!("../../shaders/catmull_rom_formulas.wgsl"),
    include_str!("../../shaders/catmull_rom_forward.wgsl")
);

/// WGSL source owned by [`KernelId::CatmullRomPullback`], composed with the shared stencil.
pub const CATMULL_ROM_PULLBACK_SHADER: &str = concat!(
    include_str!("../../shaders/catmull_rom_formulas.wgsl"),
    include_str!("../../shaders/catmull_rom_pullback.wgsl")
);

/// WGSL source owned by [`KernelId::CatmullRomReduce`].
pub const CATMULL_ROM_REDUCE_SHADER: &str = include_str!("../../shaders/catmull_rom_reduce.wgsl");

/// Default one-dimensional workgroup for the three membership passes.
pub const DEFAULT_CATMULL_ROM_WORKGROUP: u32 = 128;

/// Default rows per pullback unit: one invocation sums this many rows of one channel.
pub const DEFAULT_CATMULL_ROM_CHUNK_ROWS: u32 = 32;

/// The smallest grid the operator is certified for, as on the CPU.
pub const MIN_CATMULL_ROM_GRID: usize = 3;

/// The largest grid the pullback's private accumulator holds (`MAX_GRID` in the shader).
pub const MAX_CATMULL_ROM_GRID: usize = 32;

/// Lanes of the reduction's workgroup (`LANES` in `catmull_rom_reduce.wgsl`): one workgroup a slot.
const REDUCE_LANES: u32 = 128;

uniform_params! {
    ForwardParams(16) {
        count: u32,
        channels: u32,
        grid: u32,
        stride: u32,
    }
}

uniform_params! {
    PullbackParams(32) {
        rows: u32,
        channels: u32,
        grid: u32,
        chunk_rows: u32,
        count: u32,
        stride: u32,
        pad: [u32; 2],
    }
}

uniform_params! {
    ReduceParams(32) {
        channels: u32,
        grid: u32,
        chunks: u32,
        count: u32,
        stride: u32,
        pad: [u32; 3],
    }
}

/// The shape every membership pass validates: `x [rows, channels]`, control points
/// `[channels, grid]`.
#[derive(Clone, Copy, Debug)]
struct MembershipShape {
    rows: usize,
    channels: usize,
    grid: usize,
}

impl MembershipShape {
    /// The shape of `input` against `control`.
    ///
    /// # Errors
    /// [`ComputeError::ShapeMismatch`] for an empty input, control points not one row per channel,
    /// or a grid outside `MIN_CATMULL_ROM_GRID..=MAX_CATMULL_ROM_GRID`.
    fn of(input: &GpuTensor<f32>, control: &GpuTensor<f32>) -> Result<Self, ComputeError> {
        let TensorShape {
            rows,
            cols: channels,
        } = input.shape();
        if rows == 0 || channels == 0 {
            return OperandCheck("catmull_rom input must be non-empty").mismatch(1, 0);
        }
        let TensorShape {
            rows: control_rows,
            cols: grid,
        } = control.shape();
        if control_rows != channels {
            return OperandCheck("catmull_rom control rows, one per channel")
                .mismatch(channels, control_rows);
        }
        if !(MIN_CATMULL_ROM_GRID..=MAX_CATMULL_ROM_GRID).contains(&grid) {
            return OperandCheck("catmull_rom grid, 3 to 32 control points")
                .mismatch(MAX_CATMULL_ROM_GRID, grid);
        }
        Ok(Self {
            rows,
            channels,
            grid,
        })
    }

    /// `tensor` holds this shape's `[rows, channels]`.
    fn require_input_shape(
        self,
        operand: &'static str,
        tensor: &GpuTensor<f32>,
    ) -> Result<(), ComputeError> {
        self.require(operand, tensor, TensorShape::new(self.rows, self.channels))
    }

    fn require(
        self,
        operand: &'static str,
        tensor: &GpuTensor<f32>,
        shape: TensorShape,
    ) -> Result<(), ComputeError> {
        OperandCheck(operand).shape(tensor, shape)
    }

    /// Elements of `x`, `rows · channels`.
    fn elements(self) -> Result<u32, ComputeError> {
        let elements = self
            .rows
            .checked_mul(self.channels)
            .ok_or(ComputeError::IndexOverflow {
                what: "catmull_rom element count",
                value: usize::MAX,
            })?;
        try_u32("catmull_rom element count", elements)
    }
}

/// Builds one validated Catmull-Rom membership forward: `output[e] = S_c(input[e])`.
pub struct CatmullRomForwardBuilder<'a>(CatmullRomForwardFields<'a>);

operation_builder! {
    CatmullRomForwardBuilder(CatmullRomForwardFields, "catmull_rom") {
        /// The membership's input, `[rows, channels]`.
        input: GpuTensor<f32>,
        /// Each channel's control points, `[channels, grid]`.
        control: GpuTensor<f32>,
        /// The output, the input's shape.
        output: GpuTensor<f32>,
    }
    workgroup: DEFAULT_CATMULL_ROM_WORKGROUP;
}

impl<'a> CatmullRomForwardBuilder<'a> {
    /// Validate every operand and prepare a repeatedly encodable dispatch.
    ///
    /// # Errors
    /// [`ComputeError::ShapeMismatch`] for operands that disagree or a grid outside
    /// `MIN_CATMULL_ROM_GRID..=MAX_CATMULL_ROM_GRID`; [`ComputeError::IndexOverflow`] for a count
    /// that does not fit `u32`; [`ComputeError::Backend`] for a device that cannot satisfy the
    /// layout or the workgroup.
    pub fn prepare(self) -> Result<PreparedCatmullRomForward<'a>, ComputeError> {
        let (input, control, output) = self.operands()?;
        let shape = MembershipShape::of(input, control)?;
        shape.require_input_shape("catmull_rom output shape", output)?;
        let count = shape.elements()?;
        let dispatch = self.0.factory.grid_dispatch(
            KernelId::CatmullRomForward,
            self.0.workgroup,
            count,
            |stride| {
                Ok(ForwardParams {
                    count,
                    channels: try_u32("catmull_rom channels", shape.channels)?,
                    grid: try_u32("catmull_rom grid", shape.grid)?,
                    stride,
                })
            },
            &[input.binding(0), control.binding(1), output.binding(2)],
            self.0.label,
        )?;
        Ok(PreparedCatmullRomForward::new(
            dispatch,
            KernelId::CatmullRomForward,
            count as usize,
        ))
    }
}

/// A validated, repeatedly dispatchable Catmull-Rom membership forward.
#[derive(Debug)]
pub struct PreparedCatmullRomForward<'a>(super::PreparedOne<'a>);

prepared_operation!(PreparedCatmullRomForward);

/// Builds one validated Catmull-Rom membership pullback: the input gradient and the
/// control-point gradient under the output's cotangent.
pub struct CatmullRomPullbackBuilder<'a>(CatmullRomPullbackFields<'a>);

operation_builder! {
    CatmullRomPullbackBuilder(CatmullRomPullbackFields, "catmull_rom") {
        /// The forward's input, `[rows, channels]`.
        input: GpuTensor<f32>,
        /// Each channel's control points, `[channels, grid]`.
        control: GpuTensor<f32>,
        /// The output's cotangent, the input's shape.
        grad_output: GpuTensor<f32>,
        /// The input gradient, the input's shape; every element is written.
        grad_input: GpuTensor<f32>,
        /// The control-point gradient, `[channels, grid]`; every element is written.
        grad_control: GpuTensor<f32>,
    }
    settings {
        chunk_rows: u32 = DEFAULT_CATMULL_ROM_CHUNK_ROWS,
    }
    workgroup: DEFAULT_CATMULL_ROM_WORKGROUP;
}

impl<'a> CatmullRomPullbackBuilder<'a> {
    /// Rows each pullback invocation sums for one channel; at least one.
    #[must_use]
    pub fn chunk_rows(mut self, chunk_rows: u32) -> Self {
        self.0.chunk_rows = chunk_rows;
        self
    }

    /// Validate every operand and prepare the two repeatedly encodable dispatches: the per-chunk
    /// pass and the reduction over the chunks.
    ///
    /// # Errors
    /// As [`CatmullRomForwardBuilder::prepare`], and [`ComputeError::ShapeMismatch`] for a chunk of
    /// zero rows.
    pub fn prepare(self) -> Result<PreparedCatmullRomPullback<'a>, ComputeError> {
        let (input, control, grad_output, grad_input, grad_control) = self.operands()?;
        let shape = MembershipShape::of(input, control)?;
        shape.require_input_shape("catmull_rom grad_output shape", grad_output)?;
        shape.require_input_shape("catmull_rom grad_input shape", grad_input)?;
        shape.require(
            "catmull_rom grad_control shape",
            grad_control,
            control.shape(),
        )?;
        if self.0.chunk_rows == 0 {
            return OperandCheck("catmull_rom chunk rows").mismatch(1, 0);
        }
        self.0
            .factory
            .require_device(KernelId::CatmullRomPullback, self.0.workgroup)?;

        let rows = try_u32("catmull_rom rows", shape.rows)?;
        let channels = try_u32("catmull_rom channels", shape.channels)?;
        let grid = try_u32("catmull_rom grid", shape.grid)?;
        let chunks = rows.div_ceil(self.0.chunk_rows);
        let units = chunks
            .checked_mul(channels)
            .ok_or(ComputeError::IndexOverflow {
                what: "catmull_rom pullback units",
                value: usize::MAX,
            })?;
        let slots = channels * grid;
        let partials = self
            .0
            .factory
            .device
            .create_buffer(&wgpu::BufferDescriptor {
                label: Some("catmull_rom.partials"),
                size: u64::from(units) * u64::from(grid) * 4,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
        let label = self.0.label.unwrap_or_else(|| "catmull_rom".to_owned());

        let (groups, stride) = self.0.factory.grid_stride(self.0.workgroup, units)?;
        let pullback = self.0.factory.dispatch(
            KernelId::CatmullRomPullback,
            self.0.workgroup,
            groups,
            &PullbackParams {
                rows,
                channels,
                grid,
                chunk_rows: self.0.chunk_rows,
                count: units,
                stride,
                pad: [0; 2],
            },
            &[
                input.binding(0),
                control.binding(1),
                grad_output.binding(2),
                grad_input.binding(3),
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: partials.as_entire_binding(),
                },
            ],
            format!("{label}.pullback"),
        )?;
        // One workgroup a slot, its stride counted in workgroups.
        let groups = slots.min(self.0.factory.limits.max_workgroups_per_dimension);
        let stride = groups;
        if self.0.factory.limits.max_invocations_per_workgroup < REDUCE_LANES {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "catmull_rom reduction needs {REDUCE_LANES} invocations a workgroup, device \
                     exposes {}",
                    self.0.factory.limits.max_invocations_per_workgroup
                ),
            });
        }
        let reduce = self.0.factory.dispatch(
            KernelId::CatmullRomReduce,
            REDUCE_LANES,
            groups,
            &ReduceParams {
                channels,
                grid,
                chunks,
                count: slots,
                stride,
                pad: [0; 3],
            },
            &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: partials.as_entire_binding(),
                },
                grad_control.binding(1),
            ],
            format!("{label}.reduce"),
        )?;
        Ok(PreparedCatmullRomPullback {
            _operands: core::marker::PhantomData,
            pullback,
            reduce,
            _partials: partials,
            chunks,
        })
    }
}

/// A validated, repeatedly dispatchable Catmull-Rom membership pullback: two passes and the
/// partials buffer they share.
#[derive(Debug)]
pub struct PreparedCatmullRomPullback<'a> {
    _operands: core::marker::PhantomData<&'a ()>,
    pullback: PreparedDispatch,
    reduce: PreparedDispatch,
    /// Held for the two bind groups that borrow it.
    _partials: wgpu::Buffer,
    chunks: u32,
}

impl PreparedCatmullRomPullback<'_> {
    /// Record both passes, the per-chunk pass first, without submitting them.
    ///
    /// # Errors
    /// Whatever the underlying dispatches report.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) -> Result<(), ComputeError> {
        self.pullback.encode_into(encoder, None)?;
        self.reduce.encode_into(encoder, None)
    }

    /// The kernels this operation dispatches, in order.
    #[must_use]
    pub const fn kernels(&self) -> [KernelId; 2] {
        [KernelId::CatmullRomPullback, KernelId::CatmullRomReduce]
    }

    /// The row chunks the coefficient gradient is summed over.
    #[must_use]
    pub const fn chunks(&self) -> u32 {
        self.chunks
    }
}
