//! `TensorArena` — deterministic reuse of intermediate GPU buffers.
//!
//! # What this is for
//!
//! Phase 3 showed that a resident chain is the right execution model, and
//! `docs/compute-graph-decision.md` argued that the strongest reason to build
//! anything further is **memory**, not scheduling: an 8-layer chain at
//! 200 000 × 128 f32 holds ~819 MiB of intermediates that reuse reduces to
//! ~205 MiB. This module is that reuse, and deliberately nothing else.
//!
//! It is **not** a compute graph. It does not know what an operation is, does
//! not schedule, does not decide when to submit, and does not own the device.
//! It hands out buffers and takes them back.
//!
//! # The three states a buffer can be in
//!
//! ```text
//!   free ──acquire()──> leased ──drop(lease)──> retired ──completion──> free
//!    ▲                                                                    │
//!    └────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! * **free** — owned by the pool, referenced by nothing, immediately reusable.
//! * **leased** — an [`ArenaTensor`] exists. The pool cannot hand it out again,
//!   enforced by ownership: the pool no longer holds the buffer at all.
//! * **retired** — the lease is gone, but the GPU may still be reading it. The
//!   slab carries the [`SubmissionEpoch`] that must complete before it is safe.
//!
//! The retired state is the whole reason this module is not fifty lines.
//!
//! # When a retired buffer becomes reusable
//!
//! A released buffer is tagged with the epoch after which no submitted work can
//! still reference it:
//!
//! * **no arena encoder open** — tag with the latest *issued* epoch. Everything
//!   that could reference the buffer has already been submitted.
//! * **an arena encoder is open** — the buffer may have been encoded into
//!   commands not yet submitted, so tagging with the latest issued epoch would
//!   free it too early. It is held *pending* instead and receives the epoch of
//!   the next [`TensorArena::submit`].
//!
//! That second case is the one a naive pool gets wrong, and it is not
//! hypothetical: `acquire → encode → drop lease → submit` is a perfectly natural
//! order to write, and it is exactly the order that corrupts results if the
//! buffer is recycled on release.
//!
//! Tracking it requires knowing when an encoder is open, which is why
//! [`TensorArena::session`] exists.
//!
//! In Phase 4 a caller could step outside that tracking by using
//! `backend.encoder()` with arena tensors. Phase 5 closes it: see
//! [`crate::scope`]. An operation touching a pooled buffer has scope
//! [`crate::Tracked`], and the only `encode` for that scope takes
//! [`TrackedSession`], so there is no untracked path left to take.
//!
//! # What is enforced where
//!
//! | invariant | mechanism |
//! |---|---|
//! | two live leases never share an allocation | ownership — a leased buffer is *moved out* of the pool |
//! | a prepared operation cannot outlive its lease | lifetimes -- `Prepared*<'a, S>` borrows the operand |
//! | a pooled operand reaches a tracked submission | type-level scope ([`crate::scope`]) |
//! | in-flight buffers are not recycled | epoch watermark ([`crate::CompletionTracker`]) |
//! | no cross-device reuse | structural — `acquire` is the only source of slabs and creates them on the borrowed backend |
//! | double release impossible | `Drop` consumes; the lease is not `Clone` |
//! | arena drop needs no unsafe | `Rc<RefCell<..>>` + `Drop`, no raw pointers |
//!
//! # Deliberate non-features (first version)
//!
//! No background thread. No automatic defragmentation. No suballocation from a
//! large buffer — whole `wgpu::Buffer` objects only. No best-fit search without
//! a measurement justifying it. Each of these is a real technique; none is
//! justified by a measurement yet, and adding them now would be optimising
//! before profiling.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use hymeko::tensor::compute::{BackendKind, ComputeError, TensorShape};

use crate::backend::WgpuBackend;
use crate::completion::{CompletionTracker, SubmissionEpoch};
use crate::resources::GpuTensor;

/// How a request is matched against cached allocations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReusePolicy {
    /// Reuse only a byte-identical allocation.
    ///
    /// The default, and the right first choice: it makes reuse decisions
    /// obvious, wastes nothing, and a layered chain reuses heavily under it
    /// because every layer of a fixed feature dimension asks for the same size.
    #[default]
    ExactSize,
    /// Reuse an allocation from the same power-of-two size class, provided it is
    /// at least as large as the request.
    ///
    /// Trades internal fragmentation (up to 2×) for hit rate on ragged sizes.
    /// Off by default: on the chain workloads measured, sizes repeat exactly, so
    /// this would spend memory to buy nothing.
    SizeClass,
}

/// Budget and policy for a [`TensorArena`].
///
/// # Preconditions
///
/// Both limits are ceilings on the **free list only**. Live leases and
/// in-flight allocations are never evicted, so a chain that genuinely needs
/// more memory than the budget still runs — the arena simply stops caching.
#[derive(Clone, Copy, Debug)]
pub struct TensorArenaConfig {
    /// Ceiling on bytes held in the free list.
    pub max_cached_bytes: u64,
    /// Ceiling on the number of buffers held in the free list.
    pub max_cached_buffers: usize,
    pub reuse_policy: ReusePolicy,
}

