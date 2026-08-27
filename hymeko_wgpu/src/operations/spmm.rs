//! CSR × dense SpMM: `Y = A · X`.
//!
//! # Lifecycle
//!
//! ```text
//!   backend.operations().spmm()      OperationFactory  -> SpmmBuilder
//!       .matrix(&a).input(&x).output(&y)
//!       .workgroup(WorkgroupShape::default())
//!       .prepare()?                  <- ALL validation happens here
//!   -> PreparedSpmm                     owns pipeline Arc + bind group
//!       .encode(&mut encoder)?       <- hot path: no validation, no allocation
//!   backend.submit(encoder)          <- explicit, batches several operations
//!   backend.read_tensor(&y)?         <- explicit readback boundary
//! ```
//!
//! `prepare()` is where every shape, limit and aliasing rule is checked, so
//! `encode()` reduces to "set pipeline, set bind group, dispatch". A
//! `PreparedSpmm` is re-dispatchable any number of times, including after the
//! input tensor's contents are rewritten in place, because the bind group binds
//! *buffers*, not contents.

use std::sync::Arc;

use hymeko::tensor::compute::{BackendKind, ComputeError, SparseCsrShape, TensorShape, try_u32};

use crate::operations::activation::ActivationKind;
use crate::operations::dispatch::PreparedDispatch;
use crate::pipeline::KernelId;
use crate::resources::{GpuCsrMatrix, GpuTensor};
use crate::scope::{EncodeScope, JoinScope, Joined, TensorOperand, Tracked, Untracked};

/// WGSL source for the SpMM kernel, raw and fused.
///
/// One module. `activation_formulas.wgsl` is prepended to `spmm.wgsl` by
/// compile-time concatenation, so every activation formula has a single
/// definition shared with the elementwise kernels, and `spmm_acc` has a single
/// definition shared by every entry point. There is no runtime shader-string
/// assembly: both halves are string literals fixed in the binary.
pub const SPMM_SHADER: &str = concat!(
    include_str!("../../shaders/activation_formulas.wgsl"),
    include_str!("../../shaders/spmm.wgsl")
);

/// Workgroup extent for the SpMM kernel.
///
/// `x` indexes the feature dimension, `y` the row dimension — matching the
/// kernel's invocation mapping. Which extent is fastest is a **measured**
/// question; the brief calls for 8×8, 16×8 and 16×16, and nothing here presumes
/// a winner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkgroupShape {
    pub x: u32,
    pub y: u32,
}

impl WorkgroupShape {
    #[must_use]
    pub const fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }

    /// The three configurations the phase brief requires measuring.
    #[must_use]
    pub const fn sweep() -> [Self; 3] {
        [Self::new(8, 8), Self::new(16, 8), Self::new(16, 16)]
    }

    #[must_use]
    pub const fn invocations(&self) -> u32 {
        self.x * self.y
    }
}

impl Default for WorkgroupShape {
    /// 16×8 = 128 invocations. Retained as the shape a caller gets when it asks
    /// for one without saying anything about the workload.
    ///
    /// Prefer [`spmm_workgroup_for`], which the builder applies automatically:
    /// the measured best shape depends on the feature dimension, so a single
    /// default is a compromise by construction.
    fn default() -> Self {
        Self::new(16, 8)
    }
}

