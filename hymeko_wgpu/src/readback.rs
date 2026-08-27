//! The readback state machine — one implementation, several callers.
//!
//! # Why this module exists
//!
//! Phase 7 measured where device-to-host readback time actually goes. The
//! dominant cost was not the transfer: it was **allocating the host
//! destination**. Copying the same bytes out of the same mapped range ran at
//! 3.7–4.0 GB/s into a fresh `Vec` and 20–22 GB/s into a preallocated one
//! (7.4× on the Turing host). Staging allocation was a second, smaller fixed
//! cost — 4.3 ms on Windows, 18.3 ms on Linux, independent of size.
//!
//! Both are ownership problems, not transfer problems. [`ReadbackWorkspace`] is
//! the explicit carrier of that ownership: it holds the two reusable buffers
//! and nothing else. It is **not** a second readback path — it is the same
//! [`readback_into`] engine with the caller supplying storage that outlives the
//! call instead of storage created and dropped inside it.
//!
//! # The single-mechanism rule
//!
//! ```text
//!   WgpuBackend::read_tensor       (legacy, owns a temporary workspace)
//!   WgpuBackend::read_tensor_into  (reusable, caller owns the workspace)
//!   profiling::timed_readback      (shares every step, own clocks between)
//!            |                |               |
//!            +----------------+---------------+
//!                             v
//!                      readback_into()          <- steps 1-8
//!                             |
//!             +---------------+---------------+
//!             v                               v
//!      map_blocking()                 drain_mapped_into()
//!      (steps 5)                      (steps 6,7,8 - also used for u64
//!                                      timestamp ticks by GpuTimer::read)
//! ```
//!
//! No caller re-implements shape validation, staging growth, map, mapped-range
//! validation, the byte copy, or unmap.
//!
//! The stage-decomposing profiling probe does not call [`readback_into`]: its
//! whole purpose is to time the boundaries *between* the steps, and it must
//! write GPU timestamps between the encode and the submit. It therefore calls
//! the same step primitives in the same order with its own clocks interleaved.
//! That is the one place the composition is written twice, and it is inherent
//! rather than incidental — a measurement that delegated the composition could
//! not report the components. It is declared as intentional in the Phase 8
//! report. Production carries no clock, no counter and no timestamp query.
//!
//! # What this is not
//!
//! Not a pool. There is no global cache, no free-list, no background trimming,
//! no mutex and no implicit sharing. A workspace is an ordinary owned value;
//! two threads reading concurrently hold two workspaces, and the borrow checker
//! enforces that rather than a runtime lock.

use hymeko::tensor::compute::{BackendKind, ComputeError, TensorShape};

use crate::WgpuBackend;

/// Bytes per `f32` element, named so the arithmetic below reads as intent.
const F32_BYTES: usize = std::mem::size_of::<f32>();

// ---------------------------------------------------------------------------
// Step 1 — shape and byte-count validation
// ---------------------------------------------------------------------------

/// Element count and byte length for a readback, both checked.
///
/// Rejects an element count that overflows `usize`, a byte length that
/// overflows `u64`, and a transfer larger than the device will allocate. The
/// last one matters: without it an oversized request reaches wgpu as a
/// validation error whose message does not name the limit it broke.
///
/// # Errors
///
/// [`ComputeError::IndexOverflow`] on either overflow;
/// [`ComputeError::ShapeMismatch`] when the transfer exceeds `max_buffer_size`.
pub(crate) fn readback_extent(
    shape: TensorShape,
    max_buffer_size: u64,
) -> Result<(usize, u64), ComputeError> {
    let elements = shape
        .checked_elements()
        .ok_or(ComputeError::IndexOverflow {
            what: "tensor elements",
            value: usize::MAX,
        })?;

    let bytes = elements
        .checked_mul(F32_BYTES)
        .and_then(|b| u64::try_from(b).ok())
        .ok_or(ComputeError::IndexOverflow {
            what: "readback byte length",
            value: elements,
        })?;

    if bytes > max_buffer_size {
        return Err(ComputeError::ShapeMismatch {
            operand: "readback exceeds the device maximum buffer size",
            expected: usize::try_from(max_buffer_size).unwrap_or(usize::MAX),
            actual: usize::try_from(bytes).unwrap_or(usize::MAX),
        });
    }

    Ok((elements, bytes))
}

