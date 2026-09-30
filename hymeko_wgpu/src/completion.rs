//! Submission epochs and non-blocking completion tracking.
//!
//! # Why this module exists
//!
//! A buffer pool that recycles GPU memory must answer one question correctly:
//! *has the GPU finished with this allocation?* Getting it wrong produces
//! corrupted results with no error — the worst failure mode available.
//!
//! Phase 3's backend could answer only the blocking form of that question:
//! [`crate::WgpuBackend::wait`] blocks until a given [`wgpu::SubmissionIndex`]
//! completes. Calling that on every lease release would serialise the host
//! against the GPU and throw away the asynchrony the resident chain exists to
//! exploit — explicitly forbidden by the Phase 4 brief.
//!
//! So this is the **minimum backend extension** the arena needs, and nothing
//! more: a monotonic epoch counter plus a non-blocking watermark of what has
//! completed.
//!
//! # The mechanism, and why this one
//!
//! `wgpu` offers three candidate primitives. They were compared before choosing:
//!
//! | primitive | verdict |
//! |---|---|
//! | `poll(PollType::Poll)` → `PollStatus::QueueEmpty` | correct but *coarse*: tells you everything finished, never which submission. Usable, wasteful — a pool would recycle nothing while any later work is in flight. |
//! | `poll(PollType::Wait { index, timeout: Some(ZERO) })` | per-submission, but leans on zero-timeout semantics that `wgpu` documents only as "max time to wait". Relying on an unspecified edge case to decide memory reuse is not a trade worth making. |
//! | `Queue::on_submitted_work_done(callback)` | **chosen.** Documented to fire once the work submitted *before* the call has completed. Portable, exact, and needs no timeout semantics. |
//!
//! The callback is driven by any `device.poll(...)`, so [`CompletionTracker::poll`]
//! does one non-blocking `PollType::Poll` and then reads the watermark.
//!
//! # The ordering argument — stated precisely
//!
//! An earlier version of this comment claimed `fetch_max` made the watermark
//! "conservative under any out-of-order completion". **That was wrong**, and the
//! distinction matters enough to spell out, because the whole arena rests on it.
//!
//! ## What a single queue guarantees
//!
//! A `wgpu::Queue` processes its submissions **in submission order**. Work
//! *within* one submission may run in parallel, but the queue's timeline is
//! sequential: submission *N* does not complete before submission *N−1*.
//!
//! ## Why that permits a scalar watermark
//!
//! In-order completion means completion is **downward closed**: if submission
//! *N* has completed, then every submission *1 … N−1* has too. A set that is
//! downward closed over a total order is fully described by its maximum, so one
//! `u64` represents the entire completion state exactly — no set, no bitmap, no
//! per-submission bookkeeping.
//!
//! **This is a precondition, not a consequence of `fetch_max`.** Take the
//! in-order guarantee away and a scalar watermark is not conservative, it is
//! simply *wrong*: a watermark of 9 asserts that 5 is complete, and if 5 belongs
//! to an independently progressing timeline, that assertion can be false while
//! the arena hands out 5's buffer.
//!
//! ## What `fetch_max` actually buys
//!
//! Exactly one thing: independence from the order in which the **host observes**
//! the callbacks. `on_submitted_work_done` callbacks run on whichever thread
//! polls, and nothing in the API promises that a callback registered earlier is
//! *delivered* earlier. Under the in-order guarantee that does not matter — if
//! the callback for *N* has fired, *N* completed, hence all *< N* completed — so
//! taking the maximum of whatever has been observed is correct regardless of
//! delivery order. `store` would be a latent bug here, not because the GPU
//! reorders, but because the host might observe out of order and clobber a
//! higher watermark with a lower one.
//!
//! So: **in-order completion makes the watermark valid; `fetch_max` makes it
//! robust to host-side observation order.** Two different claims.
//!
//! ## Multiple queues
//!
//! **Not supported, by construction.** `WgpuBackend` owns exactly one
//! `wgpu::Queue`, and a `CompletionTracker` is created per arena over that one
//! queue. Two queues would be two independent timelines, completion would no
//! longer be downward closed over a single order, and this type would be
//! unsound — not merely imprecise.
//!
//! A multi-queue design would need one watermark per queue and a slab tagged
//! with the queue it was last used on (or a conservative join across all
//! queues). That is a different data structure, and it should be built when a
//! second queue exists, not speculatively.
//!
//! # What this module deliberately does not do
//!
//! No background thread, no timer, no automatic polling. The host advances the
//! watermark when it asks — [`crate::TensorArena::poll_completion`] — which
//! keeps the "no hidden submit, no hidden wait" property of the operation API.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use hymeko::tensor::compute::{BackendKind, ComputeError};