/// Pick the SpMM workgroup shape for a feature dimension — **from measurement**.
///
/// # Why the feature dimension decides this
///
/// The kernel maps `gid.x` to the feature axis, so a workgroup 16 wide in `x`
/// puts 16 lanes on features. When `feature_dim < x`, the surplus lanes are
/// masked off and the workgroup runs at a fraction of its occupancy: at
/// `feature_dim = 1`, a 16-wide shape wastes 15 of every 16 lanes. Narrow-`x`
/// therefore wins for thin feature dimensions and loses for wide ones, where
/// the contiguous X reads a wide wave gets are worth more.
///
/// # Measured data (RTX 5080 Laptop / Vulkan, 2026-08-25)
///
/// Two problem sizes, because a rule fitted to one of them is not a rule. Times
/// in µs; **bold** is that row's winner.
///
/// | `feature_dim` | med 8×8 | med 16×8 | med 16×16 | lg 8×8 | lg 16×8 | lg 16×16 |
/// |---:|---:|---:|---:|---:|---:|---:|
/// | 1 | **100.4** | 113.2 | 125.2 | **429.8** | 511.6 | 510.8 |
/// | 4 | **114.5** | 124.3 | 127.3 | **435.7** | 521.2 | 579.5 |
/// | 8 | **119.2** | 130.1 | 131.6 | **513.7** | 611.7 | 655.1 |
/// | 16 | 145.5 | 134.7 | **123.0** | 927.1 | **675.7** | 718.3 |
/// | 32 | 189.8 | **176.2** | 179.0 | 1721.7 | **1197.3** | 1290.4 |
/// | 64 | 337.8 | 275.9 | **272.4** | 3595.2 | **2456.9** | 2649.1 |
/// | 128 | 651.0 | 540.2 | **531.5** | 8828.7 | **6035.0** | 7029.0 |
///
/// # Why the rule is only two arms
///
/// A first version of this rule was fitted to the medium spec alone and read
/// `feature_dim >= 32 → 16×16`. Measured against the large spec it was **16.5 %
/// slower at `feature_dim` 128** (7029 vs 6035 µs). Extrapolating a workgroup
/// shape from one problem size was the error; the data above is what corrected
/// it.
///
/// What survives across *both* sizes and *both* runs of the sweep:
///
/// * **`feature_dim < 16` → 8×8**, decisively: 6–20 % ahead everywhere, and the
///   only regime where a 16-wide shape idles most of its `x` lanes.
/// * **`feature_dim >= 16` → 16×8.** On large it wins by 6–16 %. On medium it
///   loses to 16×16 by at most 1.6 % — and 16×16's medium lead is not stable:
///   across two runs the winner at `feature_dim` 16 and 32 flipped, with margins
///   inside the ~3 % confidence intervals this host produces.
///
/// Trading ≤1.6 % on medium for 16.5 % on large is the whole argument. 16×16
/// remains reachable through [`SpmmBuilder::workgroup`] and is still swept.
///
/// # What is interpolated, and what is measured
///
/// Only `feature_dim` ∈ {1, 4, 8, 16, 32, 64, 128} was measured. The ranges
/// 2–3, 5–7 and 9–15 are **interpolated** from their measured neighbours. The
/// single boundary (16) sits exactly on a measured point, so the interpolation
/// cannot move a winner.
///
/// Only two problem sizes were measured. The rule takes no row-count argument
/// because the two arms above agree on both; if a third size disagreed, this
/// function's signature — not its constants — would be what needs revisiting.
///
/// # Device-limit fallback
///
/// `max_invocations_per_workgroup` is a device limit (1024 on the reference
/// adapter; the WebGPU downlevel floor is 256). If the measured choice exceeds
/// it, the shape steps down through the sweep rather than failing — a small
/// device should run slower, not refuse.
///
/// # Preconditions
///
/// `max_invocations_per_workgroup >= 1`. A device reporting 0 is not a device
/// this backend can use; `prepare()` rejects that case separately.
///
/// # Postconditions
///
/// The returned shape has `invocations() <= max_invocations_per_workgroup`
/// whenever that limit is at least 1, and both extents are non-zero.
///
/// # Determinism
///
/// A pure function of its two arguments. No global state, no device query, no
/// caching, no interior mutability — so it is unit-testable without a GPU, and
/// the same inputs give the same pipeline-cache key on every host.
#[must_use]
pub const fn spmm_workgroup_for(
    feature_dim: usize,
    max_invocations_per_workgroup: u32,
) -> WorkgroupShape {
    let measured = if feature_dim < 16 {
        WorkgroupShape::new(8, 8)
    } else {
        WorkgroupShape::new(16, 8)
    };

    // Step down, largest first, to the widest shape the device admits.
    if measured.invocations() <= max_invocations_per_workgroup {
        measured
    } else if 64 <= max_invocations_per_workgroup {
        WorkgroupShape::new(8, 8)
    } else if 16 <= max_invocations_per_workgroup {
        WorkgroupShape::new(4, 4)
    } else {
        WorkgroupShape::new(1, 1)
    }
}

