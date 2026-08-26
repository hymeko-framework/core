//! Phase 7 diagnostic instrumentation: GPU timestamps and readback stage timing.
//!
//! # Scope — this is a measuring instrument, not a feature
//!
//! Everything here exists to answer two open questions from the Phase 6 report:
//!
//! * what does the SpMM compute pass actually cost on the GPU, as opposed to the
//!   submit-to-completion wall time that Phase 6 could only measure (its §5,
//!   "not comparable");
//! * which stage of [`crate::WgpuBackend::read_tensor`] produces the
//!   large-transfer cliff that Appendix A.5 recorded as "observed, not
//!   explained".
//!
//! It is **not** a profiler framework, not a production readback pool, and not a
//! public API. `#[doc(hidden)]` for the same reason [`crate::testing`] is:
//! benches and examples are separate crates and cannot reach `pub(crate)`.
//!
//! # The ordinary dispatch path is untouched
//!
//! `PreparedSpmm::encode` continues to call `encode_into(encoder, None)`, which
//! is byte-for-byte what it did before — `timestamp_writes: None` was already
//! the literal in the descriptor. Profiling is a *different caller*
//! (`encode_with_timestamps`), not a runtime mode, so the hot path gains no
//! branch, no allocation and no measurable overhead.
//!
//! # Timestamp semantics, stated rather than assumed
//!
//! * Ticks are raw device counter values; nanoseconds require multiplying by
//!   [`wgpu::Queue::get_timestamp_period`].
//! * Values are only comparable **within one submission on one queue**. This
//!   module never subtracts a timestamp taken in one submit from one taken in
//!   another.
//! * A counter can wrap. [`ElapsedTicks`] therefore reports `None` when the end
//!   value is not greater than the begin value, rather than wrapping into an
//!   enormous positive interval or an underflow panic. A wrap and a genuinely
//!   out-of-order pair are indistinguishable from two samples, and inventing a
//!   number for either would be worse than reporting the sample as unusable.
//! * Query **resolve and readback time is never included** in a reported
//!   interval: the interval is computed from resolved counter values, and the
//!   cost of getting them to the host is outside those two values by
//!   construction.

use std::time::{Duration, Instant};

use hymeko::tensor::compute::{BackendKind, ComputeError};

use crate::WgpuBackend;

/// A pair of raw timestamp ticks and the period needed to interpret them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElapsedTicks {
    pub begin: u64,
    pub end: u64,
    pub period_ns: f32,
}

impl ElapsedTicks {
    /// Elapsed nanoseconds, or `None` when the pair is unusable.
    ///
    /// `None` means `end <= begin`: either the counter wrapped between the two
    /// writes, or they were resolved out of order. Both are real possibilities
    /// and neither can be distinguished from a single sample, so the sample is
    /// discarded rather than guessed at.
    #[must_use]
    pub fn nanos(self) -> Option<f64> {
        let ticks = self.end.checked_sub(self.begin)?;
        if ticks == 0 {
            return None;
        }
        Some(ticks as f64 * f64::from(self.period_ns))
    }

    /// Elapsed microseconds, or `None` — see [`Self::nanos`].
    #[must_use]
    pub fn micros(self) -> Option<f64> {
        self.nanos().map(|ns| ns / 1000.0)
    }
}

/// Why kernel timing is unavailable, when it is.
///
/// Reported rather than papered over: an adapter without timestamp support must
/// yield "unsupported", never an inferred or fabricated kernel time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimestampSupport {
    /// Compute-pass timestamps available (`Features::TIMESTAMP_QUERY`).
    Passes,
    /// Pass timestamps plus `write_timestamp` inside encoders, which is what
    /// timing a buffer *copy* additionally needs
    /// (`Features::TIMESTAMP_QUERY_INSIDE_ENCODERS`).
    PassesAndEncoders,
    /// Neither. Kernel and copy time are not measurable on this adapter.
    Unsupported,
}

impl TimestampSupport {
    #[must_use]
    pub const fn passes(self) -> bool {
        matches!(self, Self::Passes | Self::PassesAndEncoders)
    }