// ---------------------------------------------------------------------------
// Step 2 — grow-only staging storage
// ---------------------------------------------------------------------------

/// A reusable device staging buffer that grows and never implicitly shrinks.
///
/// # Ownership rule
///
/// A staging buffer must not be copied into or mapped while a previous mapping
/// still owns it. That is enforced structurally rather than by convention: the
/// engine unmaps before returning on **every** path, including error paths, and
/// a mapping failure discards the buffer outright so the next call cannot
/// inherit an indeterminate state.
#[derive(Default)]
pub struct StagingStorage {
    buffer: Option<wgpu::Buffer>,
    capacity: u64,
    /// Cumulative device allocations. Structural evidence of reuse for tests
    /// that must not depend on timing.
    allocations: u64,
}

impl StagingStorage {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: None,
            capacity: 0,
            allocations: 0,
        }
    }

    /// A storage pre-sized to `bytes` of device capacity.
    ///
    /// Ordinary use does not need this — capacity grows on demand. It exists
    /// for the diagnostic probe, which must vary allocated capacity
    /// independently of transfer size to reproduce the Phase 7 finding that on
    /// one host the two are not the same thing.
    #[must_use]
    pub fn with_capacity(backend: &WgpuBackend, bytes: u64) -> Self {
        let mut s = Self::new();
        if bytes > 0 {
            let _ = s.ensure(backend, bytes);
        }
        s
    }

    /// Current device capacity in bytes. Zero before the first readback.
    #[must_use]
    pub const fn capacity(&self) -> u64 {
        self.capacity
    }

    /// How many device buffers this storage has allocated over its lifetime.
    ///
    /// The reuse gate in the Phase 8 tests: a second same-size readback must
    /// leave this unchanged.
    #[must_use]
    pub const fn allocations(&self) -> u64 {
        self.allocations
    }

    /// Ensure at least `bytes` of capacity, allocating only when it is short.
    ///
    /// Grow-only: a smaller request after a larger one reuses the larger buffer
    /// rather than reallocating down. Callers that genuinely want the memory
    /// back drop the workspace.
    pub(crate) fn ensure(&mut self, backend: &WgpuBackend, bytes: u64) -> &wgpu::Buffer {
        if self.buffer.is_none() || self.capacity < bytes {
            // `staging_buffer` already applies the 4-byte floor wgpu requires.
            let buffer = backend
                .resources()
                .staging_buffer("readback.staging", bytes);
            self.capacity = buffer.size();
            self.buffer = Some(buffer);
            self.allocations += 1;
        }
        self.buffer.as_ref().expect("just ensured present")
    }

    /// The device buffer, once allocated.
    #[must_use]
    pub(crate) const fn buffer(&self) -> Option<&wgpu::Buffer> {
        self.buffer.as_ref()
    }

    /// Drop the device buffer after an indeterminate mapping.
    ///
    /// Called only when `map_async` reported failure, where wgpu gives no way
    /// to establish whether the buffer is mapped. Discarding is cheap and
    /// leaves the storage in the same state as a fresh one, which is the
    /// "valid reusable" half of the failure contract.
    pub(crate) fn discard(&mut self) {
        self.buffer = None;
        self.capacity = 0;
    }
}

// ---------------------------------------------------------------------------
// Steps 5-8 — map, validate, copy out, unmap
// ---------------------------------------------------------------------------