impl core::fmt::Display for WorkgroupShape {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}x{}", self.x, self.y)
    }
}

/// Uniform parameter block. Mirrors `struct Params` in `spmm.wgsl`.
///
/// # Layout
/// Four `u32` = 16 bytes, which satisfies the uniform address space's 16-byte
/// struct alignment with no implicit padding. The fourth field is explicit so
/// the Rust and WGSL definitions cannot drift.
///
/// **No `usize` field** — every value arrives through the checked
/// [`try_u32`] conversion.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct SpmmParams {
    num_rows: u32,
    num_cols: u32,
    feature_dim: u32,
    /// LeakyReLU slope for the fused kernel. Occupies the tail-padding slot the
    /// unfused `spmm.wgsl` declares as `_pad` and never reads, so one 16-byte
    /// layout serves both shaders. Zero when unfused — identical bytes to the
    /// `_pad: 0` this replaced.
    alpha: f32,
}

const _: () = assert!(std::mem::size_of::<SpmmParams>() == 16);

/// Builds and validates a [`PreparedSpmm`].
///
/// Obtained from [`crate::OperationFactory::spmm`].
pub struct SpmmBuilder<'a, S: EncodeScope = Untracked> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    cache: &'a crate::PipelineCache,
    limits: SpmmLimits,
    matrix: Option<&'a GpuCsrMatrix<f32>>,
    input: Option<&'a GpuTensor<f32>>,
    output: Option<&'a GpuTensor<f32>>,
    /// `None` means "derive from the feature dimension at prepare time" via
    /// [`spmm_workgroup_for`]. `Some` is an explicit caller override.
    workgroup: Option<WorkgroupShape>,
    /// `Some` selects the hand-written fused kernel; `None` the plain SpMM.
    fused_activation: Option<ActivationKind>,
    label: Option<String>,
    /// Provenance of the operands seen so far; see [`crate::scope`].
    _scope: core::marker::PhantomData<S>,
}

/// The device limits SpMM validates against.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpmmLimits {
    pub max_workgroups_per_dimension: u32,
    pub max_invocations_per_workgroup: u32,
    pub max_storage_buffers_per_stage: u32,
}

impl<'a> SpmmBuilder<'a, Untracked> {
    pub(crate) fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        cache: &'a crate::PipelineCache,
        limits: SpmmLimits,
    ) -> Self {
        Self {
            device,
            queue,
            cache,
            limits,
            matrix: None,
            input: None,
            output: None,
            workgroup: None,
            fused_activation: None,
            label: None,
            _scope: core::marker::PhantomData,
        }
    }
}

