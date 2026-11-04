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

use hymeko_tensor::compute::{ComputeError, TensorShape, try_u32};

use super::OperandCheck;
use crate::pipeline::KernelId;
use crate::{GpuCsrMatrix, GpuTensor};

/// WGSL source owned by [`KernelId::ScatterMeanCsr`].
pub const SCATTER_MEAN_CSR_SHADER: &str = include_str!("../../shaders/scatter_mean_csr.wgsl");

/// Default one-dimensional workgroup for the scatter gather.
pub const DEFAULT_SCATTER_MEAN_WORKGROUP: u32 = 128;

uniform_params! {
    ScatterMeanParams(24) {
        n_nodes: u32,
        n_edges: u32,
        hidden: u32,
        nnz: u32,
        count: u32,
        stride: u32,
    }
}

/// Builds one validated edge-to-node mean scatter dispatch.
pub struct ScatterMeanBuilder<'a>(ScatterMeanFields<'a>);

operation_builder! {
    ScatterMeanBuilder(ScatterMeanFields, "scatter_mean") {
        /// The inverse incidence, as a CSR the canonical owner already validated.
        ///
        /// `rows = n_nodes`, `cols = n_edges`, `col_ind` = the incident hyperedge. Its `values`
        /// buffer is not consumed by this unweighted mean; see the module documentation.
        incidence: GpuCsrMatrix<f32>,
        /// The host owner's per-node incidence count. Not recomputed on device.
        counts: GpuTensor<f32>,
        edge_state: GpuTensor<f32>,
        node_state: GpuTensor<f32>,
    }
    workgroup: DEFAULT_SCATTER_MEAN_WORKGROUP;
}

impl<'a> ScatterMeanBuilder<'a> {
    /// Validate every operand and prepare a repeatedly encodable dispatch.
    ///
    /// # Errors
    /// [`ComputeError::ShapeMismatch`] for any operand whose shape disagrees with the CSR the
    /// others describe; [`ComputeError::IndexOverflow`] for a count that does not fit `u32`;
    /// [`ComputeError::Backend`] for a device that cannot satisfy the layout or workgroup.
    pub fn prepare(self) -> Result<PreparedScatterMean<'a>, ComputeError> {
        let (incidence, counts, edge_state, node_state) = self.operands()?;

        let n_nodes = node_state.shape().rows;
        let hidden = node_state.shape().cols;
        let n_edges = edge_state.shape().rows;
        if n_nodes == 0 || hidden == 0 {
            return OperandCheck("scatter_mean node state must be non-empty").mismatch(1, 0);
        }
        OperandCheck("scatter_mean edge state shape")
            .shape(edge_state, TensorShape::new(n_edges, hidden))?;
        // The CSR's own structural properties were proved by `upload_csr`; what remains is that this
        // CSR describes THIS pooling — its rows are the nodes being written and its columns are the
        // edges being read.
        let csr = incidence.shape();
        if csr.rows != n_nodes {
            return OperandCheck("scatter_mean incidence rows").mismatch(n_nodes, csr.rows);
        }
        if csr.cols != n_edges {
            return OperandCheck("scatter_mean incidence columns").mismatch(n_edges, csr.cols);
        }
        let (counts_len, _) = OperandCheck("scatter_mean count length").elements(counts)?;
        if counts_len != n_nodes {
            return OperandCheck("scatter_mean counts length").mismatch(n_nodes, counts_len);
        }
        let kernel = KernelId::ScatterMeanCsr;
        let (elements, count) =
            OperandCheck("scatter_mean output element count").elements(node_state)?;
        let dispatch = self.0.factory.grid_dispatch(
            kernel,
            self.0.workgroup,
            count,
            |stride| {
                Ok(ScatterMeanParams {
                    n_nodes: try_u32("scatter_mean node count", n_nodes)?,
                    n_edges: try_u32("scatter_mean edge count", n_edges)?,
                    hidden: try_u32("scatter_mean hidden", hidden)?,
                    nnz: try_u32("scatter_mean incidence count", csr.nnz)?,
                    count,
                    stride,
                })
            },
            &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: incidence.row_ptr().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: incidence.col_ind().as_entire_binding(),
                },
                counts.binding(2),
                edge_state.binding(3),
                node_state.binding(4),
            ],
            self.0.label,
        )?;
        Ok(PreparedScatterMean::new(dispatch, kernel, elements))
    }
}

/// A validated, repeatedly dispatchable edge-to-node mean scatter.
#[derive(Debug)]
pub struct PreparedScatterMean<'a>(super::PreparedOne<'a>);

prepared_operation!(PreparedScatterMean);