impl Default for TensorArenaConfig {
    /// 512 MiB / 64 buffers.
    ///
    /// Sized against the measured workload rather than picked round: the 8-layer
    /// chain at 200 000 × 128 needs two live 102 MiB intermediates, so a 512 MiB
    /// cache holds a comfortable multiple of the working set without approaching
    /// the Phase 3 host peak.
    fn default() -> Self {
        Self {
            max_cached_bytes: 512 * 1024 * 1024,
            max_cached_buffers: 64,
            reuse_policy: ReusePolicy::ExactSize,
        }
    }
}

/// What the free list is indexed by.
///
/// Usage flags are part of the key, not a compatibility test: `wgpu` fixes a
/// buffer's usage at creation, and binding a buffer for a usage it was not
/// created with is a validation error. Two allocations of equal size but
/// different usage are therefore different resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AllocationKey {
    bytes: u64,
    usage: wgpu::BufferUsages,
}

impl AllocationKey {
    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.bytes
    }

    #[must_use]
    pub const fn usage(self) -> wgpu::BufferUsages {
        self.usage
    }
}

/// Counters describing what the arena has done. Every field is exact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArenaStats {
    /// `acquire` calls satisfied from the free list.
    pub reuse_hits: u64,
    /// `acquire` calls that had to create a `wgpu::Buffer`.
    pub reuse_misses: u64,
    /// `wgpu::Buffer` objects created over the arena's lifetime.
    pub buffers_created: u64,
    /// Buffers dropped because the budget was exceeded.
    pub evictions: u64,
    /// Bytes currently held by live leases.
    pub active_bytes: u64,
    /// Bytes retired but not yet known complete.
    pub in_flight_bytes: u64,
    /// Bytes sitting in the free list.
    pub cached_bytes: u64,
    /// Buffers in the free list.
    pub cached_buffers: usize,
    /// Highest value of `active + in_flight + cached` observed.
    pub peak_arena_bytes: u64,
    /// Sum of all `wgpu::Buffer` sizes the arena has ever created.
    pub total_allocated_bytes: u64,
}

impl ArenaStats {
    /// Bytes the arena currently holds on the device, in any state.
    #[must_use]
    pub const fn device_bytes(&self) -> u64 {
        self.active_bytes + self.in_flight_bytes + self.cached_bytes
    }

    /// Hit rate over all acquires; `None` before the first acquire.
    #[must_use]
    pub fn hit_rate(&self) -> Option<f64> {
        let total = self.reuse_hits + self.reuse_misses;
        (total > 0).then(|| self.reuse_hits as f64 / total as f64)
    }
}

/// One pooled allocation.
struct Slab {
    buffer: wgpu::Buffer,
    bytes: u64,
    usage: wgpu::BufferUsages,
    /// Epoch that must complete before this slab may be handed out again.
    safe_after: SubmissionEpoch,
}

/// Interior state, shared with every outstanding lease so `Drop` can return a
/// buffer without needing `&mut TensorArena`.
///
/// `Rc<RefCell<..>>` rather than a raw pointer: invariant 9 requires the arena
/// to drop without unsafe, and this is the safe construction. It is not global
/// state — one instance per arena, created by the caller, dropped with it.
struct PoolInner {
    config: TensorArenaConfig,
    free: HashMap<AllocationKey, Vec<Slab>>,
    /// Released while a session was open: the epoch is not yet known.
    ///
    /// Each entry records WHICH session was open at release time, because a slab
    /// may be safely re-handed out inside that same session but not to another
    /// one -- see `TrackedSession::acquire`. `None` means the owning session was
    /// ambiguous (two were open at once), which disables same-session reuse.
    pending: Vec<(Option<u64>, Slab)>,
    /// Released with a known epoch, waiting for the watermark to reach it.
    retired: Vec<Slab>,
    stats: ArenaStats,
    open_encoders: usize,
    /// The single open session, when exactly one is open.
    current_session: Option<u64>,
    next_session_id: u64,
}

impl PoolInner {
    fn note_peak(&mut self) {
        let now = self.stats.device_bytes();
        if now > self.stats.peak_arena_bytes {
            self.stats.peak_arena_bytes = now;
        }
    }

    /// Whether a matching slab is available, WITHOUT removing it.
    ///
    /// Separate from `take_free` because the acquire path needs to probe before
    /// deciding to poll; probing with `take_free` would consume the slab.
    fn has_free(&self, key: AllocationKey, want_bytes: u64) -> bool {
        self.free
            .get(&key)
            .is_some_and(|bucket| bucket.iter().any(|s| s.bytes >= want_bytes))
    }