impl<'a, S: EncodeScope> SpmmBuilder<'a, S> {
    /// Carry every field across a scope change.
    ///
    /// Needed because adding an operand can change the builder's type parameter,
    /// and Rust has no way to mutate a type parameter in place. Zero cost: the
    /// scope is a `PhantomData`, so this is a move of the same layout.
    fn retype<S2: EncodeScope>(self) -> SpmmBuilder<'a, S2> {
        SpmmBuilder {
            device: self.device,
            queue: self.queue,
            cache: self.cache,
            limits: self.limits,
            matrix: self.matrix,
            input: self.input,
            output: self.output,
            workgroup: self.workgroup,
            fused_activation: self.fused_activation,
            label: self.label,
            _scope: core::marker::PhantomData,
        }
    }

    /// The sparse operand `A`.
    #[must_use]
    pub fn matrix(mut self, matrix: &'a GpuCsrMatrix<f32>) -> Self {
        self.matrix = Some(matrix);
        self
    }

    /// The dense operand `X`, shape `[num_cols, feature_dim]`.
    #[must_use]
    pub fn input<T>(mut self, input: &'a T) -> SpmmBuilder<'a, Joined<S, T>>
    where
        T: TensorOperand,
        S: JoinScope<T::Scope>,
    {
        self.input = Some(input.gpu_tensor());
        self.retype()
    }

    /// The dense result `Y`, shape `[num_rows, feature_dim]`.
    #[must_use]
    pub fn output<T>(mut self, output: &'a T) -> SpmmBuilder<'a, Joined<S, T>>
    where
        T: TensorOperand,
        S: JoinScope<T::Scope>,
    {
        self.output = Some(output.gpu_tensor());
        self.retype()
    }

    /// Override the workgroup extent, bypassing [`spmm_workgroup_for`].
    ///
    /// Changes the pipeline, and therefore the cache key. Needed by the
    /// workgroup sweep benchmarks, which must be able to select a shape the
    /// rule would not pick -- otherwise the rule could never be re-measured.
    ///
    /// An override is NOT checked against the rule: a caller who names a shape
    /// gets that shape, subject only to the device limits enforced in
    /// `prepare()`.
    #[must_use]
    pub fn workgroup(mut self, workgroup: WorkgroupShape) -> Self {
        self.workgroup = Some(workgroup);
        self
    }

    /// Fuse an activation into SpMM's store: `Y = f(A · X)` in one dispatch.
    ///
    /// A configuration field rather than a separate `spmm_relu()` entry point —
    /// fusion is one more axis on this operation, and an axis belongs in the
    /// config, not in a new function name.
    ///
    /// # What this changes
    ///
    /// A different shader, hence a different pipeline and cache key. The bind
    /// group, buffer layout and parameter block are unchanged; `alpha` occupies
    /// the slot the unfused kernel pads.
    ///
    /// # Why it is not automatic
    ///
    /// Fusion removes one dispatch and one full read+write of `Y`, so it pays
    /// most where `Y` is large relative to the CSR traversal. Whether it wins is
    /// a property of the workload, not of the operation — see the cell G vs
    /// cell H measurement in `docs/wgpu-backend.md`.
    ///
    /// # Errors
    ///
    /// [`ActivationKind::Identity`] is rejected by `prepare()`: fusing a no-op
    /// is the unfused kernel, and accepting it would cost a second pipeline for
    /// identical behaviour.
    #[must_use]
    pub fn fused_activation(mut self, kind: ActivationKind) -> Self {
        self.fused_activation = Some(kind);
        self
    }

    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Validate everything and build the dispatchable operation.
    ///
    /// # Errors
    /// * [`ComputeError::ShapeMismatch`] — operand missing, or a shape that does
    ///   not satisfy `A[r,c] · X[c,f] = Y[r,f]`;
    /// * [`ComputeError::IndexOverflow`] — an extent, `nnz`, or a dispatch
    ///   dimension that does not fit `u32` or exceeds a device limit;
    /// * [`ComputeError::Backend`] — input and output alias the same buffer, the
    ///   workgroup exceeds the device's invocation limit, or shader compilation
    ///   fails.
    pub fn prepare(self) -> Result<PreparedSpmm<'a, S>, ComputeError> {
        let matrix = self.matrix.ok_or(ComputeError::ShapeMismatch {
            operand: "spmm matrix (not supplied)",
            expected: 1,
            actual: 0,
        })?;
        let input = self.input.ok_or(ComputeError::ShapeMismatch {
            operand: "spmm input (not supplied)",
            expected: 1,
            actual: 0,
        })?;
        let output = self.output.ok_or(ComputeError::ShapeMismatch {
            operand: "spmm output (not supplied)",
            expected: 1,
            actual: 0,
        })?;

        let a: SparseCsrShape = matrix.shape();
        let x: TensorShape = input.shape();
        let y: TensorShape = output.shape();

        // --- shape compatibility -------------------------------------------
        // X must have one row per matrix column.
        if x.rows != a.cols {
            return Err(ComputeError::ShapeMismatch {
                operand: "spmm input rows (must equal matrix columns)",
                expected: a.cols,
                actual: x.rows,
            });
        }
        // Y must have one row per matrix row.
        if y.rows != a.rows {
            return Err(ComputeError::ShapeMismatch {
                operand: "spmm output rows (must equal matrix rows)",
                expected: a.rows,
                actual: y.rows,
            });
        }
        // feature_dim must agree between X and Y.
        if x.cols != y.cols {
            return Err(ComputeError::ShapeMismatch {
                operand: "spmm feature_dim (input vs output)",
                expected: x.cols,
                actual: y.cols,
            });
        }
        let feature_dim = x.cols;

        // --- aliasing -------------------------------------------------------
        // X is bound read-only and Y read_write; binding one buffer as both is
        // a data race the validator may or may not catch, and the result would
        // be silently wrong either way.
        if std::ptr::eq(input.buffer(), output.buffer()) {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: "spmm input and output alias the same buffer".to_owned(),
            });
        }

        // --- u32 limits ------------------------------------------------------
        let num_rows = try_u32("spmm num_rows", a.rows)?;
        let num_cols = try_u32("spmm num_cols", a.cols)?;
        let fd_u32 = try_u32("spmm feature_dim", feature_dim)?;
        let _nnz = try_u32("spmm nnz", a.nnz)?;
        // row_ptr holds nnz values, so its own largest entry must fit u32 too.
        let _row_ptr_len = try_u32("spmm row_ptr length", a.row_ptr_len())?;

        // --- workgroup selection and device limits ---------------------------
        // Resolved here, not in the constructor, because the rule needs the
        // feature dimension -- which is only known once the operands are set.
        let workgroup = self.workgroup.unwrap_or_else(|| {
            spmm_workgroup_for(feature_dim, self.limits.max_invocations_per_workgroup)
        });
        let invocations = workgroup.invocations();
        if invocations == 0 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!("spmm workgroup {workgroup} has zero invocations"),
            });
        }
        if invocations > self.limits.max_invocations_per_workgroup {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "spmm workgroup {} needs {invocations} invocations, device allows {}",
                    workgroup, self.limits.max_invocations_per_workgroup
                ),
            });
        }
        // 5 storage bindings: row_ptr, col_ind, values, x, y.
        if self.limits.max_storage_buffers_per_stage < 5 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "spmm needs 5 storage buffers per stage, device allows {}",
                    self.limits.max_storage_buffers_per_stage
                ),
            });
        }

        // --- dispatch extent --------------------------------------------------
        let groups_x = fd_u32.div_ceil(workgroup.x);
        let groups_y = num_rows.div_ceil(workgroup.y);
        for (dim, groups) in [("x", groups_x), ("y", groups_y)] {
            if groups > self.limits.max_workgroups_per_dimension {
                return Err(ComputeError::IndexOverflow {
                    what: if dim == "x" {
                        "spmm dispatch workgroups (feature axis) exceeds device limit"
                    } else {
                        "spmm dispatch workgroups (row axis) exceeds device limit"
                    },
                    value: groups as usize,
                });
            }
        }

        // --- zero-sized rule --------------------------------------------------
        // A dispatch with a zero extent in any dimension is invalid in WGPU.
        // With no rows or no features there is also nothing to compute, and the
        // output is already correct: `zeroed_tensor` allocates it zeroed and a
        // zero-element Y has no elements to be wrong. `encode` therefore skips
        // the pass entirely, which is recorded here rather than discovered at
        // dispatch time.
        let empty = num_rows == 0 || fd_u32 == 0;

        // --- pipeline ---------------------------------------------------------
        // Kernel identity fully determines source, entry point and layout, so
        // there is nothing here that could be mismatched with the cache key.
        // Bind-group and pipeline layouts are cached per LayoutId and are no
        // longer rebuilt on every prepare().
        let kernel = match self.fused_activation {
            None => KernelId::Spmm,
            Some(ActivationKind::Identity) => {
                return Err(ComputeError::Backend {
                    backend: BackendKind::Wgpu,
                    message: "fused Identity is exactly the unfused SpMM; drop fused_activation()"
                        .to_string(),
                });
            }
            Some(kind) => KernelId::SpmmActivation(kind.entry()),
        };

        let bundle = self
            .cache
            .bundle(self.device, kernel, workgroup.x, workgroup.y)?;

        // --- params -----------------------------------------------------------
        let params = SpmmParams {
            num_rows,
            num_cols,
            feature_dim: fd_u32,
            alpha: self.fused_activation.map_or(0.0, ActivationKind::alpha),
        };
        let params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spmm.params"),
            size: std::mem::size_of::<SpmmParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

        // --- bind group --------------------------------------------------------
        // Built once against the bundle's layout. Re-dispatch and in-place input
        // rewrites reuse it, because it binds buffers rather than contents.
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spmm.bind_group"),
            layout: bundle.bind_group_layout(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: matrix.row_ptr().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: matrix.col_ind().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: matrix.values().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: input.buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: output.buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        Ok(PreparedSpmm {
            _operands: core::marker::PhantomData,
            _scope: core::marker::PhantomData,
            dispatch: PreparedDispatch::new(
                Arc::clone(bundle.pipeline()),
                bind_group,
                params_buffer,
                [groups_x, groups_y, 1],
                empty,
                self.label.unwrap_or_else(|| "spmm".to_owned()),
            ),
            workgroup,
            kernel,
            matrix_shape: a,
            feature_dim,
        })
    }
}