    #[must_use]
    pub const fn encoders(self) -> bool {
        matches!(self, Self::PassesAndEncoders)
    }

    /// Classify what a device's granted features permit.
    #[must_use]
    pub fn from_features(features: wgpu::Features) -> Self {
        let pass = features.contains(wgpu::Features::TIMESTAMP_QUERY);
        let enc = features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
        match (pass, enc) {
            (true, true) => Self::PassesAndEncoders,
            (true, false) => Self::Passes,
            _ => Self::Unsupported,
        }
    }
}

/// Owns a query set and the two buffers needed to get its values to the host.
///
/// One instance is reused across repetitions; creating a query set per
/// measurement would put allocation cost inside the thing being measured.
pub struct GpuTimer {
    set: wgpu::QuerySet,
    /// `QUERY_RESOLVE | COPY_SRC` — `resolve_query_set` writes here.
    resolve: wgpu::Buffer,
    /// `MAP_READ | COPY_DST` — resolved values are copied here to be mapped.
    readback: wgpu::Buffer,
    capacity: u32,
    period_ns: f32,
    support: TimestampSupport,
}

/// Bytes per resolved timestamp, fixed by the wgpu query format.
const TIMESTAMP_BYTES: u64 = 8;

impl GpuTimer {
    /// Create a timer for `capacity` timestamps, or `None` when the device
    /// cannot take them.
    ///
    /// # Panics
    ///
    /// Never. Capacity is clamped to at least 2 so a begin/end pair always fits.
    #[must_use]
    pub fn new(backend: &WgpuBackend, capacity: u32) -> Option<Self> {
        let support = TimestampSupport::from_features(backend.device().features());
        if !support.passes() {
            return None;
        }
        let capacity = capacity.max(2);
        let bytes = u64::from(capacity) * TIMESTAMP_BYTES;
        let device = backend.device();
        Some(Self {
            set: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("phase7.timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: capacity,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("phase7.query_resolve"),
                size: bytes,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("phase7.query_readback"),
                size: bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity,
            period_ns: backend.queue().get_timestamp_period(),
            support,
        })
    }

    #[must_use]
    pub const fn support(&self) -> TimestampSupport {
        self.support
    }

    #[must_use]
    pub const fn period_ns(&self) -> f32 {
        self.period_ns
    }

    #[must_use]
    pub const fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Timestamp writes bracketing a compute pass.
    ///
    /// # Errors
    ///
    /// [`ComputeError::IndexOverflow`] if either index is outside the set — a
    /// bounds error here would otherwise surface as a driver validation abort
    /// with no indication of which index was wrong.
    pub fn pass_writes(
        &self,
        begin: u32,
        end: u32,
    ) -> Result<wgpu::ComputePassTimestampWrites<'_>, ComputeError> {
        self.check_index(begin)?;
        self.check_index(end)?;
        Ok(wgpu::ComputePassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: Some(begin),
            end_of_pass_write_index: Some(end),
        })
    }

    /// Write a timestamp directly into the encoder, between commands.
    ///
    /// Needed to time a buffer copy, which is not a pass.
    ///
    /// # Errors
    ///
    /// [`ComputeError::Backend`] when the device lacks
    /// `TIMESTAMP_QUERY_INSIDE_ENCODERS`, [`ComputeError::IndexOverflow`] when
    /// the index is out of range.
    pub fn write(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        index: u32,
    ) -> Result<(), ComputeError> {
        if !self.support.encoders() {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: "TIMESTAMP_QUERY_INSIDE_ENCODERS not granted; \
                          copy timing is unavailable on this adapter"
                    .to_string(),
            });
        }
        self.check_index(index)?;
        encoder.write_timestamp(&self.set, index);
        Ok(())
    }

    fn check_index(&self, index: u32) -> Result<(), ComputeError> {
        if index >= self.capacity {
            return Err(ComputeError::IndexOverflow {
                what: "timestamp query index beyond query set capacity",
                value: index as usize,
            });
        }
        Ok(())
    }

    /// Record the resolve + copy commands for `0..count` queries.
    ///
    /// Must be encoded after every timestamp write in the same submission, and
    /// before that submission is submitted.
    ///
    /// # Errors
    ///
    /// [`ComputeError::IndexOverflow`] if `count` exceeds capacity.
    pub fn resolve(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        count: u32,
    ) -> Result<(), ComputeError> {
        if count > self.capacity {
            return Err(ComputeError::IndexOverflow {
                what: "timestamp resolve count beyond query set capacity",
                value: count as usize,
            });
        }
        encoder.resolve_query_set(&self.set, 0..count, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(
            &self.resolve,
            0,
            &self.readback,
            0,
            u64::from(count) * TIMESTAMP_BYTES,
        );
        Ok(())
    }

    /// Read `count` resolved ticks back to the host.
    ///
    /// Call only after the submission containing [`Self::resolve`] has
    /// completed. The time this takes is **not** part of any reported interval:
    /// intervals are differences between counter values, which were already
    /// fixed on the device.
    ///
    /// # Errors
    ///
    /// [`ComputeError::Backend`] on map or poll failure.
    pub fn read(&self, backend: &WgpuBackend, count: u32) -> Result<Vec<u64>, ComputeError> {
        let bytes = u64::from(count) * TIMESTAMP_BYTES;
        let slice = self.readback.slice(..bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        backend
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!("poll during timestamp readback failed: {e}"),
            })?;
        rx.recv()
            .map_err(|e| ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!("timestamp map callback never fired: {e}"),
            })?
            .map_err(|e| ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!("timestamp buffer map failed: {e}"),
            })?;
        let view = slice
            .get_mapped_range()
            .map_err(|e| ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!("timestamp mapped range unavailable: {e}"),
            })?;
        let ticks: Vec<u64> = bytemuck::cast_slice::<u8, u64>(&view).to_vec();
        drop(view);
        self.readback.unmap();
        Ok(ticks)
    }

    /// Convenience: interpret two of the ticks returned by [`Self::read`].
    #[must_use]
    pub fn elapsed(&self, ticks: &[u64], begin: usize, end: usize) -> Option<ElapsedTicks> {
        Some(ElapsedTicks {
            begin: *ticks.get(begin)?,
            end: *ticks.get(end)?,
            period_ns: self.period_ns,
        })
    }
}