    /// Take a matching slab from the free list, if one is available.
    fn take_free(&mut self, key: AllocationKey, want_bytes: u64) -> Option<Slab> {
        match self.config.reuse_policy {
            ReusePolicy::ExactSize => {
                let bucket = self.free.get_mut(&key)?;
                let slab = bucket.pop()?;
                if bucket.is_empty() {
                    self.free.remove(&key);
                }
                Some(slab)
            }
            ReusePolicy::SizeClass => {
                // The key's byte count is already class-rounded by `key_for`, so
                // an exact bucket lookup finds the class. The extra check that
                // the slab really is large enough guards against a rounding
                // change making the two disagree.
                let bucket = self.free.get_mut(&key)?;
                let pos = bucket.iter().position(|s| s.bytes >= want_bytes)?;
                let slab = bucket.swap_remove(pos);
                if bucket.is_empty() {
                    self.free.remove(&key);
                }
                Some(slab)
            }
        }
    }

    /// Move retired slabs whose epoch has completed onto the free list.
    fn reclaim_completed(&mut self, watermark: SubmissionEpoch) {
        let mut i = 0;
        while i < self.retired.len() {
            if self.retired[i].safe_after.is_complete_at(watermark) {
                let slab = self.retired.swap_remove(i);
                self.stats.in_flight_bytes -= slab.bytes;
                self.push_free(slab);
            } else {
                i += 1;
            }
        }
    }

    /// Put a slab on the free list, evicting to stay inside the budget.
    ///
    /// Eviction is deterministic: the *incoming* slab is dropped first if it
    /// alone exceeds the budget, otherwise the oldest cached slabs go. Never
    /// touches leased or in-flight memory — those are not the arena's to free.
    fn push_free(&mut self, slab: Slab) {
        let key = AllocationKey {
            bytes: slab.bytes,
            usage: slab.usage,
        };

        let would_bytes = self.stats.cached_bytes + slab.bytes;
        let would_buffers = self.stats.cached_buffers + 1;
        if would_bytes > self.config.max_cached_bytes
            || would_buffers > self.config.max_cached_buffers
        {
            self.evict_until_fits(slab.bytes);
        }

        // Re-check: eviction may not have been able to make room (e.g. the free
        // list was already empty and this one slab exceeds the budget). Dropping
        // the incoming slab is correct — it is unreferenced by construction.
        if self.stats.cached_bytes + slab.bytes > self.config.max_cached_bytes
            || self.stats.cached_buffers + 1 > self.config.max_cached_buffers
        {
            self.stats.evictions += 1;
            drop(slab);
            return;
        }

        self.stats.cached_bytes += slab.bytes;
        self.stats.cached_buffers += 1;
        self.free.entry(key).or_default().push(slab);
    }

    /// Drop cached slabs until `incoming` bytes would fit.
    fn evict_until_fits(&mut self, incoming: u64) {
        while (self.stats.cached_bytes + incoming > self.config.max_cached_bytes
            || self.stats.cached_buffers + 1 > self.config.max_cached_buffers)
            && self.stats.cached_buffers > 0
        {
            // Deterministic victim: the smallest key, first slab. Sorting by key
            // rather than using HashMap order keeps eviction reproducible across
            // runs, which matters for a test asserting eviction counts.
            let victim_key = self
                .free
                .keys()
                .copied()
                .min_by_key(|k| (k.bytes, k.usage.bits()));
            let Some(k) = victim_key else { break };
            let Some(bucket) = self.free.get_mut(&k) else {
                break;
            };
            let Some(slab) = bucket.pop() else {
                self.free.remove(&k);
                continue;
            };
            if bucket.is_empty() {
                self.free.remove(&k);
            }
            self.stats.cached_bytes -= slab.bytes;
            self.stats.cached_buffers -= 1;
            self.stats.evictions += 1;
            drop(slab);
        }
    }

    /// Accept a released slab, routing it by whether an encoder is open.
    fn release(&mut self, slab: Slab, latest_issued: SubmissionEpoch) {
        self.stats.active_bytes -= slab.bytes;
        if self.open_encoders > 0 {
            // May be referenced by commands not yet submitted; the epoch is
            // assigned at the next submit.
            self.stats.in_flight_bytes += slab.bytes;
            let owner = self.current_session;
            self.pending.push((owner, slab));
        } else {
            let mut slab = slab;
            slab.safe_after = latest_issued;
            self.stats.in_flight_bytes += slab.bytes;
            self.retired.push(slab);
        }
    }

    /// Assign `epoch` to everything released while an encoder was open.
    fn seal_pending(&mut self, epoch: SubmissionEpoch) {
        for (_owner, mut slab) in self.pending.drain(..) {
            slab.safe_after = epoch;
            self.retired.push(slab);
        }
    }
}

