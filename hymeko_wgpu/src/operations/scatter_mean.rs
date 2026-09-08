//! Edge to node mean scatter over GPU-resident, validated operands.
//!
//! # Naming
//!
//! The *operation* is `scatter_mean`: pooling edge state onto nodes is a mathematical act with no
//! opinion about storage. Its builder and prepared type carry that name. CSR appears only where
//! something physical is named — [`KernelId::ScatterMeanCsr`], `LayoutId::ScatterMeanCsr`,
//! `GpuCsrMatrix`, `scatter_mean_csr.wgsl` — because those *are* the CSR lowering. A COO lowering
//! would be a second physical arm under the same operation identity, not a second operation.
//!
//! The mathematical owner remains Nagare's `ops::scatter::CycleIncidence`. This backend consumes
//! an inverse-incidence CSR and a per-node incidence count that owner already produced, and
//! reproduces its accumulation order and its reciprocal multiply. It derives neither.
//!
//! # Why the CSR arrives as a [`GpuCsrMatrix`] and not as two raw `GpuTensor<u32>`s
//!
//! Raw buffers are opaque to `prepare()`: their CONTENT cannot be inspected host-side, so a builder
//! taking them could not prove `row_ptr[0] == 0`, monotonicity, `offset <= nnz`, the terminal
//! `offset == nnz`, or that every column names a real edge. Left to the shader those would read out
//! of bounds and WGPU would CLAMP the access — a silently wrong answer rather than a failure.
//!
//! [`crate::ResourceFactory::upload_csr`] already proves all of them, and `GpuCsrMatrix`'s fields
//! are private, so the only way to obtain one is through that owner. Consuming it makes the
//! structural contract hold BY CONSTRUCTION rather than by a second validator this module would
//! have to write and keep in agreement with the first.
//!
//! Mapping for an inverse incidence: `rows = n_nodes`, `cols = n_edges`, `nnz` = total incidences,
//! `col_ind` = the incident hyperedge. The CSR's `values` buffer is **deliberately not consumed**:
//! this slice is the unweighted mean, whose only weight is the reciprocal of the incidence count.
//! Reading it would introduce a per-incidence weighting semantics neither the CPU owner nor this
//! slice has.
//!
//! The kernel is a node-indexed **gather**: one invocation per `(node, channel)`, walking that
//! node's CSR row. Every output element is written by exactly one invocation, so there is no
//! atomic and no nondeterministic accumulation order. See `shaders/scatter_mean_csr.wgsl` for why
//! the atomics-based push alternative is rejected rather than merely unused.

use std::sync::Arc;

use hymeko::tensor::compute::{BackendKind, ComputeError, TensorShape, try_u32};

use crate::operations::dispatch::PreparedDispatch;
use crate::pipeline::{KernelId, PipelineCache};
use crate::{GpuCsrMatrix, GpuTensor};

/// WGSL source owned by [`KernelId::ScatterMeanCsr`].
pub const SCATTER_MEAN_CSR_SHADER: &str = include_str!("../../shaders/scatter_mean_csr.wgsl");

/// Default one-dimensional workgroup for the scatter gather.
pub const DEFAULT_SCATTER_MEAN_WORKGROUP: u32 = 128;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct ScatterMeanParams {
    n_nodes: u32,
    n_edges: u32,
    hidden: u32,
    nnz: u32,
    count: u32,
    stride: u32,
}

const _: () = assert!(std::mem::size_of::<ScatterMeanParams>() == 24);

#[derive(Clone, Copy, Debug)]
pub(crate) struct ScatterMeanLimits {
    pub max_workgroups_per_dimension: u32,
    pub max_invocations_per_workgroup: u32,
    pub max_storage_buffers_per_stage: u32,
}

/// Builds one validated edge-to-node mean scatter dispatch.
pub struct ScatterMeanBuilder<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    cache: &'a PipelineCache,
    limits: ScatterMeanLimits,
    incidence: Option<&'a GpuCsrMatrix<f32>>,
    counts: Option<&'a GpuTensor<f32>>,
    edge_state: Option<&'a GpuTensor<f32>>,
    node_state: Option<&'a GpuTensor<f32>>,
    workgroup: u32,
    label: Option<String>,
}