// ---------------------------------------------------------------------------
// Readback stage decomposition
// ---------------------------------------------------------------------------

/// One readback, decomposed into the stages Phase 7 must separate.
///
/// Every field is measured at its own boundary. **No component is derived by
/// subtracting one aggregate from another** — that would attribute the
/// difference of two noisy numbers to whichever stage was not measured.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReadbackTiming {
    /// R1 — staging buffer creation. Zero when a buffer is reused.
    pub alloc_us: f64,
    /// R2 — GPU-side copy, from timestamps. `None` when the adapter cannot
    /// write timestamps inside encoders.
    pub gpu_copy_us: Option<f64>,
    /// R3 — submit plus wait for GPU completion, host-observed.
    pub submit_wait_us: f64,
    /// R4 — `map_async` until its callback has fired, host-observed.
    pub map_us: f64,
    /// R5 — copying the mapped range into a `Vec`.
    pub memcpy_us: f64,
    /// R6 — the whole operation, measured end to end independently.
    pub total_us: f64,
    /// Bytes transferred, for throughput.
    pub bytes: u64,
    /// Whether this repetition allocated a fresh staging buffer.
    pub fresh_staging: bool,
    /// Whether the destination `Vec` was preallocated and reused.
    pub dest_reused: bool,
    /// Copy chunk size in bytes; `None` for a single copy.
    pub chunk_bytes: Option<u64>,
}

impl ReadbackTiming {
    /// End-to-end throughput in GB/s (10^9 bytes/s, matching the CUDA report).
    #[must_use]
    pub fn total_gbps(&self) -> f64 {
        if self.total_us <= 0.0 {
            return 0.0;
        }
        self.bytes as f64 / (self.total_us * 1e-6) / 1e9
    }