/// A pool of reusable GPU buffers bound to one [`WgpuBackend`].
///
/// Created per chain or per inference loop, not globally — there is deliberately
/// no singleton, and no arena handle is stored in any tensor type.
pub struct TensorArena<'b> {
    backend: &'b WgpuBackend,
    inner: Rc<RefCell<PoolInner>>,
    tracker: CompletionTracker,
}

impl<'b> TensorArena<'b> {
    #[must_use]
    pub fn new(backend: &'b WgpuBackend, config: TensorArenaConfig) -> Self {
        Self {
            backend,
            inner: Rc::new(RefCell::new(PoolInner {
                config,
                free: HashMap::new(),
                pending: Vec::new(),
                retired: Vec::new(),
                stats: ArenaStats::default(),
                open_encoders: 0,
                current_session: None,
                next_session_id: 0,
            })),
            tracker: CompletionTracker::new(),
        }
    }

    #[must_use]
    pub fn config(&self) -> TensorArenaConfig {
        self.inner.borrow().config
    }

    /// Current counters. Cheap; copies a small struct.
    #[must_use]
    pub fn stats(&self) -> ArenaStats {
        self.inner.borrow().stats
    }

    /// Default storage usage for an intermediate tensor — matches what
    /// `ResourceFactory::zeroed_tensor` creates, so arena and non-arena tensors
    /// are interchangeable at every operation boundary.
    pub const STORAGE_RW: wgpu::BufferUsages = wgpu::BufferUsages::STORAGE
        .union(wgpu::BufferUsages::COPY_DST)
        .union(wgpu::BufferUsages::COPY_SRC);

    fn key_for(&self, bytes: u64, usage: wgpu::BufferUsages) -> AllocationKey {
        let bytes = match self.inner.borrow().config.reuse_policy {
            ReusePolicy::ExactSize => bytes,
            ReusePolicy::SizeClass => bytes.next_power_of_two(),
        };
        AllocationKey { bytes, usage }
    }

