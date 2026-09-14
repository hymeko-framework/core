//! Opt-in throughput and sampled-latency measurement of `hymeko::util::CircularBuffer`.
//!
//! Default-off twice over: the target is registered with `test = false`, and `main`
//! returns before measuring unless Cargo passed `--bench`. Launch only through
//! `cargo bench -p hymeko_core --bench circular_buffer_bench`.
//!
//! Optional environment:
//! - `HYMEKO_BENCH_CB_SCOPE`: `matrix` (default) or `calibration` (one small cell).
//! - `HYMEKO_BENCH_CB_PHASES`: comma-separated subset of `throughput,latency` (default both).
//! - `HYMEKO_BENCH_CB_OUT`: directory receiving the sidecar JSON records.
//!
//! Counting model: one Criterion iteration is one produced item. Rejected pushes, empty
//! pops and values evicted by `force_push` are counted separately and never enter a
//! throughput numerator. Workers are spawned once per cell, outside every timed window,
//! and block on a start barrier between rounds.

use criterion::{BenchmarkId, Criterion, SamplingMode, Throughput};
use hymeko::util::CircularBuffer;
use serde::Serialize;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Barrier, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

const CAPACITIES: [usize; 3] = [1, 64, 1024];
const PAIRS: [usize; 4] = [1, 2, 4, 8];
const WARM_UP: Duration = Duration::from_secs(1);
const MEASUREMENT: Duration = Duration::from_secs(2);
const SAMPLE_SIZE: usize = 10;
const LATENCY_ITEMS: u64 = 1 << 18;
const LATENCY_WARMUPS: usize = 1;
const LATENCY_REPETITIONS: usize = 5;
const SAMPLE_EVERY: u64 = 8;
const PRODUCER_SHIFT: u32 = 40;

// ---------------------------------------------------------------------------
// Bounded FIFO contract, the measured implementations and fault controls
// ---------------------------------------------------------------------------

/// The bounded FIFO contract shared by the measured buffer and its lock-based baseline.
trait BoundedFifo: Sync {
    fn with_capacity(capacity: usize) -> Self;
    /// Inserts unless full; a full buffer returns the value to the caller.
    fn push(&self, value: u64) -> Result<(), u64>;
    /// Always inserts; a full buffer evicts and returns its oldest value.
    fn force_push(&self, value: u64) -> Option<u64>;
    fn pop(&self) -> Option<u64>;
    fn is_empty(&self) -> bool;
}

impl BoundedFifo for CircularBuffer<u64> {
    fn with_capacity(capacity: usize) -> Self {
        CircularBuffer::new(capacity)
    }
    fn push(&self, value: u64) -> Result<(), u64> {
        CircularBuffer::push(self, value)
    }
    fn force_push(&self, value: u64) -> Option<u64> {
        CircularBuffer::force_push(self, value)
    }
    fn pop(&self) -> Option<u64> {
        CircularBuffer::pop(self)
    }
    fn is_empty(&self) -> bool {
        CircularBuffer::is_empty(self)
    }
}

/// Baseline with the same contract: one `Mutex` around a pre-allocated `VecDeque`.
struct MutexRing {
    capacity: usize,
    slots: Mutex<VecDeque<u64>>,
}

impl MutexRing {
    fn slots(&self) -> MutexGuard<'_, VecDeque<u64>> {
        self.slots
            .lock()
            .expect("no worker panics while holding the baseline lock")
    }
}

impl BoundedFifo for MutexRing {
    fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be non-zero");
        let slots = Mutex::new(VecDeque::with_capacity(capacity));
        Self { capacity, slots }
    }
    fn push(&self, value: u64) -> Result<(), u64> {
        let mut slots = self.slots();
        if slots.len() == self.capacity {
            return Err(value);
        }
        slots.push_back(value);
        Ok(())
    }
    fn force_push(&self, value: u64) -> Option<u64> {
        let mut slots = self.slots();
        let evicted = if slots.len() == self.capacity {
            slots.pop_front()
        } else {
            None
        };
        slots.push_back(value);
        evicted
    }
    fn pop(&self) -> Option<u64> {
        self.slots().pop_front()
    }
    fn is_empty(&self) -> bool {
        self.slots().is_empty()
    }
}

/// `push` reports acceptance without inserting.
const DROP_ACCEPTED_PUSH: u8 = 0;
/// `force_push` reports no eviction without inserting.
const DROP_FORCED_PUSH: u8 = 1;
/// `pop` fabricates a copy of payload 0 without removing anything.
const DUPLICATE_POP: u8 = 2;
/// `pop` flips one bit of a removed payload, preserving every count.
const CORRUPT_POP: u8 = 3;

/// Fault-injection control around the Core buffer: exactly one fault per instance, so a
/// sound accounting check must reject the round it occurs in.
struct Faulty<const FAULT: u8> {
    inner: CircularBuffer<u64>,
    fired: AtomicBool,
}

impl<const FAULT: u8> Faulty<FAULT> {
    fn inject(&self, fault: u8) -> bool {
        FAULT == fault && !self.fired.swap(true, Ordering::AcqRel)
    }
}

