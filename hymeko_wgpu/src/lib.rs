//! `hymeko_wgpu` — WGPU compute backend for the HyMeKo tensor kernels.
//!
//! # Status
//!
//! | capability | state |
//! |---|---|
//! | CSR SpMM (`spmm.wgsl`), optionally fused with an activation | implemented, tested |
//! | elementwise activation, out-of-place and in-place | implemented, tested |
//! | pipeline cache, explicit encode / submit / readback | implemented, tested |
//! | `TensorArena` buffer reuse + non-blocking completion tracking | implemented, tested |
//! | `TrackedSession` and type-level operand provenance ([`scope`]) | implemented, tested |
//!
//! (An earlier version of this header claimed two operations existed *and* that
//! the first was "deliberately not here yet" — a Phase 1 sentence that survived
//! into Phase 3. Both halves cannot be true; the table above is the current
//! state.)
//!
//! # Position in the stack
//!
//! ```text
//!   hymeko_core::tensor::compute      backend-independent vocabulary
//!     TensorShape, SparseCsrShape,    (no wgpu types, no device handles)
//!     BackendKind, ComputeError
//!            |                    |
//!            v                    v
//!   hymeko_hnn (CPU backend)   hymeko_wgpu (this crate)
//!     *_into_serial              WgpuBackend, GpuTensor, GpuCsrMatrix
//!     *_into_parallel            PipelineCache, WGSL
//! ```
//!
//! This crate depends on `hymeko_core` **only**. It does not depend on
//! `hymeko_hnn`: a GPU backend needs shapes and errors, not hypergraph views.
//! That is precisely why the shared vocabulary was placed in core.
//!
//! # What this crate does not do
//!
//! * It does not submit implicitly. [`WgpuBackend::encoder`] hands out an
//!   encoder; several operations share it; [`WgpuBackend::submit`] is explicit.
//! * It does not read back implicitly. Readback is a separate, explicit step.
//! * It does not own any core tensor type, and no core type owns a GPU handle.
//!   [`ResourceFactory`] adapts host data into device resources.
//!
//! # Example — device, resources, explicit submit
//!
//! The minimal surface: acquire a device, upload operands, submit. Encoding a
//! real operation is shown on [`SpmmBuilder`]; pooled buffers and sessions on
//! [`TensorArena`].
//!
//! ```no_run
//! use hymeko::tensor::compute::{SparseCsrShape, TensorShape};
//! use hymeko_wgpu::WgpuBackend;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let backend = WgpuBackend::builder()
//!     .label("hymeko-compute")
//!     .build()?;
//! println!("{}", backend.capabilities().summary());
//!
//! let res = backend.resources();
//! let csr = res.upload_csr(
//!     "a",
//!     SparseCsrShape::new(2, 2, 2),
//!     &[0, 1, 2],   // row_ptr
//!     &[0, 1],      // col_ind
//!     &[1.0, 2.0],  // values
//! )?;
//! let x = res.upload_tensor("x", TensorShape::new(2, 1), &[1.0, 1.0])?;
//! let y = res.zeroed_tensor("y", TensorShape::new(2, 1))?;
//!
//! // Command pattern: encode -> submit, never one submit per operation.
//! let encoder = backend.encoder(Some("setup"));
//! let index = backend.submit(encoder);
//! backend.wait(index)?;
//! # let _ = (csr, x, y);
//! # Ok(())
//! # }
//! ```

mod block_on;

pub mod arena;
pub mod completion;
#[doc(hidden)]
pub mod profiling;
pub mod scope;

pub mod backend;
pub mod operations;
pub mod pipeline;
pub mod resources;

pub use arena::{
    AllocationKey, ArenaStats, ArenaTensor, ReusePolicy, TensorArena, TensorArenaConfig,
    TrackedSession,
};
pub use backend::{DeviceCapabilities, WgpuBackend, WgpuBackendBuilder};
pub use completion::{CompletionTracker, SubmissionEpoch};
pub use operations::{
    ACTIVATION_INPLACE_SHADER, ACTIVATION_SHADER, ACTIVATION_WORKGROUP_SWEEP, ActivationBuilder,
    ActivationKind, DEFAULT_ACTIVATION_WORKGROUP, OperationFactory, PreparedActivation,
    PreparedSpmm, SPMM_ACTIVATION_SHADER, SPMM_SHADER, SpmmBuilder, WorkgroupShape,
    spmm_workgroup_for,
};
pub use pipeline::{PipelineCache, PipelineKey};
pub use resources::{GpuCsrMatrix, GpuTensor, ResourceFactory};
pub use scope::{EncodeScope, JoinScope, Joined, TensorOperand, Tracked, Untracked};

/// Re-exported so downstream code can name the shared vocabulary without also
/// importing `hymeko_core` directly.
pub use hymeko::tensor::compute::{BackendKind, ComputeError, SparseCsrShape, TensorShape};

/// WGSL source for the skeleton validation kernel.
///
/// Not an operation. It writes `value` into every element of `out`, and exists
/// so the pipeline cache, bind group creation and the encode/submit path can be
/// exercised without involving a real kernel. Retained because
/// `tests/skeleton.rs` still uses it to test that machinery in isolation.
pub const FILL_SHADER: &str = include_str!("../shaders/fill.wgsl");

#[cfg(test)]
mod fanout_tests;

#[doc(hidden)]
pub mod testing;