    /// Acquire an intermediate tensor of `shape` with the default storage usage.
    ///
    /// # Errors
    ///
    /// [`ComputeError::IndexOverflow`] if the element count does not fit.
    pub fn acquire(&self, shape: TensorShape) -> Result<ArenaTensor<'_>, ComputeError> {
        self.acquire_with_usage(shape, Self::STORAGE_RW)
    }

    /// Acquire with explicit usage flags.
    ///
    /// # Postconditions
    ///
    /// The returned lease owns its allocation exclusively: no other live lease
    /// can hold the same `wgpu::Buffer`, because the pool moved it out.
    ///
    /// The buffer contents are **undefined** — a reused buffer holds whatever
    /// the previous tenant wrote. Operations in this crate write every element
    /// of their output, so this is sound for them; a caller that needs zeros
    /// must write them.
    ///
    /// # Errors
    ///
    /// [`ComputeError::IndexOverflow`] if the element count does not fit `usize`
    /// arithmetic.
    pub fn acquire_with_usage(
        &self,
        shape: TensorShape,
        usage: wgpu::BufferUsages,
    ) -> Result<ArenaTensor<'_>, ComputeError> {
        let elements = shape
            .checked_elements()
            .ok_or(ComputeError::IndexOverflow {
                what: "arena tensor elements",
                value: usize::MAX,
            })?;
        // A zero-sized tensor still needs a valid allocation: wgpu rejects
        // zero-length buffers, and Phase 2/3 kernels bound themselves by the
        // params block rather than arrayLength, so a 1-element pad is invisible.
        let len = elements.max(1);
        let bytes = (len * std::mem::size_of::<f32>()) as u64;
        let key = self.key_for(bytes, usage);

        // A miss is the one moment when polling is unambiguously worth it: the
        // alternative is `create_buffer`, which costs far more than a
        // non-blocking poll. Without this, a buffer released *after* the last
        // poll stays stranded in `retired` and the next acquire allocates a
        // second one — which is exactly what a per-iteration inference loop does
        // (release at end of iteration, acquire at start of the next).
        //
        // Deliberately only on the miss path: polling on every acquire would tax
        // the hit path, which is meant to be nearly free.
        if !self.inner.borrow().has_free(key, bytes) {
            // Ignore a poll failure here: it is not fatal to the acquire, which
            // falls back to allocating. A genuine device fault surfaces at the
            // next submit or readback with a better message than this path could
            // give.
            let _ = self.poll_completion();
        }

        let slab = {
            let mut pool = self.inner.borrow_mut();
            match pool.take_free(key, bytes) {
                Some(slab) => {
                    pool.stats.reuse_hits += 1;
                    pool.stats.cached_bytes -= slab.bytes;
                    pool.stats.cached_buffers -= 1;
                    slab
                }
                None => {
                    pool.stats.reuse_misses += 1;
                    pool.stats.buffers_created += 1;
                    pool.stats.total_allocated_bytes += key.bytes;
                    let buffer = self
                        .backend
                        .device()
                        .create_buffer(&wgpu::BufferDescriptor {
                            label: Some("arena.tensor"),
                            size: key.bytes,
                            usage,
                            mapped_at_creation: false,
                        });
                    Slab {
                        buffer,
                        bytes: key.bytes,
                        usage,
                        safe_after: SubmissionEpoch::NEVER,
                    }
                }
            }
        };

        let mut pool = self.inner.borrow_mut();
        pool.stats.active_bytes += slab.bytes;
        pool.note_peak();
        drop(pool);

        Ok(ArenaTensor {
            tensor: Some(GpuTensor::from_parts(slab.buffer, shape)),
            bytes: slab.bytes,
            usage: slab.usage,
            pool: Rc::clone(&self.inner),
            tracker: self.tracker.clone(),
            _lifetime: std::marker::PhantomData,
        })
    }

    /// Create an encoder whose lifetime the arena tracks.
    ///
    /// **Use this rather than [`WgpuBackend::encoder`] when encoding arena
    /// tensors.** While the returned guard is alive the arena treats every
    /// released lease as possibly-referenced-but-not-yet-submitted, and defers
    /// its epoch to the next [`Self::submit`]. Encoding arena tensors into an
    /// untracked encoder and releasing them before submitting would let the pool
    /// hand the buffer out again while the command buffer still reads it.
    #[must_use]
    pub fn session(&self, label: Option<&str>) -> TrackedSession<'_> {
        let id = {
            let mut pool = self.inner.borrow_mut();
            pool.open_encoders += 1;
            let id = pool.next_session_id;
            pool.next_session_id += 1;
            // Same-session reuse is only sound when exactly one session is open:
            // two open sessions may be submitted in either order, so a slab
            // released under one could be written by the other first.
            pool.current_session = if pool.open_encoders == 1 {
                Some(id)
            } else {
                None
            };
            id
        };
        TrackedSession {
            id,
            arena: self,
            encoder: Some(self.backend.encoder(label)),
            pool: Rc::clone(&self.inner),
            tracker: self.tracker.clone(),
            device: self.backend.device(),
            queue: self.backend.queue(),
            encoded: 0,
        }
    }

    /// Submit a tracked encoder, returning the epoch that identifies it.
    ///
    /// Does **not** wait, and does not read anything back.
    /// Advance the completion watermark **without blocking**, moving any
    /// now-safe retired buffers onto the free list.
    ///
    /// Call between chain iterations. Costs one non-blocking device poll.
    ///
    /// # Errors
    ///
    /// [`ComputeError::Backend`] if the device poll fails.
    pub fn poll_completion(&self) -> Result<SubmissionEpoch, ComputeError> {
        let watermark = self.tracker.poll(self.backend.device())?;
        self.inner.borrow_mut().reclaim_completed(watermark);
        Ok(watermark)
    }

    /// Block until all issued work completes, then reclaim everything retired.
    ///
    /// The synchronising escape hatch — for teardown, or a caller that wants a
    /// clean slate. Not on the release path.
    ///
    /// # Errors
    ///
    /// [`ComputeError::Backend`] if the device poll fails.
    pub fn reclaim_all(&self) -> Result<(), ComputeError> {
        let latest = self.tracker.latest();
        let watermark = self.tracker.wait_for(self.backend.device(), latest)?;
        self.inner.borrow_mut().reclaim_completed(watermark);
        Ok(())
    }

    /// Drop every cached buffer, releasing device memory.
    ///
    /// Only touches the free list: leased and in-flight allocations are not the
    /// arena's to free, and are left alone.
    pub fn trim(&self) {
        let mut pool = self.inner.borrow_mut();
        let freed = pool.stats.cached_buffers;
        pool.free.clear();
        pool.stats.cached_bytes = 0;
        pool.stats.cached_buffers = 0;
        pool.stats.evictions += freed as u64;
    }

    /// The completion tracker, for callers coordinating their own submissions.
    #[must_use]
    pub fn tracker(&self) -> &CompletionTracker {
        &self.tracker
    }

    /// Whether this arena pools for `backend`.
    ///
    /// # Invariant 4, and why it is structural rather than checked
    ///
    /// An allocation from another device can never enter this arena, because
    /// there is no API that inserts one: [`Self::acquire`] is the only source of
    /// slabs and it creates them on `self.backend`. The arena also borrows that
    /// backend for its whole life, so it cannot outlive or be re-pointed at
    /// another device.
    ///
    /// wgpu 30 exposes no public device identity, so this compares backend
    /// object identity -- sufficient, because one `WgpuBackend` owns exactly one
    /// `wgpu::Device`. The remaining cross-device case (an arena tensor passed to
    /// an operation built from a different backend) is caught by wgpu's own bind
    /// group validation, not by this crate.
    #[must_use]
    pub fn owns_device(&self, backend: &WgpuBackend) -> bool {
        std::ptr::eq(self.backend, backend)
    }

    /// # Errors
    ///
    /// [`ComputeError::Backend`] if `backend` is a different device.
    pub fn ensure_same_device(&self, backend: &WgpuBackend) -> Result<(), ComputeError> {
        if self.owns_device(backend) {
            Ok(())
        } else {
            Err(ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: "tensor arena belongs to a different wgpu device".to_string(),
            })
        }
    }
}
/// A tracked execution session: a command encoder whose submission the arena
/// observes, and the **only** place a pooled operand may be encoded.
///
/// # Why this type is mandatory rather than recommended
///
/// Phase 4 had an `ArenaEncoder` that was merely *preferred*: a caller could
/// take the raw `backend.encoder()` and the arena would never learn that its
/// buffers were referenced. That gap is closed in Phase 5 by
/// [`crate::scope`]: an operation built on an [`ArenaTensor`] has scope
/// [`crate::Tracked`], and the only `encode` implementation for that scope takes
/// `&mut TrackedSession`. There is nowhere else for it to go, and the compiler
/// says so.
///
/// # Lifecycle
///
/// ```text
///   arena.session(label)  ──> encode ops ──> submit()  ──> SubmissionEpoch
///                                    └────── drop ─────>  nothing executed
/// ```
///
/// While a session is open, every lease released is held *pending*: it may sit
/// in commands not yet submitted, so it cannot be dated yet. `submit()` stamps
/// the pending set with the real epoch; dropping without submitting frees them
/// immediately, because commands that were never submitted can never run.
///
/// # What it does not do
///
/// No hidden readback, no per-operation submit, no dynamic dispatch. The session
/// holds one encoder and one counter.
pub struct TrackedSession<'a> {
    id: u64,
    arena: &'a TensorArena<'a>,
    encoder: Option<wgpu::CommandEncoder>,
    pool: Rc<RefCell<PoolInner>>,
    tracker: CompletionTracker,
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    encoded: usize,
}