impl<const FAULT: u8> BoundedFifo for Faulty<FAULT> {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: CircularBuffer::new(capacity),
            fired: AtomicBool::new(false),
        }
    }
    fn push(&self, value: u64) -> Result<(), u64> {
        if self.inject(DROP_ACCEPTED_PUSH) {
            return Ok(());
        }
        self.inner.push(value)
    }
    fn force_push(&self, value: u64) -> Option<u64> {
        if self.inject(DROP_FORCED_PUSH) {
            return None;
        }
        self.inner.force_push(value)
    }
    fn pop(&self) -> Option<u64> {
        if self.inject(DUPLICATE_POP) {
            return Some(0);
        }
        self.inner.pop().map(|value| {
            if self.inject(CORRUPT_POP) {
                value ^ 1
            } else {
                value
            }
        })
    }
    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[derive(Clone, Copy, Debug)]
enum Implementation {
    CoreCircularBuffer,
    MutexVecDeque,
}

impl Implementation {
    const ALL: [Self; 2] = [Self::CoreCircularBuffer, Self::MutexVecDeque];

    fn label(self) -> &'static str {
        match self {
            Self::CoreCircularBuffer => "circular_buffer",
            Self::MutexVecDeque => "mutex_vecdeque",
        }
    }

    fn throughput(self, criterion: &mut Criterion, spec: CellSpec) -> ThroughputCell {
        match self {
            Self::CoreCircularBuffer => measure_throughput::<CircularBuffer<u64>>(criterion, spec),
            Self::MutexVecDeque => measure_throughput::<MutexRing>(criterion, spec),
        }
    }

    fn latency(self, spec: CellSpec) -> LatencyCell {
        match self {
            Self::CoreCircularBuffer => measure_latency::<CircularBuffer<u64>>(spec),
            Self::MutexVecDeque => measure_latency::<MutexRing>(spec),
        }
    }
}

// ---------------------------------------------------------------------------
// Preflight, before any timing: comparison validity, bucket arithmetic, oracle power
// ---------------------------------------------------------------------------

/// Both implementations must return identical outcomes on one deterministic mixed
/// push / force_push / pop sequence, then drain to identical contents.
fn assert_semantic_parity() {
    fn apply<F: BoundedFifo>(fifo: &F, step: u64) -> (Option<u64>, bool) {
        let returned = match splitmix64(step) % 4 {
            0 | 1 => fifo.push(step).err(),
            2 => fifo.force_push(step),
            _ => fifo.pop(),
        };
        (returned, fifo.is_empty())
    }
    for capacity in [1, 2, 3, 64] {
        let core = <CircularBuffer<u64> as BoundedFifo>::with_capacity(capacity);
        let baseline = MutexRing::with_capacity(capacity);
        for step in 0..4096 {
            assert_eq!(
                apply(&core, step),
                apply(&baseline, step),
                "parity: capacity {capacity}, step {step}"
            );
        }
        let core_rest: Vec<u64> = std::iter::from_fn(|| BoundedFifo::pop(&core)).collect();
        let baseline_rest: Vec<u64> = std::iter::from_fn(|| baseline.pop()).collect();
        assert_eq!(
            core_rest, baseline_rest,
            "parity: drained contents, capacity {capacity}"
        );
    }
}

/// Every probed value must fall inside the bounds reported for its bucket.
fn assert_histogram_buckets() {
    let edges = (0..64).flat_map(|bit| {
        let base = 1_u64 << bit;
        [base - 1, base, base | (base >> 1)]
    });
    for ticks in (0..1 << 16).chain(edges).chain([u64::MAX]) {
        let index = Histogram::index(ticks);
        let (lower, upper) = Histogram::bounds(index);
        assert!(
            index < Histogram::BUCKETS,
            "bucket index overflow for {ticks}"
        );
        assert!(
            lower <= ticks && (ticks < upper || upper == u64::MAX),
            "bucket bounds exclude {ticks}"
        );
    }
}

/// The per-round check must accept a fault-free round and reject a lost insertion, a
/// duplicated removal and a count-preserving payload corruption, through the real pool.
fn assert_accounting_rejects_faults() {
    fn violated<F: BoundedFifo>(mode: Mode) -> bool {
        let implementation = Implementation::CoreCircularBuffer;
        let spec = CellSpec {
            implementation,
            mode,
            capacity: 1,
            pairs: 2,
        };
        with_pool::<F, Unprobed, _>(spec, |pool| pool.run_round(256).verify(mode).is_err())
    }
    for mode in Mode::ALL {
        assert!(
            !violated::<CircularBuffer<u64>>(mode),
            "control: clean {mode:?} round rejected"
        );
        assert!(
            violated::<Faulty<DUPLICATE_POP>>(mode),
            "control: duplicate pop undetected, {mode:?}"
        );
        assert!(
            violated::<Faulty<CORRUPT_POP>>(mode),
            "control: corrupt pop undetected, {mode:?}"
        );
    }
    assert!(
        violated::<Faulty<DROP_ACCEPTED_PUSH>>(Mode::Rejecting),
        "control: dropped push undetected"
    );
    assert!(
        violated::<Faulty<DROP_FORCED_PUSH>>(Mode::Overwrite),
        "control: dropped force_push undetected"
    );
}
// ---------------------------------------------------------------------------
// Accounting
// ---------------------------------------------------------------------------