/// A validated, repeatedly dispatchable SpMM.
///
/// # Invariants
/// Holds an `Arc` to a cached pipeline and its own bind group, so neither is
/// rebuilt per dispatch. `encode` performs **no** validation and **no**
/// allocation — everything was settled in [`SpmmBuilder::prepare`].
#[derive(Debug)]
pub struct PreparedSpmm<'a, S: EncodeScope = Untracked> {
    /// Ties this operation to the borrows of its operands.
    ///
    /// # Why a prepared operation borrows, when it does not need to
    ///
    /// The bind group internally keeps its buffers alive, so outliving the
    /// operand tensors would be memory-safe. It would **not** be correct once a
    /// [`crate::TensorArena`] is in play: releasing a lease returns the buffer to
    /// the pool, which may hand it to a different logical tensor while this
    /// operation still dispatches into it -- silently wrong results, no error.
    ///
    /// The Phase 4 brief requires that a prepared operation never hold an
    /// invalidated lease. Nothing at runtime can detect that, so it is made
    /// impossible instead: this marker means the borrow checker rejects dropping
    /// an operand while the prepared operation is alive.
    ///
    /// Cost: a `PreparedSpmm` can no longer be stored in a `'static` structure.
    /// Nothing in this crate does, and the alternative was a documented hazard.
    _operands: core::marker::PhantomData<&'a ()>,
    _scope: core::marker::PhantomData<S>,
    /// Pipeline, bind group, params buffer, extent and label — the part every
    /// prepared operation has in common, implemented once.
    dispatch: PreparedDispatch,
    workgroup: WorkgroupShape,
    kernel: KernelId,
    matrix_shape: SparseCsrShape,
    feature_dim: usize,
}