impl<'a> TrackedSession<'a> {
    /// Raw encoder access, for operations whose operands are all caller-owned
    /// (scope [`crate::Untracked`]).
    ///
    /// A pooled operation cannot be encoded through this: its `encode` takes the
    /// session itself, not an encoder.
    pub fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        self.encoder
            .as_mut()
            .expect("TrackedSession used after submit; submit() consumes self")
    }

    /// Record that one pooled operation was encoded. Called by `encode`.
    pub(crate) fn note_encoded(&mut self) -> &mut wgpu::CommandEncoder {
        self.encoded += 1;
        self.encoder
            .as_mut()
            .expect("TrackedSession used after submit; submit() consumes self")
    }

    /// How many pooled operations have been encoded into this session.
    #[must_use]
    pub const fn operations_encoded(&self) -> usize {
        self.encoded
    }

    /// Whether no pooled operation has been encoded yet.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.encoded == 0
    }

    /// Submit the session's commands and date every pending release.
    ///
    /// Consumes the session, so a second submit is not a runtime error but a
    /// compile error — the value has moved.
    ///
    /// Does **not** wait and does **not** read anything back. An empty session
    /// still submits: an empty command buffer is legal, it advances the epoch,
    /// and refusing it would make "submit whatever accumulated" a conditional at
    /// every call site.
    pub fn submit(mut self) -> (wgpu::SubmissionIndex, SubmissionEpoch) {
        let encoder = self
            .encoder
            .take()
            .expect("TrackedSession::submit consumes self, so this cannot run twice");
        let (index, epoch) = self.tracker.submit(self.queue, encoder);
        {
            let mut pool = self.pool.borrow_mut();
            pool.open_encoders = pool.open_encoders.saturating_sub(1);
            if pool.open_encoders == 0 {
                pool.current_session = None;
            }
            pool.seal_pending(epoch);
        }
        // `Drop` still runs, but `encoder` is now None and `open_encoders` is
        // already decremented, so it takes the no-op path.
        (index, epoch)
    }

    /// Submit and block until the work completes.
    ///
    /// A convenience for tests and teardown. The asynchronous path is
    /// [`Self::submit`] followed by [`crate::TensorArena::poll_completion`].
    ///
    /// # Errors
    ///
    /// [`ComputeError::Backend`] if the device poll fails.
    pub fn submit_and_wait(self) -> Result<SubmissionEpoch, ComputeError> {
        let device = self.device;
        let tracker = self.tracker.clone();
        let (_index, epoch) = self.submit();
        tracker.wait_for(device, epoch)?;
        Ok(epoch)
    }
}

