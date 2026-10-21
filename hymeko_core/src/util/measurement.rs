//! Shared measurement machinery. Nothing runs or installs an allocator implicitly.
//!
//! Runners own reusable sample storage; clocks and operations use static dispatch.
//! Times are microseconds. Summaries and result destruction happen outside timing.
//! Allocation snapshots count requests on the calling thread, not live bytes/RSS.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::Instant;

/// Order statistics in the input's units, using the upper middle element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Distribution {
    pub min: f64,
    pub median: f64,
    pub q1: f64,
    pub q3: f64,
    pub worst: f64,
}

impl Distribution {
    /// Partition caller-owned scratch in place, without heap allocation.
    /// Three order-statistic selections and extrema scans take O(n) work;
    /// the whole sample is not sorted. Input order is not preserved.
    /// Quantiles use indices `n/4`, `n/2`, `3*n/4`; values use `total_cmp`.
    /// # Panics
    /// Panics for an empty sample. Statistical sample-size policy belongs to the caller.
    pub fn summarize(values: &mut [f64]) -> Self {
        assert!(
            !values.is_empty(),
            "a distribution needs at least one sample"
        );
        let n = values.len();
        let middle = n / 2;
        let (lower, median, upper) = values.select_nth_unstable_by(middle, f64::total_cmp);
        let median = *median;
        let min = lower
            .iter()
            .copied()
            .min_by(f64::total_cmp)
            .unwrap_or(median);
        let worst = upper
            .iter()
            .copied()
            .max_by(f64::total_cmp)
            .unwrap_or(median);
        let q1 = if lower.is_empty() {
            median
        } else {
            *lower.select_nth_unstable_by(n / 4, f64::total_cmp).1
        };
        let upper_quartile = n - n.div_ceil(4);
        let q3 = if upper_quartile == middle {
            median
        } else {
            *upper
                .select_nth_unstable_by(upper_quartile - middle - 1, f64::total_cmp)
                .1
        };
        Self {
            min,
            median,
            q1,
            q3,
            worst,
        }
    }
}

/// Replaceable monotonic clock; deterministic clocks can verify the runner itself.
pub trait Clock {
    type Start;
    fn start(&mut self) -> Self::Start;
    /// Elapsed, finite non-negative microseconds since `start`.
    fn elapsed_us(&mut self, start: Self::Start) -> f64;
}

/// The standard library's monotonic wall clock.
#[derive(Default, Debug)]
pub struct WallClock;

impl Clock for WallClock {
    type Start = Instant;
    fn start(&mut self) -> Instant {
        Instant::now()
    }
    fn elapsed_us(&mut self, start: Instant) -> f64 {
        start.elapsed().as_secs_f64() * 1e6
    }
}

/// Execute a caller-supplied operation under a measurement policy.
pub trait Runner {
    /// Warm up, sample, then summarize. Operation panics propagate to the caller.
    fn run<T>(&mut self, operation: impl FnMut() -> T) -> Distribution;
}

/// Fixed-count runner. Three buffers are allocated once and reused for every run.
/// The same storage also supports alternating paired comparisons (AB, BA, ...).
pub struct MeasurementRunner<C = WallClock> {
    clock: C,
    warmups: usize,
    count: usize,
    samples: Vec<f64>,
    other: Vec<f64>,
    scratch: Vec<f64>,
}

impl MeasurementRunner {
    /// Create a wall-clock runner. No operation is executed until `run`/`compare`.
    /// # Panics
    /// Panics when `samples == 0` or its storage cannot be allocated.
    pub fn new(warmups: usize, samples: usize) -> Self {
        Self::with_clock(warmups, samples, WallClock)
    }
}

impl<C: Clock> MeasurementRunner<C> {
    /// Supply a clock strategy; sample capacity is fixed for the runner's lifetime.
    /// # Panics
    /// Panics when `samples == 0` or its storage cannot be allocated.
    pub fn with_clock(warmups: usize, samples: usize, clock: C) -> Self {
        assert!(samples > 0, "a runner needs at least one sample");
        Self {
            clock,
            warmups,
            count: samples,
            samples: Vec::with_capacity(samples),
            other: Vec::with_capacity(samples),
            scratch: Vec::with_capacity(samples),
        }
    }