    /// Stages that were measured directly, summed. Deliberately *not* used to
    /// derive anything: it is reported alongside `total_us` so a large gap
    /// between them is visible as unattributed time rather than silently
    /// absorbed into a component.
    #[must_use]
    pub fn accounted_us(&self) -> f64 {
        self.alloc_us + self.submit_wait_us + self.map_us + self.memcpy_us
    }
}

/// Reusable staging buffer with the ownership rule enforced in code.
///
/// # The rule
///
/// A staging buffer must not be re-copied into or re-mapped while the GPU or a
/// previous mapping still owns it. Violating that is a validation error at best
/// and silent corruption at worst, so `mapped` is tracked and
/// [`Self::begin_reuse`] refuses rather than trusting the caller to unmap.
pub struct StagingBuffer {
    buffer: wgpu::Buffer,
    size: u64,
    mapped: bool,
}

impl StagingBuffer {
    /// Allocate a staging buffer of `size` bytes (minimum 4, as wgpu rejects
    /// zero-sized buffers).
    #[must_use]
    pub fn new(backend: &WgpuBackend, size: u64) -> Self {
        Self {
            buffer: backend.resources().staging_buffer("phase7.staging", size),
            size: size.max(4),
            mapped: false,
        }
    }

    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    #[must_use]
    pub const fn is_mapped(&self) -> bool {
        self.mapped
    }

    /// Check the buffer is large enough and not still mapped.
    ///
    /// # Errors
    ///
    /// [`ComputeError::Backend`] if still mapped, [`ComputeError::ShapeMismatch`]
    /// if too small for the requested transfer.
    pub fn begin_reuse(&self, bytes: u64) -> Result<(), ComputeError> {
        if self.mapped {
            return Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: "staging buffer is still mapped; unmap before reusing it".to_string(),
            });
        }
        if bytes > self.size {
            return Err(ComputeError::ShapeMismatch {
                operand: "staging buffer smaller than the requested transfer",
                expected: self.size as usize,
                actual: bytes as usize,
            });
        }
        Ok(())
    }
}

/// Optional knobs for [`timed_readback`].
///
/// A config struct rather than six positional parameters: the axes here are
/// orthogonal (staging reuse × chunking × timing × destination reuse) and a
/// Cartesian family of `timed_readback_chunked_reused_*` functions is exactly
/// the anti-pattern this codebase forbids.
#[derive(Default)]
pub struct ReadbackPlan<'a> {
    /// Reuse this staging buffer instead of allocating a fresh one.
    pub staging: Option<&'a mut StagingBuffer>,
    /// Split the device-to-staging copy into this many bytes per
    /// `copy_buffer_to_buffer` call, all within one encoder and one submit —
    /// testing whether the driver's path is sensitive to per-copy size rather
    /// than to total size.
    pub chunk_bytes: Option<u64>,
    /// Write timestamps around the copy to obtain R2.
    pub timer: Option<&'a GpuTimer>,
    /// Copy into this already-allocated buffer instead of allocating a fresh
    /// `Vec`. Separates the cost of *reading* the mapped range from the cost of
    /// allocating and first-touching the destination — without this, R5
    /// conflates the two and cannot attribute either.
    pub dest: Option<&'a mut Vec<f32>>,
}

