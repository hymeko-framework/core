//! Allocation counter for `HyperGraphView::from_ir`.
//!
//! Counts heap allocations and bytes requested during one `from_ir` call, for
//! the weighted (`weights: Some(..)`) and unweighted (`weights: None`)
//! workloads at two scales. The gap between the two isolates
//! `hymeko::tensor::tensor_val::extract_ref_weight_scalar`, which is the only
//! thing that differs between the two IRs.
//!
//! Run with:
//! ```text
//! cargo run --release --example alloc_count -p hymeko_hnn
//! ```
//!
//! Why an example and not `dhat`: `dhat` is the toolchain-pinned allocation
//! profiler, but wiring it in means adding a dependency to `hymeko_core`, which
//! is a CORE.YAML dependency event. A counting `GlobalAlloc` in a single
//! example binary answers the one question here ("how many allocations per
//! `from_ir`?") with no dependency and no change to the library.
//!
//! The counter is process-wide by construction — that is what `#[global_alloc]`
//! means — so this binary does exactly one thing and does it single-threaded at
//! the top level. Rayon worker allocations inside `from_ir` are counted too,
//! which is intended: the question is total allocations, not per-thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg};
use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
use hymeko_hnn::workloads::HyperSpec;

type View = HyperGraphView<f32, EdgeWScalar<f32>, f32>;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Allocator that tallies every allocation, then delegates to the system one.
struct CountingAllocator;

// SAFETY: every method forwards to `System`, the allocator whose contract this
// impl is standing in for, with the *same* pointer and the *same* `Layout` it
// was handed.
//
//   * `alloc` returns exactly what `System.alloc(layout)` returned, so the
//     pointer's validity, alignment and size guarantees are System's, unchanged.
//   * `dealloc` is only ever reached with a pointer this allocator returned,
//     i.e. one that came from `System.alloc` under the identical layout, which
//     is precisely `System.dealloc`'s precondition.
//   * The tally is two relaxed atomic adds. They touch no allocator state, can
//     not unwind, and do not reenter the allocator, so they cannot perturb the
//     invariants above. Relaxed ordering is sufficient because the counters are
//     only read after the measured region has completed.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(new_size, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn reset() {
    ALLOC_COUNT.store(0, Ordering::SeqCst);
    ALLOC_BYTES.store(0, Ordering::SeqCst);
}

fn read() -> (usize, usize) {
    (
        ALLOC_COUNT.load(Ordering::SeqCst),
        ALLOC_BYTES.load(Ordering::SeqCst),
    )
}

fn cfg() -> AggCfg {
    AggCfg {
        weight: WeightAgg::Sum,
        sign: SignAgg::PreferNonNeutral,
        clamp01: false,
    }
}

fn main() {
    let scales: [(&str, HyperSpec); 2] =
        [("small", HyperSpec::small()), ("large", HyperSpec::large())];

    println!(
        "{:<8} {:<14} {:>12} {:>14} {:>16} {:>14}",
        "scale", "weights", "incidences", "allocations", "bytes", "allocs/inc"
    );
    println!("{}", "-".repeat(84));

    for (label, spec) in scales {
        for weighted in [true, false] {
            // Built OUTSIDE the measured region: only `from_ir` is counted.
            let ir = if weighted {
                spec.ir()
            } else {
                spec.ir_unweighted()
            };
            let incidences: usize = ir.arcs.iter().map(|a| a.refs.len()).sum();
            let c = cfg();
            let ex = ScalarWeightExtractor;

            reset();
            let view = View::from_ir(&ir, &c, &ex);
            let (allocs, bytes) = read();

            // Keep the view alive past the read so nothing is optimised out.
            std::hint::black_box(&view);

            println!(
                "{:<8} {:<14} {:>12} {:>14} {:>16} {:>14.3}",
                label,
                if weighted { "Some(weight)" } else { "None" },
                incidences,
                allocs,
                bytes,
                allocs as f64 / incidences.max(1) as f64
            );
        }
    }
}