impl<S: EncodeScope> PreparedSpmm<'_, S> {
    /// Record the dispatch into `encoder`, optionally with timestamp writes
    /// around the compute pass. Does **not** submit.
    ///
    /// Several `PreparedSpmm` (and other operations) may encode into the same
    /// encoder and execute in one submit.
    ///
    /// `timestamp_writes` is threaded through rather than branched on: the
    /// descriptor field already exists and the public `encode` supplies `None`,
    /// which is byte-for-byte the value the literal used to carry. The
    /// non-profiled path therefore gains no branch and no overhead — the
    /// profiled path is a different caller, not a runtime mode.
    ///
    /// # Errors
    ///
    /// Currently infallible — the `Result` is part of the operation contract so
    /// that an operation needing per-encode validation can be added later
    /// without a breaking change.
    fn encode_into(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) -> Result<(), ComputeError> {
        self.dispatch.encode_into(encoder, timestamp_writes)
    }

    /// `(groups_x, groups_y)` actually dispatched — `(0, 0)` when empty.
    #[must_use]
    pub const fn dispatch_groups(&self) -> (u32, u32) {
        let g = self.dispatch.groups();
        (g[0], g[1])
    }

    #[must_use]
    pub const fn workgroup(&self) -> WorkgroupShape {
        self.workgroup
    }

    /// Which kernel this operation compiled to.
    ///
    /// Replaces the former `pipeline_key()`. A [`KernelId`] plus the workgroup
    /// extent *is* the cache key now, and unlike the old key it cannot be
    /// paired with a different shader body.
    #[must_use]
    pub const fn kernel(&self) -> KernelId {
        self.kernel
    }

    #[must_use]
    pub const fn matrix_shape(&self) -> SparseCsrShape {
        self.matrix_shape
    }

    #[must_use]
    pub const fn feature_dim(&self) -> usize {
        self.feature_dim
    }

    /// Multiply-accumulate count this dispatch performs: `nnz * feature_dim`.
    ///
    /// The throughput denominator for benchmarks — "effective nonzero-feature
    /// operations".
    #[must_use]
    pub const fn mac_count(&self) -> usize {
        self.matrix_shape.nnz * self.feature_dim
    }
}