fn splitmix64(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Conservation fingerprint of a payload multiset: count, wrapping sum and mixed sum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
struct Fingerprint {
    count: u64,
    sum: u64,
    mixed: u64,
}

impl Fingerprint {
    fn add(&mut self, value: u64) {
        self.count += 1;
        self.sum = self.sum.wrapping_add(value);
        self.mixed = self.mixed.wrapping_add(splitmix64(value));
    }

    fn merge(self, other: Self) -> Self {
        Self {
            count: self.count + other.count,
            sum: self.sum.wrapping_add(other.sum),
            mixed: self.mixed.wrapping_add(other.mixed),
        }
    }
}

/// Attempt and outcome counts of one worker (or, merged, of one round).
#[derive(Clone, Copy, Debug, Default, Serialize)]
struct Counters {
    /// Values placed into the buffer by an accepted `push` or by `force_push`.
    inserted: Fingerprint,
    /// Values taken out by a successful `pop`.
    removed: Fingerprint,
    /// Values displaced by `force_push` and returned to the producer.
    evicted: Fingerprint,
    /// `push` calls that returned their value because the buffer was full.
    rejected_pushes: u64,
    /// `pop` calls that found the buffer empty, including each consumer's terminal probe.
    empty_pops: u64,
}

impl Counters {
    fn merge(self, other: Self) -> Self {
        Self {
            inserted: self.inserted.merge(other.inserted),
            removed: self.removed.merge(other.removed),
            evicted: self.evicted.merge(other.evicted),
            rejected_pushes: self.rejected_pushes + other.rejected_pushes,
            empty_pops: self.empty_pops + other.empty_pops,
        }
    }
}

/// Identical retry policy for every implementation: spin 2^k times for k < 6, then yield.
#[derive(Default)]
struct Backoff {
    step: u32,
}

impl Backoff {
    const SPIN_STEPS: u32 = 6;

    fn snooze(&mut self) {
        if self.step < Self::SPIN_STEPS {
            (0..1_u32 << self.step).for_each(|_| std::hint::spin_loop());
            self.step += 1;
        } else {
            thread::yield_now();
        }
    }

    fn reset(&mut self) {
        self.step = 0;
    }
}

// ---------------------------------------------------------------------------
// Latency instrumentation, compiled out of throughput cells
// ---------------------------------------------------------------------------

/// One buffer call classified by its outcome: the unit of a latency observation.
#[derive(Clone, Copy, Debug)]
enum OpKind {
    PushAccepted,
    PushRejected,
    ForcePushInserted,
    ForcePushEvicted,
    PopRemoved,
    PopEmpty,
}

impl OpKind {
    const COUNT: usize = 6;
    const ALL: [Self; Self::COUNT] = [
        Self::PushAccepted,
        Self::PushRejected,
        Self::ForcePushInserted,
        Self::ForcePushEvicted,
        Self::PopRemoved,
        Self::PopEmpty,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::PushAccepted => "push_accepted",
            Self::PushRejected => "push_rejected",
            Self::ForcePushInserted => "force_push_inserted",
            Self::ForcePushEvicted => "force_push_evicted",
            Self::PopRemoved => "pop_removed",
            Self::PopEmpty => "pop_empty",
        }
    }
}

/// Log-linear histogram of tick counts: exact below 16, then 16 sub-buckets per octave.
struct Histogram {
    counts: Box<[u64]>,
    max: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            counts: vec![0; Self::BUCKETS].into_boxed_slice(),
            max: 0,
        }
    }
}

impl Histogram {
    const SUB_BITS: u32 = 4;
    const SUB: usize = 1 << Self::SUB_BITS;
    const BUCKETS: usize = Self::SUB * (65 - Self::SUB_BITS as usize);

    fn index(ticks: u64) -> usize {
        if ticks < Self::SUB as u64 {
            return ticks as usize;
        }
        let msb = 63 - ticks.leading_zeros();
        let sub = (ticks >> (msb - Self::SUB_BITS)) as usize & (Self::SUB - 1);
        Self::SUB + (msb - Self::SUB_BITS) as usize * Self::SUB + sub
    }

    /// Half-open tick interval of a bucket; the top bucket's upper bound saturates.
    fn bounds(index: usize) -> (u64, u64) {
        if index < Self::SUB {
            return (index as u64, index as u64 + 1);
        }
        let octave = ((index - Self::SUB) / Self::SUB) as u32;
        let sub = ((index - Self::SUB) % Self::SUB) as u64;
        let width = 1_u64 << octave;
        let lower = (1_u64 << (octave + Self::SUB_BITS)) + sub * width;
        (lower, lower.saturating_add(width))
    }

    fn record(&mut self, ticks: u64) {
        self.counts[Self::index(ticks)] += 1;
        self.max = self.max.max(ticks);
    }

    fn absorb(&mut self, other: &Self) {
        self.counts
            .iter_mut()
            .zip(other.counts.iter())
            .for_each(|(sum, count)| *sum += count);
        self.max = self.max.max(other.max);
    }