/// Time a full readback, stage by stage.
///
/// Returns the data as a `Vec`, except when [`ReadbackPlan::dest`] is set, in
/// which case the data lands in that buffer and an empty `Vec` is returned.
///
/// # Errors
///
/// [`ComputeError`] from any stage; a failure is a defect, not a slow path.
///
/// # Panics
///
/// Never: all size arithmetic is checked.
pub fn timed_readback(
    backend: &WgpuBackend,
    source: &wgpu::Buffer,
    bytes: u64,
    plan: ReadbackPlan<'_>,
) -> Result<(Vec<f32>, ReadbackTiming), ComputeError> {
    let ReadbackPlan {
        staging,
        chunk_bytes,
        timer,
        dest,
    } = plan;
    let mut t = ReadbackTiming {
        bytes,
        fresh_staging: staging.is_none(),
        dest_reused: dest.is_some(),
        chunk_bytes,
        ..ReadbackTiming::default()
    };
    let overall = Instant::now();

    // Zero-length: no buffer, no copy, no map. Reported as a real measurement
    // with zeroed stages rather than an error, because a zero-element tensor is
    // legal everywhere else in this crate.
    if bytes == 0 {
        t.total_us = us(overall.elapsed());
        return Ok((Vec::new(), t));
    }

    // --- R1: staging allocation ------------------------------------------
    let a0 = Instant::now();
    let mut owned;
    let staging_ref: &mut StagingBuffer = match staging {
        Some(s) => {
            s.begin_reuse(bytes)?;
            s
        }
        None => {
            owned = Some(StagingBuffer::new(backend, bytes));
            owned.as_mut().expect("just constructed")
        }
    };
    t.alloc_us = us(a0.elapsed());

    // --- encode: optional begin timestamp, copies, optional end timestamp -
    let mut encoder = backend.encoder(Some("phase7.readback"));
    let use_gpu_timer = timer.is_some_and(|g| g.support().encoders());
    if use_gpu_timer {
        timer.expect("checked").write(&mut encoder, 0)?;
    }

    match chunk_bytes {
        None => encoder.copy_buffer_to_buffer(source, 0, &staging_ref.buffer, 0, bytes),
        Some(chunk) => {
            let chunk = chunk.max(4);
            let mut offset = 0u64;
            while offset < bytes {
                let remaining = bytes - offset;
                let this = remaining.min(chunk);
                encoder.copy_buffer_to_buffer(source, offset, &staging_ref.buffer, offset, this);
                offset = offset
                    .checked_add(this)
                    .ok_or(ComputeError::IndexOverflow {
                        what: "chunked readback offset",
                        value: usize::MAX,
                    })?;
            }
        }
    }

    if use_gpu_timer {
        let g = timer.expect("checked");
        g.write(&mut encoder, 1)?;
        g.resolve(&mut encoder, 2)?;
    }

    // --- R3: submit + wait -------------------------------------------------
    let s0 = Instant::now();
    let index = backend.submit(encoder);
    backend.wait(index)?;
    t.submit_wait_us = us(s0.elapsed());

    // --- R4: map_async until callback ------------------------------------
    let m0 = Instant::now();
    let slice = staging_ref.buffer.slice(..bytes);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
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
        })?;
    staging_ref.mapped = true;
    t.map_us = us(m0.elapsed());

    // --- R5: mapped range -> Vec -----------------------------------------
    // Timed with the destination already allocated when `dest` is supplied, so
    // the interval is the copy alone; otherwise it also includes allocating and
    // first-touching a fresh Vec, and the two cannot be told apart.
    let c0 = Instant::now();
    let view = slice
        .get_mapped_range()
        .map_err(|e| ComputeError::Backend {
            backend: BackendKind::Wgpu,
            message: format!("mapped range unavailable: {e}"),
        })?;
    let src: &[f32] = bytemuck::cast_slice(&view);
    let out: Vec<f32> = match dest {
        Some(buf) => {
            buf.clear();
            buf.extend_from_slice(src);
            t.memcpy_us = us(c0.elapsed());
            // The data is in `buf`; returning a clone here would add an
            // allocation to R6 and make it incomparable with the fresh-dest
            // variant. The caller validates through its own buffer.
            Vec::new()
        }
        None => {
            let v = src.to_vec();
            t.memcpy_us = us(c0.elapsed());
            v
        }
    };
    drop(view);
    staging_ref.buffer.unmap();
    staging_ref.mapped = false;

    // --- R2: GPU copy, from resolved timestamps ---------------------------
    if use_gpu_timer {
        let g = timer.expect("checked");
        let ticks = g.read(backend, 2)?;
        t.gpu_copy_us = g.elapsed(&ticks, 0, 1).and_then(ElapsedTicks::micros);
    }

    t.total_us = us(overall.elapsed());
    Ok((out, t))
}