/// Issue `map_async` and block until its callback has fired.
///
/// Split out because three callers need exactly this and nothing around it:
/// the engine, the timestamped profiling probe, and [`crate::profiling::GpuTimer::read`].
///
/// # Errors
///
/// [`ComputeError::Backend`] if the poll fails, the callback never fires, or
/// the mapping itself is refused.
pub(crate) fn map_blocking(
    backend: &WgpuBackend,
    slice: wgpu::BufferSlice<'_>,
) -> Result<(), ComputeError> {
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });

    // `PollType::Wait` blocks in the driver rather than spinning.
    backend
        .device()
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| ComputeError::Backend {
            backend: BackendKind::Wgpu,
            message: format!("poll during readback failed: {e}"),
        })?;

    rx.recv()
        .map_err(|e| ComputeError::Backend {
            backend: BackendKind::Wgpu,
            message: format!("map callback never fired: {e}"),
        })?
        .map_err(|e| ComputeError::Backend {
            backend: BackendKind::Wgpu,
            message: format!("buffer map failed: {e}"),
        })
}

/// Copy a mapped range into `dest` and unmap, whatever happens.
///
/// Generic over the element type so the `f32` tensor path and the `u64`
/// timestamp path share one implementation instead of two that must be kept
/// byte-compatible by inspection.
///
/// `dest` is cleared and refilled, so it ends with exactly the copied elements
/// and keeps whatever capacity it had — that reuse is the entire point.
///
/// # Postconditions
///
/// The buffer is unmapped on every return path, success or failure.
///
/// # Errors
///
/// [`ComputeError::Backend`] if the mapped range is unavailable or is not
/// validly aligned and sized for `T`.
pub(crate) fn drain_mapped_into<T: bytemuck::Pod>(
    buffer: &wgpu::Buffer,
    slice: wgpu::BufferSlice<'_>,
    dest: &mut Vec<T>,
) -> Result<usize, ComputeError> {
    // Scoped so the view — which borrows the mapping — is dropped before the
    // unmap below, on both the success and the failure path.
    let copied = {
        match slice.get_mapped_range() {
            Ok(view) => {
                // Checked rather than `cast_slice`, which panics on a bad
                // length or alignment. A staging buffer should always satisfy
                // both; a panic here would abort a caller's process over a
                // condition that is reportable.
                match bytemuck::try_cast_slice::<u8, T>(&view) {
                    Ok(src) => {
                        dest.clear();
                        dest.extend_from_slice(src);
                        Ok(src.len())
                    }
                    Err(e) => Err(ComputeError::Backend {
                        backend: BackendKind::Wgpu,
                        message: format!(
                            "mapped range is not a valid slice of the element type: {e}"
                        ),
                    }),
                }
            }
            Err(e) => Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!("mapped range unavailable: {e}"),
            }),
        }
    };
    buffer.unmap();
    copied
}

// ---------------------------------------------------------------------------
// Steps 1-8 — the engine
// ---------------------------------------------------------------------------

/// Copy `bytes` from `source` into `dest`, reusing `staging`.
///
/// The one readback state machine. Everything that reads a tensor back to the
/// host goes through here.
///
/// Returns the number of `f32` elements written into `dest`.
///
/// # Preconditions
///
/// `bytes` must already have been validated by [`readback_extent`], and
/// `source` must have been created with `COPY_SRC`.
///
/// # Postconditions
///
/// `dest` holds exactly `bytes / 4` elements. The staging buffer is unmapped.
/// On failure the staging storage is left either untouched or emptied — never
/// holding a buffer whose mapping state is unknown.
///
/// # Errors
///
/// [`ComputeError::Backend`] from the submit, poll, map or mapped-range steps.
pub(crate) fn readback_into(
    backend: &WgpuBackend,
    source: &wgpu::Buffer,
    bytes: u64,
    staging: &mut StagingStorage,
    dest: &mut Vec<f32>,
) -> Result<usize, ComputeError> {
    if bytes == 0 {
        dest.clear();
        return Ok(0);
    }

    // Step 2 — staging capacity.
    let buffer = staging.ensure(backend, bytes);

    // Steps 3 and 4 — encode the copy, submit it, wait for completion.
    let mut encoder = backend.encoder(Some("readback"));
    encoder.copy_buffer_to_buffer(source, 0, buffer, 0, bytes);
    let index = backend.submit(encoder);
    backend.wait(index)?;

    // Step 5 — map. A failure here leaves the mapping state indeterminate, so
    // the buffer is discarded rather than reused.
    let slice = buffer.slice(..bytes);
    if let Err(e) = map_blocking(backend, slice) {
        staging.discard();
        return Err(e);
    }

    // Steps 6-8 — validate the range, copy out, unmap. `drain_mapped_into`
    // unmaps on every path, so no error return can leave the buffer mapped.
    let buffer = staging.buffer.as_ref().expect("ensured above");
    drain_mapped_into(buffer, buffer.slice(..bytes), dest)
}

