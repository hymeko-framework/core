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

use std::sync::Arc;

use hymeko_tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use super::OperandCheck;
use super::OperationFactory;
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

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct ForwardParams {
    count: u32,
    channels: u32,
    grid: u32,
    stride: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PullbackParams {
    rows: u32,
    channels: u32,
    grid: u32,
    chunk_rows: u32,
    count: u32,
    stride: u32,
    pad: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct ReduceParams {
    channels: u32,
    grid: u32,
    chunks: u32,
    count: u32,
    stride: u32,
    pad: [u32; 3],
}

const _: () = assert!(std::mem::size_of::<ForwardParams>() == 16);
const _: () = assert!(std::mem::size_of::<PullbackParams>() == 32);
const _: () = assert!(std::mem::size_of::<ReduceParams>() == 32);

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
        if tensor.shape() == shape {
            Ok(())
        } else {
            OperandCheck(operand).mismatch(
                shape.checked_elements().unwrap_or(usize::MAX),
                tensor.shape().checked_elements().unwrap_or(usize::MAX),
            )
        }
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

impl OperationFactory<'_> {
    /// The workgroup and storage-buffer limits a membership pass needs, checked against the device.
    fn membership_device(&self, workgroup: u32, storage: u32) -> Result<(), ComputeError> {
        if self.limits.max_storage_buffers_per_stage < storage {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "catmull_rom requires {storage} storage buffers, device exposes {}",
                    self.limits.max_storage_buffers_per_stage
                ),
            });
        }
        if workgroup == 0 || workgroup > self.limits.max_invocations_per_workgroup {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "catmull_rom workgroup {workgroup} outside 1..={}",
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
                what: "catmull_rom dispatch stride",
                value: groups as usize,
            })?;
        Ok((groups, stride))
    }

    /// A dispatch of `kernel` over `entries` (the params uniform bound last, at `entries.len()`).
    fn membership_dispatch<P: bytemuck::Pod>(
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

/// Builds one validated Catmull-Rom membership forward: `output[e] = S_c(input[e])`.
pub struct CatmullRomForwardBuilder<'a> {
    factory: OperationFactory<'a>,
    input: Option<&'a GpuTensor<f32>>,
    control: Option<&'a GpuTensor<f32>>,
    output: Option<&'a GpuTensor<f32>>,
    workgroup: u32,
    label: Option<String>,
}

impl<'a> CatmullRomForwardBuilder<'a> {
    pub(crate) fn new(factory: OperationFactory<'a>) -> Self {
        Self {
            factory,
            input: None,
            control: None,
            output: None,
            workgroup: DEFAULT_CATMULL_ROM_WORKGROUP,
            label: None,
        }
    }

    /// The membership's input, `[rows, channels]`.
    #[must_use]
    pub fn input(mut self, input: &'a GpuTensor<f32>) -> Self {
        self.input = Some(input);
        self
    }

    /// Each channel's control points, `[channels, grid]`.
    #[must_use]
    pub fn control(mut self, control: &'a GpuTensor<f32>) -> Self {
        self.control = Some(control);
        self
    }

    /// The output, the input's shape.
    #[must_use]
    pub fn output(mut self, output: &'a GpuTensor<f32>) -> Self {
        self.output = Some(output);
        self
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

    /// Validate every operand and prepare a repeatedly encodable dispatch.
    ///
    /// # Errors
    /// [`ComputeError::ShapeMismatch`] for operands that disagree or a grid outside
    /// `MIN_CATMULL_ROM_GRID..=MAX_CATMULL_ROM_GRID`; [`ComputeError::IndexOverflow`] for a count
    /// that does not fit `u32`; [`ComputeError::Backend`] for a device that cannot satisfy the
    /// layout or the workgroup.
    pub fn prepare(self) -> Result<PreparedCatmullRomForward<'a>, ComputeError> {
        let input = OperandCheck("catmull_rom input").supplied(self.input)?;
        let control = OperandCheck("catmull_rom control").supplied(self.control)?;
        let output = OperandCheck("catmull_rom output").supplied(self.output)?;
        let shape = MembershipShape::of(input, control)?;
        shape.require_input_shape("catmull_rom output shape", output)?;
        self.factory.membership_device(self.workgroup, 3)?;
        let count = shape.elements()?;
        let (groups, stride) = self.factory.grid_stride(self.workgroup, count)?;
        let params = ForwardParams {
            count,
            channels: try_u32("catmull_rom channels", shape.channels)?,
            grid: try_u32("catmull_rom grid", shape.grid)?,
            stride,
        };
        let dispatch = self.factory.membership_dispatch(
            KernelId::CatmullRomForward,
            self.workgroup,
            groups,
            &params,
            &[input.binding(0), control.binding(1), output.binding(2)],
            self.label
                .unwrap_or_else(|| "catmull_rom_forward".to_owned()),
        )?;
        Ok(PreparedCatmullRomForward {
            _operands: core::marker::PhantomData,
            dispatch,
            elements: count as usize,
        })
    }
}

/// A validated, repeatedly dispatchable Catmull-Rom membership forward.
#[derive(Debug)]
pub struct PreparedCatmullRomForward<'a> {
    _operands: core::marker::PhantomData<&'a ()>,
    dispatch: PreparedDispatch,
    elements: usize,
}

impl PreparedCatmullRomForward<'_> {
    /// Record the dispatch without submitting it.
    ///
    /// # Errors
    /// Whatever the underlying dispatch reports.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) -> Result<(), ComputeError> {
        self.dispatch.encode_into(encoder, None)
    }

    /// The kernel this operation dispatches.
    #[must_use]
    pub const fn kernel(&self) -> KernelId {
        KernelId::CatmullRomForward
    }

    /// Output element count, `rows · channels`.
    #[must_use]
    pub const fn elements(&self) -> usize {
        self.elements
    }
}