impl<'a> ScatterMeanBuilder<'a> {
    pub(crate) fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        cache: &'a PipelineCache,
        limits: ScatterMeanLimits,
    ) -> Self {
        Self {
            device,
            queue,
            cache,
            limits,
            incidence: None,
            counts: None,
            edge_state: None,
            node_state: None,
            workgroup: DEFAULT_SCATTER_MEAN_WORKGROUP,
            label: None,
        }
    }

    /// The inverse incidence, as a CSR the canonical owner already validated.
    ///
    /// `rows = n_nodes`, `cols = n_edges`, `col_ind` = the incident hyperedge. Its `values` buffer
    /// is not consumed by this unweighted mean; see the module documentation.
    #[must_use]
    pub fn incidence(mut self, incidence: &'a GpuCsrMatrix<f32>) -> Self {
        self.incidence = Some(incidence);
        self
    }

    /// The host owner's per-node incidence count. Not recomputed on device.
    #[must_use]
    pub fn counts(mut self, counts: &'a GpuTensor<f32>) -> Self {
        self.counts = Some(counts);
        self
    }

    #[must_use]
    pub fn edge_state(mut self, state: &'a GpuTensor<f32>) -> Self {
        self.edge_state = Some(state);
        self
    }

    #[must_use]
    pub fn node_state(mut self, state: &'a GpuTensor<f32>) -> Self {
        self.node_state = Some(state);
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
    /// [`ComputeError::ShapeMismatch`] for any operand whose shape disagrees with the CSR the
    /// others describe; [`ComputeError::IndexOverflow`] for a count that does not fit `u32`;
    /// [`ComputeError::Backend`] for a device that cannot satisfy the layout or workgroup.
    pub fn prepare(self) -> Result<PreparedScatterMean<'a>, ComputeError> {
        let incidence = supplied(self.incidence, "scatter_mean incidence")?;
        let counts = supplied(self.counts, "scatter_mean counts")?;
        let edge_state = supplied(self.edge_state, "scatter_mean edge state")?;
        let node_state = supplied(self.node_state, "scatter_mean node state")?;

        let n_nodes = node_state.shape().rows;
        let hidden = node_state.shape().cols;
        let n_edges = edge_state.shape().rows;
        if n_nodes == 0 || hidden == 0 {
            return shape_error("scatter_mean node state must be non-empty", 1, 0);
        }
        if edge_state.shape() != TensorShape::new(n_edges, hidden) {
            return shape_error(
                "scatter_mean edge state shape",
                n_edges.saturating_mul(hidden),
                edge_state.shape().checked_elements().unwrap_or(usize::MAX),
            );
        }
        // The CSR's own structural properties were proved by `upload_csr`; what remains is that this
        // CSR describes THIS pooling — its rows are the nodes being written and its columns are the
        // edges being read.
        let csr = incidence.shape();
        if csr.rows != n_nodes {
            return shape_error("scatter_mean incidence rows", n_nodes, csr.rows);
        }
        if csr.cols != n_edges {
            return shape_error("scatter_mean incidence columns", n_edges, csr.cols);
        }
        let counts_len = counts
            .shape()
            .checked_elements()
            .ok_or(ComputeError::IndexOverflow {
                what: "scatter_mean count length",
                value: usize::MAX,
            })?;
        if counts_len != n_nodes {
            return shape_error("scatter_mean counts length", n_nodes, counts_len);
        }
        if self.limits.max_storage_buffers_per_stage < 5 {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "scatter_mean requires 5 storage buffers, device exposes {}",
                    self.limits.max_storage_buffers_per_stage
                ),
            });
        }
        if self.workgroup == 0 || self.workgroup > self.limits.max_invocations_per_workgroup {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!(
                    "scatter_mean workgroup {} outside 1..={}",
                    self.workgroup, self.limits.max_invocations_per_workgroup
                ),
            });
        }

        let elements =
            node_state
                .shape()
                .checked_elements()
                .ok_or(ComputeError::IndexOverflow {
                    what: "scatter_mean output element count",
                    value: usize::MAX,
                })?;
        let count = try_u32("scatter_mean output element count", elements)?;
        let ideal_groups = count.div_ceil(self.workgroup);
        let groups = ideal_groups.min(self.limits.max_workgroups_per_dimension);
        let stride = groups
            .checked_mul(self.workgroup)
            .ok_or(ComputeError::IndexOverflow {
                what: "scatter_mean dispatch stride",
                value: groups as usize,
            })?;
        let params = ScatterMeanParams {
            n_nodes: try_u32("scatter_mean node count", n_nodes)?,
            n_edges: try_u32("scatter_mean edge count", n_edges)?,
            hidden: try_u32("scatter_mean hidden", hidden)?,
            nnz: try_u32("scatter_mean incidence count", csr.nnz)?,
            count,
            stride,
        };

        let kernel = KernelId::ScatterMeanCsr;
        let bundle = self.cache.bundle(self.device, kernel, self.workgroup, 1)?;
        let params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scatter_mean_csr.params"),
            size: std::mem::size_of::<ScatterMeanParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scatter_mean_csr.bind_group"),
            layout: bundle.bind_group_layout(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: incidence.row_ptr().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: incidence.col_ind().as_entire_binding(),
                },
                entry(2, counts),
                entry(3, edge_state),
                entry(4, node_state),
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });
        Ok(PreparedScatterMean {
            _operands: core::marker::PhantomData,
            dispatch: PreparedDispatch::new(
                Arc::clone(bundle.pipeline()),
                bind_group,
                params_buffer,
                [groups, 1, 1],
                count == 0,
                self.label.unwrap_or_else(|| "scatter_mean_csr".to_owned()),
            ),
            kernel,
            elements,
        })
    }
}

fn supplied<'a, T>(value: Option<&'a T>, operand: &'static str) -> Result<&'a T, ComputeError> {
    value.ok_or(ComputeError::ShapeMismatch {
        operand,
        expected: 1,
        actual: 0,
    })
}

fn shape_error<T>(
    operand: &'static str,
    expected: usize,
    actual: usize,
) -> Result<T, ComputeError> {
    Err(ComputeError::ShapeMismatch {
        operand,
        expected,
        actual,
    })
}

fn entry<T>(binding: u32, tensor: &GpuTensor<T>) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: tensor.buffer().as_entire_binding(),
    }
}

/// A validated, repeatedly dispatchable edge-to-node mean scatter.
#[derive(Debug)]
pub struct PreparedScatterMean<'a> {
    _operands: core::marker::PhantomData<&'a ()>,
    dispatch: PreparedDispatch,
    kernel: KernelId,
    elements: usize,
}

impl PreparedScatterMean<'_> {
    /// Record the dispatch without submitting it.
    ///
    /// # Errors
    /// Whatever the underlying dispatch reports; a zero-element operation records nothing.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) -> Result<(), ComputeError> {
        self.dispatch.encode_into(encoder, None)
    }

    /// The kernel this operation dispatches.
    #[must_use]
    pub const fn kernel(&self) -> KernelId {
        self.kernel
    }

    /// Output element count, `n_nodes * hidden`.
    #[must_use]
    pub const fn elements(&self) -> usize {
        self.elements
    }

    #[must_use]
    pub const fn dispatch_groups(&self) -> u32 {
        self.dispatch.groups()[0]
    }
}