// ---------------------------------------------------------------------------
// encode, split by scope
// ---------------------------------------------------------------------------
//
// Two impls, no third. This is what closes the Phase 4 bypass: an operation
// touching a pooled buffer is `Tracked`, and the only `encode` it has takes a
// `TrackedSession` whose submission the arena observes. There is no signature
// that would accept a raw `wgpu::CommandEncoder`, so the mistake is a type
// error rather than a review item.

impl PreparedSpmm<'_, Untracked> {
    /// Record the dispatch into `encoder`. Does **not** submit.
    ///
    /// Available only when every operand is caller-owned. If any operand came
    /// from a [`crate::TensorArena`], this method does not exist on the type and
    /// [`PreparedSpmm::encode`] for `Tracked` must be used instead.
    ///
    /// # Errors
    /// Currently infallible; the `Result` is part of the operation contract.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) -> Result<(), ComputeError> {
        self.encode_into(encoder, None)
    }
}

impl PreparedSpmm<'_, Tracked> {
    /// Record the dispatch into a tracked session. Does **not** submit.
    ///
    /// Required because at least one operand is a pooled [`crate::ArenaTensor`]:
    /// the arena must learn which submission references it, or it could recycle
    /// the buffer while the GPU is still reading.
    ///
    /// # Errors
    /// Currently infallible; the `Result` is part of the operation contract.
    pub fn encode(&self, session: &mut crate::TrackedSession<'_>) -> Result<(), ComputeError> {
        self.encode_into(session.note_encoded(), None)
    }

    /// Record the dispatch with timestamp queries written around the compute
    /// pass, for kernel-only measurement.
    ///
    /// Crate-internal: this exists for `crate::profiling`, not as public API.
    /// Only the tracked scope has it — the profiling probe goes through a
    /// session, so an untracked twin would be dead code.
    ///
    /// # Errors
    /// As [`Self::encode`].
    pub(crate) fn encode_with_timestamps(
        &self,
        session: &mut crate::TrackedSession<'_>,
        writes: wgpu::ComputePassTimestampWrites<'_>,
    ) -> Result<(), ComputeError> {
        self.encode_into(session.note_encoded(), Some(writes))
    }
}
/// Unit tests for the workgroup rule.
///
/// These need no GPU: `spmm_workgroup_for` is a pure function, which is the
/// point of extracting it rather than inlining the choice into `prepare()`.
#[cfg(test)]
mod workgroup_rule_tests {
    use super::{WorkgroupShape, spmm_workgroup_for};

    /// The reference adapter's limit, so the rule is exercised unconstrained.
    const AMPLE: u32 = 1024;

    /// The rule must reproduce the measured winner at every measured point.
    ///
    /// This is the test that would fail if someone "simplified" the rule to a
    /// single constant, which is exactly what the measurement disproved.
    #[test]
    fn reproduces_measured_winner_at_every_measured_feature_dim() {
        // (feature_dim, winner) from the two-size sweep documented on the
        // function. The winner is the shape that wins on BOTH problem sizes, or
        // -- where they disagree -- the one whose loss is inside the noise on
        // the size it loses. See the doc comment for why 16x16 is never chosen
        // despite leading a few medium cells by <= 1.6%.
        let measured = [
            (1usize, WorkgroupShape::new(8, 8)),
            (4, WorkgroupShape::new(8, 8)),
            (8, WorkgroupShape::new(8, 8)),
            (16, WorkgroupShape::new(16, 8)),
            (32, WorkgroupShape::new(16, 8)),
            (64, WorkgroupShape::new(16, 8)),
            (128, WorkgroupShape::new(16, 8)),
        ];
        for (fd, want) in measured {
            assert_eq!(
                spmm_workgroup_for(fd, AMPLE),
                want,
                "feature_dim {fd} disagrees with the measured winner"
            );
        }
    }