impl Drop for TrackedSession<'_> {
    fn drop(&mut self) {
        // `encoder` is Some only when the session is dropped WITHOUT submitting.
        // Those commands never reach the GPU, so anything released while the
        // session was open needs no epoch to complete: freeing it immediately is
        // correct, and stranding it would be a leak.
        if self.encoder.is_some() {
            let mut pool = self.pool.borrow_mut();
            pool.open_encoders = pool.open_encoders.saturating_sub(1);
            if pool.open_encoders == 0 {
                pool.current_session = None;
            }
            let unsubmitted: Vec<(Option<u64>, Slab)> = pool.pending.drain(..).collect();
            for (_owner, mut slab) in unsubmitted {
                slab.safe_after = SubmissionEpoch::NEVER;
                pool.retired.push(slab);
            }
        }
    }
}

/// An exclusive lease on a pooled allocation.
///
/// # Why this does NOT implement `Deref<Target = GpuTensor>`
///
/// It did in Phase 4, and that was the leak. `&*lease` yields a plain
/// `&GpuTensor<f32>`, whose [`crate::TensorOperand::Scope`] is
/// [`crate::Untracked`] — so a single deref would launder away the fact that the
/// buffer came from the pool, and the operation could then be encoded anywhere.
///
/// Operations accept the lease directly (it implements
/// [`crate::TensorOperand`]). Where a raw tensor is genuinely wanted — a
/// readback, a host write — [`Self::tensor`] provides it explicitly, which is
/// greppable rather than invisible.
///
/// # Why there is no `release()` method
///
/// `Drop` is the release, and the lease is neither `Clone` nor `Copy`, so double
/// release is unrepresentable rather than checked.
pub struct ArenaTensor<'a> {
    tensor: Option<GpuTensor<f32>>,
    bytes: u64,
    usage: wgpu::BufferUsages,
    pool: Rc<RefCell<PoolInner>>,
    tracker: CompletionTracker,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

impl ArenaTensor<'_> {
    /// The underlying tensor.
    ///
    /// **`pub(crate)` on purpose.** An independent audit of Phase 5 found that
    /// the public version of this method reopened the very hole the scope system
    /// closed: `&GpuTensor<f32>` has scope [`crate::Untracked`], so
    /// `.output(pooled.tensor())` produced an untracked operation on a pooled
    /// buffer, which a raw encoder then accepted. Removing `Deref` had only
    /// closed the *implicit* path; this was the explicit one.
    ///
    /// Callers who need the data use [`crate::WgpuBackend::read_tensor`] or
    /// [`crate::ResourceFactory::write_tensor`], both of which take any
    /// [`crate::TensorOperand`] and never hand back a raw reference.
    pub(crate) fn gpu(&self) -> &GpuTensor<f32> {
        self.tensor
            .as_ref()
            .expect("ArenaTensor used after release; Drop is the only path that clears it")
    }

    /// Logical shape of the lease.
    ///
    /// Uses the crate-internal accessor, so metadata does not depend on the
    /// public surface handing out a raw tensor.
    #[must_use]
    pub fn shape(&self) -> TensorShape {
        self.gpu().shape()
    }

    /// Size of the underlying allocation, which may exceed the logical tensor
    /// under [`ReusePolicy::SizeClass`].
    #[must_use]
    pub const fn allocation_bytes(&self) -> u64 {
        self.bytes
    }

    #[must_use]
    pub const fn usage(&self) -> wgpu::BufferUsages {
        self.usage
    }
}

impl crate::scope::sealed::TensorAccess for ArenaTensor<'_> {
    fn gpu_tensor(&self) -> &GpuTensor<f32> {
        self.gpu()
    }
}

impl crate::scope::TensorOperand for ArenaTensor<'_> {
    type Scope = crate::scope::Tracked;
}

impl Drop for ArenaTensor<'_> {
    fn drop(&mut self) {
        let Some(tensor) = self.tensor.take() else {
            return;
        };
        let slab = Slab {
            buffer: tensor.into_buffer(),
            bytes: self.bytes,
            usage: self.usage,
            safe_after: SubmissionEpoch::NEVER,
        };
        self.pool.borrow_mut().release(slab, self.tracker.latest());
    }
}