/// A submission's position in the queue's completion order.
///
/// Monotonically increasing from 1. Epoch 0 is the sentinel "nothing has been
/// submitted", which is why [`CompletionTracker::completed`] starts there — an
/// allocation last used at epoch 0 was never submitted and is free immediately.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubmissionEpoch(u64);

impl SubmissionEpoch {
    /// The sentinel epoch: no submission has occurred.
    pub const NEVER: Self = Self(0);

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Whether work tagged with this epoch is known complete given `watermark`.
    #[must_use]
    pub const fn is_complete_at(self, watermark: Self) -> bool {
        self.0 <= watermark.0
    }
}

/// The arena dates a release with the submission that may still read it.
impl hymeko::tensor::compute::arena::ArenaEpoch for SubmissionEpoch {
    const NEVER: Self = Self::NEVER;

    fn is_complete_at(self, watermark: Self) -> bool {
        Self::is_complete_at(self, watermark)
    }
}

impl core::fmt::Display for SubmissionEpoch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "e{}", self.0)
    }
}

/// Issues epochs and tracks which have completed, without blocking.
///
/// Cloneable and cheap: the watermark lives behind one `Arc<AtomicU64>` so the
/// completion callbacks — which must be `'static` — can hold it while the arena
/// reads it.
///
/// # Invariants
///
/// * `next` only increases;
/// * `completed` only increases (enforced by `fetch_max`);
/// * `completed <= next` always, because a callback is registered only after
///   its epoch is issued.
#[derive(Clone, Debug)]
pub struct CompletionTracker {
    next: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
}