/// Builds one validated Catmull-Rom membership pullback: the input gradient and the control-point
/// gradient under the output's cotangent.
pub struct CatmullRomPullbackBuilder<'a> {
    factory: OperationFactory<'a>,
    input: Option<&'a GpuTensor<f32>>,
    control: Option<&'a GpuTensor<f32>>,
    grad_output: Option<&'a GpuTensor<f32>>,
    grad_input: Option<&'a GpuTensor<f32>>,
    grad_control: Option<&'a GpuTensor<f32>>,
    workgroup: u32,
    chunk_rows: u32,
    label: Option<String>,
}

impl<'a> CatmullRomPullbackBuilder<'a> {
    pub(crate) fn new(factory: OperationFactory<'a>) -> Self {
        Self {
            factory,
            input: None,
            control: None,
            grad_output: None,
            grad_input: None,
            grad_control: None,
            workgroup: DEFAULT_CATMULL_ROM_WORKGROUP,
            chunk_rows: DEFAULT_CATMULL_ROM_CHUNK_ROWS,
            label: None,
        }
    }

    /// The forward's input, `[rows, channels]`.
    #[must_use]
    pub fn input(mut self, input: &'a GpuTensor<f32>) -> Self {
        self.input = Some(input);
        self
    }

    /// Each channel's control points, `[channels, grid]`.
    #[must_use]
    pub fn control(mut self, control: &'a GpuTensor<f32>) -> Self {
        self.control = Some(control);
        self
    }

    /// The output's cotangent, the input's shape.
    #[must_use]
    pub fn grad_output(mut self, grad_output: &'a GpuTensor<f32>) -> Self {
        self.grad_output = Some(grad_output);
        self
    }

    /// The input gradient, the input's shape; every element is written.
    #[must_use]
    pub fn grad_input(mut self, grad_input: &'a GpuTensor<f32>) -> Self {
        self.grad_input = Some(grad_input);
        self
    }

    /// The control-point gradient, `[channels, grid]`; every element is written.
    #[must_use]
    pub fn grad_control(mut self, grad_control: &'a GpuTensor<f32>) -> Self {
        self.grad_control = Some(grad_control);
        self
    }

    #[must_use]
    pub fn workgroup(mut self, workgroup: u32) -> Self {
        self.workgroup = workgroup;
        self
    }

    /// Rows each pullback invocation sums for one channel; at least one.
    #[must_use]
    pub fn chunk_rows(mut self, chunk_rows: u32) -> Self {
        self.chunk_rows = chunk_rows;
        self
    }

    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Validate every operand and prepare the two repeatedly encodable dispatches: the per-chunk
    /// pass and the reduction over the chunks.
    ///
    /// # Errors
    /// As [`CatmullRomForwardBuilder::prepare`], and [`ComputeError::ShapeMismatch`] for a chunk of
    /// zero rows.
    pub fn prepare(self) -> Result<PreparedCatmullRomPullback<'a>, ComputeError> {
        let input = OperandCheck("catmull_rom input").supplied(self.input)?;
        let control = OperandCheck("catmull_rom control").supplied(self.control)?;
        let grad_output = OperandCheck("catmull_rom grad_output").supplied(self.grad_output)?;
        let grad_input = OperandCheck("catmull_rom grad_input").supplied(self.grad_input)?;
        let grad_control = OperandCheck("catmull_rom grad_control").supplied(self.grad_control)?;
        let shape = MembershipShape::of(input, control)?;
        shape.require_input_shape("catmull_rom grad_output shape", grad_output)?;
        shape.require_input_shape("catmull_rom grad_input shape", grad_input)?;
        shape.require(
            "catmull_rom grad_control shape",
            grad_control,
            control.shape(),
        )?;
        if self.chunk_rows == 0 {
            return OperandCheck("catmull_rom chunk rows").mismatch(1, 0);
        }
        self.factory.membership_device(self.workgroup, 5)?;

        let rows = try_u32("catmull_rom rows", shape.rows)?;
        let channels = try_u32("catmull_rom channels", shape.channels)?;
        let grid = try_u32("catmull_rom grid", shape.grid)?;
        let chunks = rows.div_ceil(self.chunk_rows);
        let units = chunks
            .checked_mul(channels)
            .ok_or(ComputeError::IndexOverflow {
                what: "catmull_rom pullback units",
                value: usize::MAX,
            })?;
        let slots = channels * grid;
        let partials = self.factory.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("catmull_rom.partials"),
            size: u64::from(units) * u64::from(grid) * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let label = self.label.unwrap_or_else(|| "catmull_rom".to_owned());

        let (groups, stride) = self.factory.grid_stride(self.workgroup, units)?;
        let pullback = self.factory.membership_dispatch(
            KernelId::CatmullRomPullback,
            self.workgroup,
            groups,
            &PullbackParams {
                rows,
                channels,
                grid,
                chunk_rows: self.chunk_rows,
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
        let groups = slots.min(self.factory.limits.max_workgroups_per_dimension);
        let stride = groups;
        if self.factory.limits.max_invocations_per_workgroup < REDUCE_LANES {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "catmull_rom reduction needs {REDUCE_LANES} invocations a workgroup, device \
                     exposes {}",
                    self.factory.limits.max_invocations_per_workgroup
                ),
            });
        }
        let reduce = self.factory.membership_dispatch(
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