    fn to_record(&self, operation: &'static str) -> OperationLatency {
        let buckets: Vec<[u64; 3]> = self
            .counts
            .iter()
            .enumerate()
            .filter(|&(_, &count)| count > 0)
            .map(|(index, &count)| {
                let (lower, upper) = Self::bounds(index);
                [lower, upper, count]
            })
            .collect();
        OperationLatency {
            operation,
            samples: buckets.iter().map(|bucket| bucket[2]).sum(),
            max_ticks: self.max,
            buckets,
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn tsc_ticks() -> u64 {
    // SAFETY: RDTSC belongs to the x86_64 baseline instruction set; reading the
    // counter has no memory-safety precondition.
    unsafe { std::arch::x86_64::_rdtsc() }
}

#[cfg(not(target_arch = "x86_64"))]
fn tsc_ticks() -> u64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// Instrumentation seam; throughput cells use [`Unprobed`], so no timing code remains there.
trait Probe: Default + Send {
    /// Runs one buffer call and returns its duration in ticks when this call is sampled.
    fn call<R>(&mut self, call: impl FnOnce() -> R) -> (R, Option<u64>);
    fn record(&mut self, kind: OpKind, ticks: Option<u64>);
}

#[derive(Default)]
struct Unprobed;

impl Probe for Unprobed {
    #[inline(always)]
    fn call<R>(&mut self, call: impl FnOnce() -> R) -> (R, Option<u64>) {
        (call(), None)
    }
    #[inline(always)]
    fn record(&mut self, _: OpKind, _: Option<u64>) {}
}

/// Times every `SAMPLE_EVERY`-th buffer call of its thread with the TSC.
struct Sampled {
    calls: u64,
    histograms: [Histogram; OpKind::COUNT],
}

impl Default for Sampled {
    fn default() -> Self {
        Self {
            calls: 0,
            histograms: std::array::from_fn(|_| Histogram::default()),
        }
    }
}

impl Probe for Sampled {
    fn call<R>(&mut self, call: impl FnOnce() -> R) -> (R, Option<u64>) {
        self.calls += 1;
        if !self.calls.is_multiple_of(SAMPLE_EVERY) {
            return (call(), None);
        }
        let begin = tsc_ticks();
        let result = call();
        let end = tsc_ticks();
        (result, Some(end.saturating_sub(begin)))
    }

    fn record(&mut self, kind: OpKind, ticks: Option<u64>) {
        if let Some(ticks) = ticks {
            self.histograms[kind as usize].record(ticks);
        }
    }
}

/// Nanoseconds per tick, estimated against `Instant` over sleeping windows.
#[derive(Serialize)]
struct TickCalibration {
    ns_per_tick: f64,
    window_ms: u64,
    window_estimates: Vec<f64>,
}

impl TickCalibration {
    const WINDOWS: usize = 5;
    const WINDOW: Duration = Duration::from_millis(100);

    fn measure() -> Self {
        let window_estimates: Vec<f64> = (0..Self::WINDOWS)
            .map(|_| {
                let (instant, begin) = (Instant::now(), tsc_ticks());
                thread::sleep(Self::WINDOW);
                let (end, elapsed) = (tsc_ticks(), instant.elapsed());
                elapsed.as_nanos() as f64 / (end - begin) as f64
            })
            .collect();
        let mut sorted = window_estimates.clone();
        sorted.sort_by(f64::total_cmp);
        Self {
            ns_per_tick: sorted[Self::WINDOWS / 2],
            window_ms: Self::WINDOW.as_millis() as u64,
            window_estimates,
        }
    }
}

/// Probe cost around an empty call: the floor under every sampled latency.
fn instrumentation_floor() -> OperationLatency {
    let mut probe = Sampled::default();
    let mut floor = Histogram::default();
    for _ in 0..SAMPLE_EVERY << 20 {
        if let ((), Some(ticks)) = probe.call(|| std::hint::black_box(())) {
            floor.record(ticks);
        }
    }
    floor.to_record("empty_call")
}
// ---------------------------------------------------------------------------
// Workload: cell shape and worker loops
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    /// `push`, retried with [`Backoff`] until accepted.
    Rejecting,
    /// `force_push`; every evicted value is returned and counted.
    Overwrite,
}

impl Mode {
    const ALL: [Self; 2] = [Self::Rejecting, Self::Overwrite];

    fn label(self) -> &'static str {
        match self {
            Self::Rejecting => "rejecting",
            Self::Overwrite => "overwrite",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct CellSpec {
    implementation: Implementation,
    mode: Mode,
    capacity: usize,
    /// Producers and, equally, consumers.
    pairs: usize,
}

impl CellSpec {
    fn series(&self) -> String {
        let (implementation, mode) = (self.implementation.label(), self.mode.label());
        format!("{implementation}_{mode}_cap{}", self.capacity)
    }

    fn threads(&self) -> String {
        format!("{0}p{0}c", self.pairs)
    }

    fn quota(&self, items: u64, producer: usize) -> u64 {
        let pairs = self.pairs as u64;
        items / pairs + u64::from((producer as u64) < items % pairs)
    }

    fn header(&self) -> CellHeader {
        CellHeader {
            implementation: self.implementation.label(),
            mode: self.mode,
            capacity: self.capacity,
            producers: self.pairs,
            consumers: self.pairs,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Role {
    Producer,
    Consumer,
}

fn produce<F: BoundedFifo, P: Probe>(
    fifo: &F,
    mode: Mode,
    producer: usize,
    quota: u64,
    probe: &mut P,
) -> Counters {
    let mut counters = Counters::default();
    let mut backoff = Backoff::default();
    for sequence in 0..quota {
        let value = ((producer as u64) << PRODUCER_SHIFT) | sequence;
        match mode {
            Mode::Rejecting => push_until_accepted(fifo, value, probe, &mut backoff, &mut counters),
            Mode::Overwrite => force_insert(fifo, value, probe, &mut counters),
        }
    }
    counters
}

fn push_until_accepted<F: BoundedFifo, P: Probe>(
    fifo: &F,
    value: u64,
    probe: &mut P,
    backoff: &mut Backoff,
    counters: &mut Counters,
) {
    let mut pending = value;
    loop {
        let (result, ticks) = probe.call(|| fifo.push(pending));
        match result {
            Ok(()) => {
                probe.record(OpKind::PushAccepted, ticks);
                counters.inserted.add(pending);
                backoff.reset();
                return;
            }
            Err(returned) => {
                probe.record(OpKind::PushRejected, ticks);
                counters.rejected_pushes += 1;
                pending = returned;
                backoff.snooze();
            }
        }
    }
}

fn force_insert<F: BoundedFifo, P: Probe>(
    fifo: &F,
    value: u64,
    probe: &mut P,
    counters: &mut Counters,
) {
    let (evicted, ticks) = probe.call(|| fifo.force_push(value));
    counters.inserted.add(value);
    match evicted {
        Some(old) => {
            probe.record(OpKind::ForcePushEvicted, ticks);
            counters.evicted.add(old);
        }
        None => probe.record(OpKind::ForcePushInserted, ticks),
    }
}

fn consume<F: BoundedFifo, P: Probe>(
    fifo: &F,
    producers_done: &AtomicBool,
    probe: &mut P,
) -> Counters {
    let mut counters = Counters::default();
    let mut backoff = Backoff::default();
    loop {
        // Load before popping: an empty pop that began after every producer finished
        // proves no value can arrive any more.
        let producers_finished = producers_done.load(Ordering::Acquire);
        let (removed, ticks) = probe.call(|| fifo.pop());
        let Some(value) = removed else {
            probe.record(OpKind::PopEmpty, ticks);
            counters.empty_pops += 1;
            if producers_finished {
                return counters;
            }
            backoff.snooze();
            continue;
        };
        probe.record(OpKind::PopRemoved, ticks);
        counters.removed.add(value);
        backoff.reset();
    }
}

// ---------------------------------------------------------------------------
// Persistent pool and rounds
// ---------------------------------------------------------------------------

struct WorkerReport<P> {
    counters: Counters,
    probe: P,
    /// When this worker returned from the start barrier.
    started: Instant,
}

/// Coordination state shared by the coordinator and one cell's persistent workers.
struct RoundControl<P> {
    start: Barrier,
    producers_finished: Barrier,
    consumers_finished: Barrier,
    items: AtomicU64,
    producers_done: AtomicBool,
    shutdown: AtomicBool,
    /// Producers occupy slots `0..pairs`, consumers `pairs..2 * pairs`.
    reports: Vec<Mutex<Option<WorkerReport<P>>>>,
}

impl<P> RoundControl<P> {
    fn new(pairs: usize) -> Self {
        Self {
            start: Barrier::new(2 * pairs + 1),
            producers_finished: Barrier::new(pairs + 1),
            consumers_finished: Barrier::new(pairs + 1),
            items: AtomicU64::new(0),
            producers_done: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            reports: (0..2 * pairs).map(|_| Mutex::new(None)).collect(),
        }
    }

    fn slot(&self, slot: usize) -> MutexGuard<'_, Option<WorkerReport<P>>> {
        self.reports[slot]
            .lock()
            .expect("report slots are never poisoned")
    }
}

fn run_worker<F: BoundedFifo, P: Probe>(
    fifo: &F,
    control: &RoundControl<P>,
    spec: CellSpec,
    role: Role,
    index: usize,
) {
    loop {
        let mut probe = P::default();
        control.start.wait();
        let started = Instant::now();
        if control.shutdown.load(Ordering::Acquire) {
            return;
        }
        let items = control.items.load(Ordering::Acquire);
        let (counters, slot, finished) = match role {
            Role::Producer => (
                produce(fifo, spec.mode, index, spec.quota(items, index), &mut probe),
                index,
                &control.producers_finished,
            ),
            Role::Consumer => (
                consume(fifo, &control.producers_done, &mut probe),
                spec.pairs + index,
                &control.consumers_finished,
            ),
        };
        // Report before the finish barrier, so the coordinator never reads an empty slot.
        *control.slot(slot) = Some(WorkerReport {
            counters,
            probe,
            started,
        });
        finished.wait();
    }
}

/// Stops a cell's workers when the cell ends, including a panic between rounds.
struct ReleaseOnDrop<'a, P>(&'a RoundControl<P>);

impl<P> Drop for ReleaseOnDrop<'_, P> {
    fn drop(&mut self) {
        self.0.shutdown.store(true, Ordering::Release);
        self.0.start.wait();
    }
}

/// One cell's buffer and persistent workers; every round reuses the same threads.
struct Pool<'a, F, P> {
    fifo: &'a F,
    control: &'a RoundControl<P>,
    spec: CellSpec,
}

struct Round<P> {
    items: u64,
    elapsed: Duration,
    /// Until the last producer finished; `elapsed` minus this is the drain tail.
    producers_elapsed: Duration,
    /// Largest delay from the coordinator's clock start to a worker leaving the barrier.
    max_start_lag: Duration,
    counters: Counters,
    drained: bool,
    probes: Vec<P>,
}

impl<F: BoundedFifo, P: Probe> Pool<'_, F, P> {
    /// Timed window: start-barrier release to the last consumer's completion.
    fn run_round(&self, items: u64) -> Round<P> {
        let control = self.control;
        control.items.store(items, Ordering::Release);
        control.producers_done.store(false, Ordering::Release);
        control.start.wait();
        let began = Instant::now();
        control.producers_finished.wait();
        let producers_elapsed = began.elapsed();
        control.producers_done.store(true, Ordering::Release);
        control.consumers_finished.wait();
        let elapsed = began.elapsed();
        let reports: Vec<WorkerReport<P>> = (0..2 * self.spec.pairs)
            .map(|slot| {
                control
                    .slot(slot)
                    .take()
                    .expect("every worker reports before its finish barrier")
            })
            .collect();
        Round {
            items,
            elapsed,
            producers_elapsed,
            max_start_lag: reports
                .iter()
                .map(|report| report.started.saturating_duration_since(began))
                .max()
                .unwrap_or_default(),
            counters: reports.iter().fold(Counters::default(), |sum, report| {
                sum.merge(report.counters)
            }),
            drained: self.fifo.is_empty(),
            probes: reports.into_iter().map(|report| report.probe).collect(),
        }
    }
}

fn with_pool<F: BoundedFifo, P: Probe, R>(
    spec: CellSpec,
    body: impl FnOnce(&Pool<'_, F, P>) -> R,
) -> R {
    let fifo = F::with_capacity(spec.capacity);
    let control = RoundControl::<P>::new(spec.pairs);
    thread::scope(|scope| {
        for role in [Role::Producer, Role::Consumer] {
            for index in 0..spec.pairs {
                let (fifo, control) = (&fifo, &control);
                scope.spawn(move || run_worker(fifo, control, spec, role, index));
            }
        }
        let _release = ReleaseOnDrop(&control);
        body(&Pool {
            fifo: &fifo,
            control: &control,
            spec,
        })
    })
}

impl<P> Round<P> {
    /// Conservation and mode invariants; checked on every round, warm-up included.
    fn verify(&self, mode: Mode) -> Result<(), String> {
        let counters = &self.counters;
        let conserved = counters.removed.merge(counters.evicted) == counters.inserted;
        let mode_consistent = match mode {
            Mode::Rejecting => counters.evicted.count == 0,
            Mode::Overwrite => counters.rejected_pushes == 0,
        };
        if counters.inserted.count == self.items && conserved && mode_consistent && self.drained {
            Ok(())
        } else {
            Err(format!(
                "accounting violated: items {} drained {} {counters:?}",
                self.items, self.drained
            ))
        }
    }

    fn record(&self) -> RoundRecord {
        RoundRecord {
            items: self.items,
            elapsed_ns: self.elapsed.as_nanos() as u64,
            producers_elapsed_ns: self.producers_elapsed.as_nanos() as u64,
            max_start_lag_ns: self.max_start_lag.as_nanos() as u64,
            counters: self.counters,
        }
    }
}
// ---------------------------------------------------------------------------
// Measured cells and their sidecar records
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct CellHeader {
    implementation: &'static str,
    mode: Mode,
    capacity: usize,
    producers: usize,
    consumers: usize,
}

#[derive(Serialize)]
struct RoundRecord {
    items: u64,
    elapsed_ns: u64,
    producers_elapsed_ns: u64,
    max_start_lag_ns: u64,
    counters: Counters,
}

#[derive(Serialize)]
struct ThroughputCell {
    #[serde(flatten)]
    header: CellHeader,
    criterion_id: String,
    /// Every `iter_custom` call in order: Criterion warm-up rounds, then the samples.
    rounds: Vec<RoundRecord>,
}

impl ThroughputCell {
    fn summary(&self) -> String {
        let tail = &self.rounds[self.rounds.len().saturating_sub(SAMPLE_SIZE)..];
        let mut removed: Vec<f64> = tail
            .iter()
            .map(|round| round.counters.removed.count as f64 * 1e9 / round.elapsed_ns.max(1) as f64)
            .collect();
        removed.sort_by(f64::total_cmp);
        let median = removed.get(removed.len() / 2).copied().unwrap_or(f64::NAN);
        let totals = tail
            .iter()
            .fold(Counters::default(), |sum, round| sum.merge(round.counters));
        let drain_tail = tail
            .iter()
            .map(|round| 1.0 - round.producers_elapsed_ns as f64 / round.elapsed_ns.max(1) as f64)
            .fold(0.0, f64::max);
        let start_lag = tail
            .iter()
            .map(|round| round.max_start_lag_ns)
            .max()
            .unwrap_or(0);
        format!(
            "rounds {} | sample median removed {median:.3e}/s | sampled inserted {} removed {} rejected {} evicted {} empty_pops {} | max drain tail {:.3}% | max start lag {start_lag} ns",
            self.rounds.len(),
            totals.inserted.count,
            totals.removed.count,
            totals.rejected_pushes,
            totals.evicted.count,
            totals.empty_pops,
            drain_tail * 100.0
        )
    }
}

#[derive(Serialize)]
struct OperationLatency {
    operation: &'static str,
    samples: u64,
    max_ticks: u64,
    /// Non-empty buckets as `[lower_ticks, upper_ticks_exclusive, count]`.
    buckets: Vec<[u64; 3]>,
}

#[derive(Serialize)]
struct LatencyRepetition {
    warmup: bool,
    round: RoundRecord,
    operations: Vec<OperationLatency>,
}

#[derive(Serialize)]
struct LatencyCell {
    #[serde(flatten)]
    header: CellHeader,
    repetitions: Vec<LatencyRepetition>,
}

impl LatencyCell {
    fn summary(&self, ns_per_tick: f64) -> String {
        let measured = self
            .repetitions
            .iter()
            .filter(|repetition| !repetition.warmup);
        let (samples, max_ticks) = measured
            .flat_map(|repetition| repetition.operations.iter())
            .fold((0, 0), |(samples, max), op| {
                (samples + op.samples, max.max(op.max_ticks))
            });
        format!(
            "repetitions {} | sampled calls {samples} | max {:.0} ns",
            self.repetitions.len(),
            max_ticks as f64 * ns_per_tick
        )
    }
}

fn measure_throughput<F: BoundedFifo>(criterion: &mut Criterion, spec: CellSpec) -> ThroughputCell {
    let group_name = format!("circular_buffer_throughput_{}", spec.series());
    let mut group = criterion.benchmark_group(group_name.as_str());
    group
        .sampling_mode(SamplingMode::Flat)
        .warm_up_time(WARM_UP)
        .measurement_time(MEASUREMENT)
        .sample_size(SAMPLE_SIZE);
    if spec.mode == Mode::Rejecting {
        // One rejecting iteration transfers one item. An overwrite iteration is one offered
        // item, which is not a transfer, so Criterion gets no throughput unit there.
        group.throughput(Throughput::Elements(1));
    }
    let mut rounds = Vec::new();
    let mut violation = None;
    with_pool::<F, Unprobed, _>(spec, |pool| {
        group.bench_function(BenchmarkId::from_parameter(spec.threads()), |bencher| {
            bencher.iter_custom(|items| {
                let round = pool.run_round(items);
                if let Err(error) = round.verify(spec.mode) {
                    violation.get_or_insert(error);
                }
                rounds.push(round.record());
                round.elapsed
            });
        });
    });
    group.finish();
    if let Some(error) = violation {
        panic!("{}/{}: {error}", spec.series(), spec.threads());
    }
    ThroughputCell {
        header: spec.header(),
        criterion_id: format!("{group_name}/{}", spec.threads()),
        rounds,
    }
}

fn measure_latency<F: BoundedFifo>(spec: CellSpec) -> LatencyCell {
    let repetitions = with_pool::<F, Sampled, _>(spec, |pool| {
        (0..LATENCY_WARMUPS + LATENCY_REPETITIONS)
            .map(|repetition| {
                let round = pool.run_round(LATENCY_ITEMS);
                if let Err(error) = round.verify(spec.mode) {
                    panic!("{}/{}: {error}", spec.series(), spec.threads());
                }
                LatencyRepetition {
                    warmup: repetition < LATENCY_WARMUPS,
                    operations: merged_operations(&round.probes),
                    round: round.record(),
                }
            })
            .collect()
    });
    LatencyCell {
        header: spec.header(),
        repetitions,
    }
}

fn merged_operations(probes: &[Sampled]) -> Vec<OperationLatency> {
    OpKind::ALL
        .iter()
        .map(|&kind| {
            let mut merged = Histogram::default();
            probes
                .iter()
                .for_each(|probe| merged.absorb(&probe.histograms[kind as usize]));
            merged.to_record(kind.label())
        })
        .filter(|operation| operation.samples > 0)
        .collect()
}

// ---------------------------------------------------------------------------
// Run configuration, progress and entry point
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct MeasurementConfig {
    warm_up_ms: u64,
    measurement_ms: u64,
    sample_size: usize,
    sampling_mode: &'static str,
    latency_items: u64,
    latency_warmups: usize,
    latency_repetitions: usize,
    latency_sample_every: u64,
    backoff: &'static str,
    payload: &'static str,
    available_parallelism: usize,
    skipped_pairs: Vec<usize>,
}

#[derive(Serialize)]
struct RunRecord<C> {
    schema: u32,
    scope: &'static str,
    phase: &'static str,
    config: MeasurementConfig,
    tick_calibration: Option<TickCalibration>,
    instrumentation_floor: Option<OperationLatency>,
    cells: Vec<C>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Calibration,
    Matrix,
}

struct Settings {
    scope: Scope,
    throughput: bool,
    latency: bool,
    out: PathBuf,
    parallelism: usize,
}

impl Settings {
    fn from_env() -> Self {
        let scope = match std::env::var("HYMEKO_BENCH_CB_SCOPE").as_deref() {
            Err(_) | Ok("matrix") => Scope::Matrix,
            Ok("calibration") => Scope::Calibration,
            Ok(other) => panic!("HYMEKO_BENCH_CB_SCOPE must be matrix or calibration, got {other}"),
        };
        let phases = std::env::var("HYMEKO_BENCH_CB_PHASES")
            .unwrap_or_else(|_| "throughput,latency".to_owned());
        let phases: Vec<&str> = phases.split(',').map(str::trim).collect();
        assert!(
            phases
                .iter()
                .all(|phase| matches!(*phase, "throughput" | "latency")),
            "HYMEKO_BENCH_CB_PHASES accepts only throughput and latency"
        );
        let out = std::env::var_os("HYMEKO_BENCH_CB_OUT").map_or_else(
            || {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("target")
                    .join("circular_buffer_bench")
            },
            PathBuf::from,
        );
        Self {
            scope,
            throughput: phases.contains(&"throughput"),
            latency: phases.contains(&"latency"),
            out,
            parallelism: thread::available_parallelism().map_or(1, std::num::NonZero::get),
        }
    }

    /// Pair counts runnable without oversubscription (2 workers per pair plus the coordinator).
    fn pairs(&self) -> (Vec<usize>, Vec<usize>) {
        PAIRS
            .into_iter()
            .partition(|&pairs| 2 * pairs < self.parallelism)
    }

    /// Implementations are adjacent for every shape, so slow drift cannot separate them.
    fn cells(&self) -> Vec<CellSpec> {
        let (pairs, _) = self.pairs();
        if self.scope == Scope::Calibration {
            let implementation = Implementation::CoreCircularBuffer;
            return vec![CellSpec {
                implementation,
                mode: Mode::Rejecting,
                capacity: 64,
                pairs: 1,
            }];
        }
        let mut cells = Vec::new();
        for mode in Mode::ALL {
            for capacity in CAPACITIES {
                for &pairs in &pairs {
                    cells.extend(Implementation::ALL.map(|implementation| CellSpec {
                        implementation,
                        mode,
                        capacity,
                        pairs,
                    }));
                }
            }
        }
        cells
    }

    fn record<C>(&self, phase: &'static str) -> RunRecord<C> {
        RunRecord {
            schema: 1,
            scope: match self.scope {
                Scope::Calibration => "calibration",
                Scope::Matrix => "matrix",
            },
            phase,
            config: MeasurementConfig {
                warm_up_ms: WARM_UP.as_millis() as u64,
                measurement_ms: MEASUREMENT.as_millis() as u64,
                sample_size: SAMPLE_SIZE,
                sampling_mode: "flat",
                latency_items: LATENCY_ITEMS,
                latency_warmups: LATENCY_WARMUPS,
                latency_repetitions: LATENCY_REPETITIONS,
                latency_sample_every: SAMPLE_EVERY,
                backoff: "spin 2^k for k < 6, then thread::yield_now; reset on success",
                payload: "u64 = producer << 40 | sequence",
                available_parallelism: self.parallelism,
                skipped_pairs: self.pairs().1,
            },
            tick_calibration: None,
            instrumentation_floor: None,
            cells: Vec::new(),
        }
    }

    fn write<C: Serialize>(&self, record: &RunRecord<C>) {
        std::fs::create_dir_all(&self.out).expect("benchmark output directory is creatable");
        let path = self
            .out
            .join(format!("{}-{}.json", record.scope, record.phase));
        let bytes = serde_json::to_vec(record).expect("records serialize");
        std::fs::write(&path, bytes)
            .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
    }
}

/// Flushed progress lines carrying counts, elapsed time and a linear ETA.
struct Progress {
    phase: &'static str,
    total: usize,
    done: usize,
    started: Instant,
}

impl Progress {
    fn new(phase: &'static str, total: usize) -> Self {
        Self {
            phase,
            total,
            done: 0,
            started: Instant::now(),
        }
    }

    fn begin(&self, spec: &CellSpec) {
        let (phase, position, total) = (self.phase, self.done + 1, self.total);
        eprintln!(
            "[cb-bench] {phase} {position}/{total} start {}/{}",
            spec.series(),
            spec.threads()
        );
    }

    fn finish(&mut self, spec: &CellSpec, detail: &str) {
        self.done += 1;
        let elapsed = self.started.elapsed().as_secs_f64();
        let eta = elapsed / self.done as f64 * (self.total - self.done) as f64;
        let (phase, done, total) = (self.phase, self.done, self.total);
        eprintln!(
            "[cb-bench] {phase} {done}/{total} done {}/{} | {detail} | elapsed {elapsed:.1}s | eta {eta:.0}s",
            spec.series(),
            spec.threads()
        );
    }
}

fn run_throughput(settings: &Settings, cells: &[CellSpec]) {
    let mut criterion = Criterion::default().configure_from_args();
    let mut record = settings.record("throughput");
    let mut progress = Progress::new("throughput", cells.len());
    for &spec in cells {
        progress.begin(&spec);
        let cell = spec.implementation.throughput(&mut criterion, spec);
        progress.finish(&spec, &cell.summary());
        record.cells.push(cell);
        settings.write(&record);
    }
    criterion.final_summary();
}

fn run_latency(settings: &Settings, cells: &[CellSpec]) {
    let mut record = settings.record("latency");
    let calibration = TickCalibration::measure();
    let ns_per_tick = calibration.ns_per_tick;
    eprintln!(
        "[cb-bench] tick calibration {ns_per_tick:.5} ns/tick, windows {:?}",
        calibration.window_estimates
    );
    record.tick_calibration = Some(calibration);
    record.instrumentation_floor = Some(instrumentation_floor());
    let mut progress = Progress::new("latency", cells.len());
    for &spec in cells {
        progress.begin(&spec);
        let cell = spec.implementation.latency(spec);
        progress.finish(&spec, &cell.summary(ns_per_tick));
        record.cells.push(cell);
        settings.write(&record);
    }
}

fn main() {
    if !std::env::args().any(|argument| argument == "--bench") {
        eprintln!(
            "circular_buffer_bench: measurement is opt-in and nothing was executed; use `cargo bench -p hymeko_core --bench circular_buffer_bench`"
        );
        return;
    }
    let settings = Settings::from_env();
    assert_semantic_parity();
    assert_histogram_buckets();
    assert_accounting_rejects_faults();
    let cells = settings.cells();
    eprintln!(
        "[cb-bench] preflight passed (semantic parity, histogram buckets, 8 accounting controls); {} cells, skipped pairs {:?}, output {}",
        cells.len(),
        settings.pairs().1,
        settings.out.display()
    );
    if settings.throughput {
        run_throughput(&settings, &cells);
    }
    if settings.latency {
        run_latency(&settings, &cells);
    }
}