    /// Both switch points sit on measured feature dimensions, so an off-by-one
    /// would move a measured point onto a shape that lost there.
    #[test]
    fn boundary_is_exactly_at_16() {
        assert_eq!(spmm_workgroup_for(15, AMPLE), WorkgroupShape::new(8, 8));
        assert_eq!(spmm_workgroup_for(16, AMPLE), WorkgroupShape::new(16, 8));
    }

    /// 16x16 was the first rule's choice for wide feature dimensions and cost
    /// 16.5% at the large problem size. It must never be selected automatically
    /// again; a caller who wants it has to say so explicitly.
    #[test]
    fn never_selects_16x16_automatically() {
        for fd in [1usize, 8, 16, 32, 64, 128, 512, 4096] {
            assert_ne!(
                spmm_workgroup_for(fd, AMPLE),
                WorkgroupShape::new(16, 16),
                "fd={fd} regressed to the shape that lost 16.5% on the large spec"
            );
        }
    }

    /// A device smaller than the reference must get a smaller shape, not an
    /// error: the WebGPU downlevel floor is 256 invocations, and some adapters
    /// report less.
    #[test]
    fn steps_down_to_fit_the_device_limit() {
        // fd 128 wants 16x8 = 128 invocations.
        assert_eq!(spmm_workgroup_for(128, 256), WorkgroupShape::new(16, 8));
        assert_eq!(spmm_workgroup_for(128, 128), WorkgroupShape::new(16, 8));
        assert_eq!(spmm_workgroup_for(128, 127), WorkgroupShape::new(8, 8));
        assert_eq!(spmm_workgroup_for(128, 64), WorkgroupShape::new(8, 8));
        assert_eq!(spmm_workgroup_for(128, 63), WorkgroupShape::new(4, 4));
        assert_eq!(spmm_workgroup_for(128, 16), WorkgroupShape::new(4, 4));
        assert_eq!(spmm_workgroup_for(128, 15), WorkgroupShape::new(1, 1));
        assert_eq!(spmm_workgroup_for(128, 1), WorkgroupShape::new(1, 1));
    }

    /// A shape the device already admits must not be shrunk by the fallback.
    #[test]
    fn small_feature_dim_is_not_penalised_by_a_small_device() {
        // fd 1 wants 8x8 = 64, which fits in 64 -- no step-down should occur.
        assert_eq!(spmm_workgroup_for(1, 64), WorkgroupShape::new(8, 8));
    }

    /// The postcondition stated in the doc comment, over the whole
    /// feature-dimension range this backend can encounter.
    #[test]
    fn postcondition_holds_across_the_range() {
        for limit in [1u32, 15, 16, 32, 63, 64, 127, 128, 255, 256, 1024] {
            for fd in [1usize, 2, 7, 15, 16, 17, 31, 32, 64, 128, 512, 4096] {
                let wg = spmm_workgroup_for(fd, limit);
                assert!(
                    wg.invocations() <= limit,
                    "fd={fd} limit={limit} produced {wg} = {} invocations",
                    wg.invocations()
                );
                assert!(wg.x > 0 && wg.y > 0, "fd={fd} limit={limit} gave {wg}");
            }
        }
    }

    /// Zero-invocation shapes would divide by zero when computing the dispatch
    /// extent, so the rule must never emit one even for degenerate input.
    #[test]
    fn zero_feature_dim_still_yields_a_usable_shape() {
        let wg = spmm_workgroup_for(0, AMPLE);
        assert!(wg.x > 0 && wg.y > 0);
        assert!(wg.invocations() > 0);
    }

    /// Same inputs, same output — no hidden state, no caching, no device query.
    #[test]
    fn is_deterministic() {
        for fd in [1usize, 16, 32, 128] {
            let first = spmm_workgroup_for(fd, AMPLE);
            for _ in 0..8 {
                assert_eq!(spmm_workgroup_for(fd, AMPLE), first);
            }
        }
    }

    /// Usable in const context — proof there is no global state behind it.
    #[test]
    fn is_const_evaluable() {
        const CHOSEN: WorkgroupShape = spmm_workgroup_for(128, 1024);
        assert_eq!(CHOSEN, WorkgroupShape::new(16, 8));
    }
}