    /// Most recent first-operation samples, in execution order, without copying.
    /// After a panic this is the completed prefix; the next run clears it.
    pub fn samples(&self) -> &[f64] {
        &self.samples
    }

    /// Second-operation samples from the latest comparison; empty after `run`.
    pub fn comparison_samples(&self) -> &[f64] {
        &self.other
    }

    fn sample<T>(clock: &mut C, operation: &mut impl FnMut() -> T) -> f64 {
        let start = clock.start();
        let result = operation();
        let elapsed = clock.elapsed_us(start);
        black_box(result);
        elapsed
    }

    fn summarize(&mut self, second: bool) -> Distribution {
        self.scratch.clear();
        self.scratch
            .extend_from_slice(if second { &self.other } else { &self.samples });
        Distribution::summarize(&mut self.scratch)
    }

    /// Warm both operations in AB order, then alternate AB/BA for each sample pair.
    /// Results correspond to the arguments, regardless of execution order.
    /// Panics propagate; a subsequent call starts fresh and reuses the buffers.
    pub fn compare<A, B>(
        &mut self,
        mut a: impl FnMut() -> A,
        mut b: impl FnMut() -> B,
    ) -> (Distribution, Distribution) {
        self.samples.clear();
        self.other.clear();
        for _ in 0..self.warmups {
            black_box(a());
            black_box(b());
        }
        for ordinal in 0..self.count {
            if ordinal % 2 == 0 {
                self.samples.push(Self::sample(&mut self.clock, &mut a));
                self.other.push(Self::sample(&mut self.clock, &mut b));
            } else {
                self.other.push(Self::sample(&mut self.clock, &mut b));
                self.samples.push(Self::sample(&mut self.clock, &mut a));
            }
        }
        (self.summarize(false), self.summarize(true))
    }
}

impl<C: Clock> Runner for MeasurementRunner<C> {
    fn run<T>(&mut self, mut operation: impl FnMut() -> T) -> Distribution {
        self.samples.clear();
        self.other.clear();
        for _ in 0..self.warmups {
            black_box(operation());
        }
        for _ in 0..self.count {
            self.samples
                .push(Self::sample(&mut self.clock, &mut operation));
        }
        self.summarize(false)
    }
}

/// Cumulative allocation requests on one thread. This is not retained memory.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct AllocationSnapshot {
    pub allocations: u64,
    pub bytes: u64,
}

thread_local! {
    static ALLOCATIONS: Cell<AllocationSnapshot> = const { Cell::new(AllocationSnapshot { allocations: 0, bytes: 0 }) };
}

impl AllocationSnapshot {
    /// Read counters without allocation. Requires an installed `CountingAllocator`
    /// to observe allocations; otherwise returns zero. Worker threads are excluded.
    pub fn current_thread() -> Self {
        ALLOCATIONS.with(Cell::get)
    }

    /// Wrapping delta from an earlier snapshot of this same thread/counter stream.
    pub fn since(self, earlier: Self) -> Self {
        Self {
            allocations: self.allocations.wrapping_sub(earlier.allocations),
            bytes: self.bytes.wrapping_sub(earlier.bytes),
        }
    }
}

/// Opt-in System allocator instrumentation. Install explicitly in a test binary.
/// Uses one constant-initialized TLS cell, no mutex, heap allocation or atomics.
/// Counts attempted allocations, including failures. `GlobalAlloc`'s default
/// zeroed/reallocation paths preserve the historical harness's request semantics.
pub struct CountingAllocator;

// SAFETY: pointers/layouts pass unchanged to System. Counter updates never unwind
// or allocate; try_with also tolerates thread teardown, and additions wrap.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|cell| {
            let before = cell.get();
            cell.set(AllocationSnapshot {
                allocations: before.allocations.wrapping_add(1),
                bytes: before.bytes.wrapping_add(layout.size() as u64),
            });
        });
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[cfg(test)]
mod tests;