// ---------------------------------------------------------------------------
// The caller-owned workspace
// ---------------------------------------------------------------------------

/// Reusable storage for repeated readbacks: device staging plus host
/// destination.
///
/// # Ownership and lifetime
///
/// The workspace owns both buffers and is owned by the caller. Nothing global
/// is involved. [`crate::WgpuBackend::read_tensor_into`] borrows it mutably and
/// returns a slice borrowed from it, so the borrow checker — not a runtime
/// check — prevents starting a second readback while the first result is still
/// in use:
///
/// ```compile_fail
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use hymeko::tensor::compute::TensorShape;
/// use hymeko_wgpu::WgpuBackend;
/// let backend = WgpuBackend::new()?;
/// let res = backend.resources();
/// let a = res.upload_tensor("a", TensorShape::new(2, 1), &[1.0, 2.0])?;
/// let b = res.upload_tensor("b", TensorShape::new(2, 1), &[3.0, 4.0])?;
/// let mut ws = backend.create_readback_workspace();
///
/// let first = backend.read_tensor_into(&a, &mut ws)?;
/// // Second readback while `first` still borrows the workspace: rejected.
/// let second = backend.read_tensor_into(&b, &mut ws)?;
/// println!("{first:?} {second:?}");
/// # Ok(())
/// # }
/// ```
///
/// Sequentially, which is the intended use, it compiles:
///
/// ```no_run
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use hymeko::tensor::compute::TensorShape;
/// use hymeko_wgpu::WgpuBackend;
/// let backend = WgpuBackend::new()?;
/// let res = backend.resources();
/// let a = res.upload_tensor("a", TensorShape::new(2, 1), &[1.0, 2.0])?;
/// let mut ws = backend.create_readback_workspace();
///
/// let first = backend.read_tensor_into(&a, &mut ws)?.to_vec();
/// let second = backend.read_tensor_into(&a, &mut ws)?;
/// assert_eq!(first, second);
/// # Ok(())
/// # }
/// ```
///
/// # Capacity
///
/// Grow-only. Both buffers grow to the largest readback seen and are then
/// reused; a smaller readback afterwards allocates nothing. Neither shrinks
/// implicitly — dropping the workspace is how the memory is returned, and that
/// keeps the cost model visible at the call site.
///
/// # Concurrency
///
/// Not shared. Parallel readbacks use one workspace each. There is no mutex
/// because there is nothing to contend for.
#[derive(Default)]
pub struct ReadbackWorkspace {
    staging: StagingStorage,
    destination: Vec<f32>,
}