fn us(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

// ---------------------------------------------------------------------------
// Crate-internal bridges, exposed only through this doc(hidden) module
// ---------------------------------------------------------------------------

/// Encode a tracked SpMM with timestamps around its compute pass.
///
/// `PreparedSpmm::encode_with_timestamps` is `pub(crate)` deliberately — kernel
/// instrumentation is not public API. This thin bridge lets the Phase 7 probe,
/// which is a separate crate, reach it without widening the operation's own
/// surface.
///
/// # Errors
///
/// As [`crate::PreparedSpmm::encode`].
pub fn encode_spmm_profiled(
    op: &crate::PreparedSpmm<'_, crate::Tracked>,
    session: &mut crate::TrackedSession<'_>,
    writes: wgpu::ComputePassTimestampWrites<'_>,
) -> Result<(), ComputeError> {
    op.encode_with_timestamps(session, writes)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_converts_ticks_with_the_period() {
        let e = ElapsedTicks {
            begin: 1_000,
            end: 2_000,
            period_ns: 2.5,
        };
        // 1000 ticks * 2.5 ns = 2500 ns = 2.5 us
        assert_eq!(e.nanos(), Some(2500.0));
        assert_eq!(e.micros(), Some(2.5));
    }

    /// A wrapped or out-of-order pair must yield `None`, never a huge positive
    /// interval from wrapping subtraction and never a panic.
    #[test]
    fn elapsed_rejects_wrapped_or_out_of_order_pairs() {
        let wrapped = ElapsedTicks {
            begin: u64::MAX - 10,
            end: 5,
            period_ns: 1.0,
        };
        assert_eq!(wrapped.nanos(), None, "wrap must not produce a number");

        let equal = ElapsedTicks {
            begin: 42,
            end: 42,
            period_ns: 1.0,
        };
        assert_eq!(equal.nanos(), None, "a zero interval is not a measurement");
    }

    #[test]
    fn support_classification_matches_features() {
        use wgpu::Features as F;
        assert_eq!(
            TimestampSupport::from_features(F::empty()),
            TimestampSupport::Unsupported
        );
        assert_eq!(
            TimestampSupport::from_features(F::TIMESTAMP_QUERY),
            TimestampSupport::Passes
        );
        assert_eq!(
            TimestampSupport::from_features(
                F::TIMESTAMP_QUERY | F::TIMESTAMP_QUERY_INSIDE_ENCODERS
            ),
            TimestampSupport::PassesAndEncoders
        );
        // INSIDE_ENCODERS without the base feature is not usable for passes.
        assert_eq!(
            TimestampSupport::from_features(F::TIMESTAMP_QUERY_INSIDE_ENCODERS),
            TimestampSupport::Unsupported
        );
    }

    #[test]
    fn support_predicates_are_consistent() {
        assert!(TimestampSupport::Passes.passes());
        assert!(!TimestampSupport::Passes.encoders());
        assert!(TimestampSupport::PassesAndEncoders.passes());
        assert!(TimestampSupport::PassesAndEncoders.encoders());
        assert!(!TimestampSupport::Unsupported.passes());
        assert!(!TimestampSupport::Unsupported.encoders());
    }

    #[test]
    fn timing_throughput_and_accounting() {
        let t = ReadbackTiming {
            alloc_us: 1.0,
            gpu_copy_us: Some(10.0),
            submit_wait_us: 20.0,
            map_us: 5.0,
            memcpy_us: 4.0,
            total_us: 40.0,
            bytes: 1_000_000,
            fresh_staging: true,
            dest_reused: false,
            chunk_bytes: None,
        };
        // 1e6 bytes in 40 us = 25 GB/s
        assert!((t.total_gbps() - 25.0).abs() < 1e-9);
        // Accounted is the sum of directly measured host stages only.
        assert!((t.accounted_us() - 30.0).abs() < 1e-9);
        assert_eq!(t.total_us - t.accounted_us(), 10.0);
    }

    #[test]
    fn zero_total_reports_zero_throughput_rather_than_dividing() {
        let t = ReadbackTiming {
            bytes: 1024,
            ..ReadbackTiming::default()
        };
        assert_eq!(t.total_gbps(), 0.0);
    }
}