impl Default for CompletionTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CompletionTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next: Arc::new(AtomicU64::new(0)),
            completed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Submit `encoder` on `queue`, returning the epoch that identifies it.
    ///
    /// Registers the completion callback **after** the submit, which is what
    /// `on_submitted_work_done` requires: it reports on work submitted before
    /// the call.
    ///
    /// # Postconditions
    ///
    /// The returned epoch is strictly greater than every previously returned
    /// epoch, and greater than [`Self::completed`] at the moment of return.
    pub fn submit(
        &self,
        queue: &wgpu::Queue,
        encoder: wgpu::CommandEncoder,
    ) -> (wgpu::SubmissionIndex, SubmissionEpoch) {
        let index = queue.submit(Some(encoder.finish()));
        let epoch = self.next.fetch_add(1, Ordering::AcqRel) + 1;

        let completed = Arc::clone(&self.completed);
        queue.on_submitted_work_done(move || {
            // fetch_max, not store: callbacks may be DELIVERED to the host in
            // any order, and a `store` from a late-delivered earlier callback
            // would clobber a higher watermark. The GPU-side ordering is
            // guaranteed separately -- see the module header.
            completed.fetch_max(epoch, Ordering::AcqRel);
        });

        (index, SubmissionEpoch(epoch))
    }

    /// Highest epoch known to have completed. Never blocks, never polls.
    #[must_use]
    pub fn completed(&self) -> SubmissionEpoch {
        SubmissionEpoch(self.completed.load(Ordering::Acquire))
    }

    /// Most recently issued epoch.
    #[must_use]
    pub fn latest(&self) -> SubmissionEpoch {
        SubmissionEpoch(self.next.load(Ordering::Acquire))
    }

    /// Drive pending completion callbacks **without blocking**, then report the
    /// watermark.
    ///
    /// This is the call that makes deferred recycling possible: it costs one
    /// non-blocking device poll and can only move the watermark forward.
    ///
    /// # Errors
    ///
    /// [`ComputeError::Backend`] if the device reports a poll failure. A poll
    /// that simply finds nothing finished is **not** an error.
    pub fn poll(&self, device: &wgpu::Device) -> Result<SubmissionEpoch, ComputeError> {
        device
            .poll(wgpu::PollType::Poll)
            .map_err(|e| ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!("non-blocking completion poll failed: {e}"),
            })?;
        Ok(self.completed())
    }

    /// Block until `epoch` has completed, then report the watermark.
    ///
    /// The escape hatch for callers that genuinely must synchronise — a
    /// readback, or an arena asked to reclaim everything. **Not** used on the
    /// lease-release path.
    ///
    /// # Errors
    ///
    /// [`ComputeError::Backend`] on device poll failure.
    pub fn wait_for(
        &self,
        device: &wgpu::Device,
        epoch: SubmissionEpoch,
    ) -> Result<SubmissionEpoch, ComputeError> {
        if epoch.is_complete_at(self.completed()) {
            return Ok(self.completed());
        }
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!("blocking completion wait failed: {e}"),
            })?;
        // The poll above ran the callbacks; re-read rather than assume.
        Ok(self.completed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Epoch algebra is pure, so it is testable without a device — and it is
    /// the part a recycling bug would hinge on.
    #[test]
    fn never_epoch_is_complete_against_any_watermark() {
        assert!(SubmissionEpoch::NEVER.is_complete_at(SubmissionEpoch::NEVER));
        assert!(SubmissionEpoch::NEVER.is_complete_at(SubmissionEpoch(7)));
    }

    #[test]
    fn an_epoch_is_not_complete_before_its_watermark() {
        let e = SubmissionEpoch(5);
        assert!(!e.is_complete_at(SubmissionEpoch(4)));
        assert!(e.is_complete_at(SubmissionEpoch(5)));
        assert!(e.is_complete_at(SubmissionEpoch(6)));
    }

    #[test]
    fn fresh_tracker_reports_nothing_submitted_and_nothing_completed() {
        let t = CompletionTracker::new();
        assert_eq!(t.latest(), SubmissionEpoch::NEVER);
        assert_eq!(t.completed(), SubmissionEpoch::NEVER);
    }

    /// Host-side callback DELIVERY order must not lower the watermark.
    ///
    /// This is the property `fetch_max` provides. It is deliberately NOT a claim
    /// that GPU completion may be out of order -- that would invalidate the
    /// scalar watermark entirely (see the module header). What is simulated here
    /// is two callbacks whose host-side delivery is transposed.
    #[test]
    fn late_delivery_of_an_earlier_callback_does_not_lower_the_watermark() {
        let t = CompletionTracker::new();
        t.completed.fetch_max(9, Ordering::AcqRel);
        assert_eq!(t.completed(), SubmissionEpoch(9));
        // Callback for epoch 3 delivered after the one for epoch 9.
        t.completed.fetch_max(3, Ordering::AcqRel);
        assert_eq!(
            t.completed(),
            SubmissionEpoch(9),
            "watermark regressed; deferred recycling would free a live buffer"
        );
    }

    /// The downward-closure property the arena relies on: a watermark of N means
    /// every epoch up to N is complete. This is what makes one `u64` a complete
    /// description of completion state, and it is only true because a single
    /// queue completes in order.
    #[test]
    fn watermark_implies_all_lower_epochs_complete() {
        let watermark = SubmissionEpoch(6);
        for e in 0..=6u64 {
            assert!(
                SubmissionEpoch(e).is_complete_at(watermark),
                "epoch {e} must be implied complete by watermark 6"
            );
        }
        for e in 7..12u64 {
            assert!(!SubmissionEpoch(e).is_complete_at(watermark));
        }
    }
}