impl ReadbackWorkspace {
    /// An empty workspace. Both buffers are allocated on first use.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            staging: StagingStorage::new(),
            destination: Vec::new(),
        }
    }

    /// Pre-size both buffers for a known output length.
    ///
    /// Useful when the largest readback is known up front — an inference loop
    /// whose output shape is fixed — because it moves both allocations out of
    /// the first timed iteration. Purely an optimisation: the workspace grows
    /// on demand without it.
    ///
    /// Reserving is **not** a device-specific tuning knob. Phase 7 found that
    /// on one host a larger *staging* allocation changed the driver's transfer
    /// rate; that observation is deliberately not exploited here, because it
    /// did not reproduce on the other host and encoding it would be a
    /// hardware-specific heuristic in a portable path.
    ///
    /// # Errors
    ///
    /// As [`readback_extent`]: overflow, or a request beyond the device limit.
    pub fn reserve(&mut self, backend: &WgpuBackend, elements: usize) -> Result<(), ComputeError> {
        let (elements, bytes) = readback_extent(
            TensorShape::new(elements, 1),
            backend.capabilities().max_buffer_size,
        )?;
        if bytes > 0 {
            let _ = self.staging.ensure(backend, bytes);
        }
        if elements > self.destination.capacity() {
            self.destination.reserve(elements - self.destination.len());
        }
        Ok(())
    }

    /// Device staging capacity in bytes.
    #[must_use]
    pub const fn staging_capacity(&self) -> u64 {
        self.staging.capacity()
    }

    /// Device staging allocations performed so far.
    ///
    /// Exposed so reuse can be asserted structurally instead of through a
    /// timing threshold, which would be flaky.
    #[must_use]
    pub const fn staging_allocations(&self) -> u64 {
        self.staging.allocations()
    }

    /// Host destination capacity in elements.
    #[must_use]
    pub fn destination_capacity(&self) -> usize {
        self.destination.capacity()
    }

    /// Split borrow for the engine.
    pub(crate) fn parts(&mut self) -> (&mut StagingStorage, &mut Vec<f32>) {
        (&mut self.staging, &mut self.destination)
    }

    /// Take the result buffer, leaving the workspace to regrow a destination
    /// while keeping its staging buffer.
    ///
    /// Use when a caller must keep one result while continuing to read into the
    /// same workspace. The device staging buffer — the more expensive of the
    /// two — is retained; only the host destination is surrendered.
    ///
    /// This is also what lets the legacy owned-`Vec` API return without
    /// copying: the buffer is moved out rather than cloned.
    pub fn take_result(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extent_rejects_transfers_beyond_the_device_limit() {
        // 1024 elements = 4096 bytes, against a 4095-byte device limit.
        let err = readback_extent(TensorShape::new(1024, 1), 4095)
            .expect_err("a transfer past the device limit must be refused");
        assert!(
            matches!(err, ComputeError::ShapeMismatch { .. }),
            "expected ShapeMismatch, got {err:?}"
        );
        // Exactly at the limit is allowed.
        assert_eq!(
            readback_extent(TensorShape::new(1024, 1), 4096).expect("exact fit"),
            (1024, 4096)
        );
    }

    #[test]
    fn extent_rejects_element_counts_that_overflow_a_byte_length() {
        let err = readback_extent(TensorShape::new(usize::MAX, 2), u64::MAX)
            .expect_err("an overflowing element count must be refused");
        assert!(
            matches!(err, ComputeError::IndexOverflow { .. }),
            "expected IndexOverflow, got {err:?}"
        );
    }

    #[test]
    fn extent_of_an_empty_shape_is_zero_bytes() {
        assert_eq!(
            readback_extent(TensorShape::new(0, 8), 1024).expect("empty is legal"),
            (0, 0)
        );
    }

    #[test]
    fn fresh_storage_reports_no_capacity_and_no_allocations() {
        let s = StagingStorage::new();
        assert_eq!(s.capacity(), 0);
        assert_eq!(s.allocations(), 0);
    }

    #[test]
    fn a_new_workspace_holds_nothing() {
        let w = ReadbackWorkspace::new();
        assert_eq!(w.staging_capacity(), 0);
        assert_eq!(w.staging_allocations(), 0);
        assert_eq!(w.destination_capacity(), 0);
    }
}
