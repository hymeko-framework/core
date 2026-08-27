//! The shared body of a prepared compute dispatch.
//!
//! # What this is
//!
//! SpMM and activation had, byte for byte, the same `encode_into`: skip when
//! empty, open a compute pass, set the pipeline, set bind group 0, dispatch.
//! They also owned the same four things — a pipeline `Arc`, a bind group, a
//! params buffer kept alive for the bind group's sake, and a label.
//!
//! [`PreparedDispatch`] owns exactly that and nothing else. It is the *only*
//! ordinary compute-pass encode implementation in the crate.
//!
//! # What this is not
//!
//! Not an operation abstraction. There is no `Operation` trait, no
//! `Box<dyn Operation>` and no runtime kernel-kind switch: `PreparedSpmm` and
//! `PreparedActivation` remain distinct types with their own validation,
//! metadata and scope parameters, and each simply *contains* one of these. The
//! type-state (`EncodeScope`) and the operand lifetime stay on the wrappers,
//! where they carry meaning; this struct is deliberately scope-agnostic and
//! knows nothing about tensors.
//!
//! A general `KernelSpec` framework was considered and rejected — see the
//! Phase 9 report. With two consumers it would have been more machinery than
//! the duplication it removed.

use std::sync::Arc;

use hymeko::tensor::compute::ComputeError;

/// A dispatch that has passed every check and can be encoded repeatedly.
///
/// # Invariants
///
/// * `groups` is either all-nonzero or the dispatch is `empty`. WGPU rejects a
///   zero extent in any dimension, so an empty dispatch must be skipped rather
///   than issued with zeros.
/// * `bind_group` was built against the layout of `pipeline`'s bundle, and
///   borrows `params_buffer`, which is why that buffer is owned here.
#[derive(Debug)]
pub(crate) struct PreparedDispatch {
    pipeline: Arc<wgpu::ComputePipeline>,
    bind_group: wgpu::BindGroup,
    /// Held solely to keep the uniform buffer alive for `bind_group`. Never
    /// read after construction.
    _params_buffer: wgpu::Buffer,
    groups: [u32; 3],
    empty: bool,
    label: String,
}

impl PreparedDispatch {
    pub(crate) const fn new(
        pipeline: Arc<wgpu::ComputePipeline>,
        bind_group: wgpu::BindGroup,
        params_buffer: wgpu::Buffer,
        groups: [u32; 3],
        empty: bool,
        label: String,
    ) -> Self {
        Self {
            pipeline,
            bind_group,
            _params_buffer: params_buffer,
            groups,
            empty,
            label,
        }
    }

    /// Record the dispatch, optionally with timestamp writes around the pass.
    ///
    /// `timestamp_writes` is threaded through rather than branched on: the
    /// descriptor field already exists and every production caller supplies
    /// `None`, which is byte-for-byte the value the literal used to carry. The
    /// non-profiled path therefore gains no branch and no overhead — profiling
    /// is a different caller, not a runtime mode.
    ///
    /// # Errors
    ///
    /// Currently infallible; the `Result` is part of the operation contract so
    /// an operation needing per-encode validation can be added later without a
    /// breaking change.
    pub(crate) fn encode_into(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) -> Result<(), ComputeError> {
        // Zero-extent dispatches are invalid; with nothing to compute the
        // output is already correct, so the pass is skipped entirely.
        if self.empty {
            return Ok(());
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(&self.label),
            timestamp_writes,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.dispatch_workgroups(self.groups[0], self.groups[1], self.groups[2]);
        Ok(())
    }

    /// Workgroup counts actually dispatched — all zero when empty.
    pub(crate) const fn groups(&self) -> [u32; 3] {
        if self.empty { [0, 0, 0] } else { self.groups }
    }
}