impl<'a> TrackedSession<'a> {
    /// Acquire a tensor that may reuse a buffer released **earlier in this same
    /// session**.
    ///
    /// # Why this is safe, and why the arena-level `acquire` cannot do it
    ///
    /// [`TensorArena::acquire`] refuses to hand out a slab released while a
    /// session is open, because the commands referencing it may not have been
    /// submitted yet. Inside *one* session that caution is provably unnecessary:
    ///
    /// 1. a released lease can no longer be named by any new operation — the
    ///    operations borrow it, so the borrow checker forbids it;
    /// 2. therefore every dispatch touching that buffer was encoded **before**
    ///    the release;
    /// 3. commands in one command buffer execute in order, with barriers between
    ///    compute passes;
    /// 4. so the new tenant's dispatches strictly follow the old tenant's, and
    ///    the reuse is ordered rather than racing.
    ///
    /// Across *different* sessions none of that holds: two sessions can be open
    /// at once and submitted in either order, so a slab released under one could
    /// be written by the other before the first one's reads execute. That is why
    /// eligibility is keyed on the session id, and why opening a second session
    /// disables same-session reuse entirely (the owner becomes ambiguous).
    ///
    /// # Why it matters
    ///
    /// This is what lets a fan-out reach its liveness minimum. In the diamond
    /// topology `shared` dies after both branches read it, and `output` should
    /// take over its allocation — but both events happen inside one session, so
    /// without this method the arena allocates a fourth buffer for a topology
    /// whose true peak is three.
    ///
    /// # Errors
    ///
    /// As [`TensorArena::acquire`].
    pub fn acquire(&mut self, shape: TensorShape) -> Result<ArenaTensor<'a>, ComputeError> {
        self.acquire_with_usage(shape, TensorArena::STORAGE_RW)
    }

    /// [`Self::acquire`] with explicit usage flags.
    ///
    /// # Errors
    ///
    /// As [`TensorArena::acquire_with_usage`].
    pub fn acquire_with_usage(
        &mut self,
        shape: TensorShape,
        usage: wgpu::BufferUsages,
    ) -> Result<ArenaTensor<'a>, ComputeError> {
        let elements = shape
            .checked_elements()
            .ok_or(ComputeError::IndexOverflow {
                what: "arena tensor elements",
                value: usize::MAX,
            })?;
        let bytes = (elements.max(1) * std::mem::size_of::<f32>()) as u64;
        let key = self.arena.key_for(bytes, usage);

        // Try this session's own pending releases first.
        let reclaimed = {
            let mut pool = self.pool.borrow_mut();
            let found = pool.pending.iter().position(|(owner, slab)| {
                *owner == Some(self.id) && slab.usage == usage && slab.bytes >= key.bytes
            });
            found.map(|i| {
                let (_owner, slab) = pool.pending.swap_remove(i);
                pool.stats.in_flight_bytes -= slab.bytes;
                pool.stats.active_bytes += slab.bytes;
                pool.stats.reuse_hits += 1;
                pool.note_peak();
                slab
            })
        };

        if let Some(slab) = reclaimed {
            return Ok(ArenaTensor {
                tensor: Some(GpuTensor::from_parts(slab.buffer, shape)),
                bytes: slab.bytes,
                usage: slab.usage,
                pool: Rc::clone(&self.pool),
                tracker: self.tracker.clone(),
                _lifetime: std::marker::PhantomData,
            });
        }

        self.arena.acquire_with_usage(shape, usage)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Budget arithmetic and eviction are pure enough to test without a device,
    /// and they are where an off-by-one silently leaks memory.
    fn stats_with(cached_bytes: u64, cached_buffers: usize) -> ArenaStats {
        ArenaStats {
            cached_bytes,
            cached_buffers,
            ..ArenaStats::default()
        }
    }

    #[test]
    fn device_bytes_sums_every_state() {
        let s = ArenaStats {
            active_bytes: 100,
            in_flight_bytes: 20,
            cached_bytes: 3,
            ..ArenaStats::default()
        };
        assert_eq!(s.device_bytes(), 123);
    }

    #[test]
    fn hit_rate_is_none_before_any_acquire() {
        assert_eq!(ArenaStats::default().hit_rate(), None);
    }

    #[test]
    fn hit_rate_counts_both_outcomes() {
        let s = ArenaStats {
            reuse_hits: 3,
            reuse_misses: 1,
            ..ArenaStats::default()
        };
        assert_eq!(s.hit_rate(), Some(0.75));
    }

    #[test]
    fn stats_helper_is_consistent() {
        let s = stats_with(64, 1);
        assert_eq!(s.device_bytes(), 64);
        assert_eq!(s.cached_buffers, 1);
    }

    #[test]
    fn default_policy_is_exact_size() {
        assert_eq!(
            TensorArenaConfig::default().reuse_policy,
            ReusePolicy::ExactSize
        );
        assert_eq!(ReusePolicy::default(), ReusePolicy::ExactSize);
    }

    #[test]
    fn default_budget_holds_several_large_intermediates() {
        let c = TensorArenaConfig::default();
        // 200_000 x 128 f32 = 102.4 MB; the budget must hold at least two.
        let intermediate = 200_000u64 * 128 * 4;
        assert!(
            c.max_cached_bytes >= 2 * intermediate,
            "default budget cannot cache the measured working set"
        );
    }

    #[test]
    fn allocation_key_distinguishes_usage() {
        let a = AllocationKey {
            bytes: 256,
            usage: wgpu::BufferUsages::STORAGE,
        };
        let b = AllocationKey {
            bytes: 256,
            usage: wgpu::BufferUsages::UNIFORM,
        };
        assert_ne!(
            a, b,
            "same size but different usage must not share a bucket"
        );
    }
}
