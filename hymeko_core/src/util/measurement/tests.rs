use super::*;
use std::cell::RefCell;

#[derive(Default)]
struct StepClock(u32);
impl Clock for StepClock {
    type Start = ();
    fn start(&mut self) {}
    fn elapsed_us(&mut self, (): ()) -> f64 {
        self.0 += 1;
        f64::from(self.0)
    }
}

#[test]
fn quantiles_match_sorted_reference_for_odd_even_and_small_samples() {
    for n in 1..40 {
        let mut samples: Vec<f64> = (0..n).rev().map(f64::from).collect();
        let d = Distribution::summarize(&mut samples);
        assert_eq!(d.min, 0.0);
        assert_eq!(d.median, f64::from(n / 2));
        assert_eq!(d.q1, f64::from(n / 4));
        assert_eq!(d.q3, f64::from(3 * n / 4));
        assert_eq!(d.worst, f64::from(n - 1));
    }
}

#[test]
fn partitioned_quantiles_preserve_total_order_including_float_edge_cases() {
    let inputs = [
        f64::NAN,
        -0.0,
        0.0,
        f64::INFINITY,
        f64::NEG_INFINITY,
        3.,
        3.,
        -5.,
    ];
    for n in 1..=inputs.len() {
        for offset in 0..inputs.len() {
            let mut sample: Vec<_> = inputs
                .iter()
                .copied()
                .cycle()
                .skip(offset)
                .take(n)
                .collect();
            let mut reference = sample.clone();
            reference.sort_by(f64::total_cmp);
            let d = Distribution::summarize(&mut sample);
            assert_eq!(
                [d.min, d.q1, d.median, d.q3, d.worst].map(f64::to_bits),
                [
                    reference[0],
                    reference[n / 4],
                    reference[n / 2],
                    reference[3 * n / 4],
                    reference[n - 1]
                ]
                .map(f64::to_bits)
            );
        }
    }
}

#[test]
#[should_panic(expected = "at least one sample")]
fn empty_distribution_is_rejected() {
    Distribution::summarize(&mut []);
}

#[test]
#[should_panic(expected = "at least one sample")]
fn empty_runner_is_rejected() {
    MeasurementRunner::with_clock(2, 0, StepClock::default());
}

#[test]
fn runner_preserves_schedule_raw_samples_and_reuses_all_buffers() {
    let mut runner = MeasurementRunner::with_clock(2, 5, StepClock::default());
    let addresses = (
        runner.samples.as_ptr(),
        runner.other.as_ptr(),
        runner.scratch.as_ptr(),
    );
    let mut calls = 0;
    let first = runner.run(|| {
        calls += 1;
        calls
    });
    assert_eq!(calls, 7);
    assert_eq!(first.median, 3.0);
    assert_eq!(runner.samples(), &[1., 2., 3., 4., 5.]);
    let order = RefCell::new(Vec::new());
    let (a, b) = runner.compare(
        || order.borrow_mut().push('a'),
        || order.borrow_mut().push('b'),
    );
    assert_eq!(
        &*order.borrow(),
        &[
            'a', 'b', 'a', 'b', 'a', 'b', 'b', 'a', 'a', 'b', 'b', 'a', 'a', 'b'
        ]
    );
    assert_eq!(runner.samples(), &[6., 9., 10., 13., 14.]);
    assert_eq!(runner.comparison_samples(), &[7., 8., 11., 12., 15.]);
    assert_eq!((a.median, b.median), (10., 11.));
    runner.run(|| ());
    assert!(runner.comparison_samples().is_empty());
    assert_eq!(
        addresses,
        (
            runner.samples.as_ptr(),
            runner.other.as_ptr(),
            runner.scratch.as_ptr()
        )
    );
    assert_eq!(
        (
            runner.samples.capacity(),
            runner.other.capacity(),
            runner.scratch.capacity()
        ),
        (5, 5, 5)
    );
}

#[test]
fn runner_recovers_from_operation_panic_without_stale_samples() {
    let mut runner = MeasurementRunner::with_clock(0, 3, StepClock::default());
    let mut calls = 0;
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runner.run(|| {
            calls += 1;
            assert_ne!(calls, 2, "operation failed");
        });
    }));
    assert!(panic.is_err());
    assert_eq!(runner.samples(), &[1.]);
    assert_eq!(runner.run(|| ()).median, 3.);
    assert_eq!(runner.samples(), &[2., 3., 4.]);
}

#[test]
fn wall_clock_wiring_and_constructor_are_explicit() {
    let runner = MeasurementRunner::new(2, 5);
    assert!(runner.samples().is_empty());
    let mut clock = WallClock;
    let start = clock.start();
    let elapsed = clock.elapsed_us(start);
    assert!(elapsed.is_finite() && elapsed >= 0.0);
}

#[test]
fn allocation_requests_include_default_zeroing_and_reallocation() {
    let layout = Layout::from_size_align(16, 8).unwrap();
    let before = AllocationSnapshot::current_thread();
    // SAFETY: nonzero valid layouts; each successful allocation is freed once.
    unsafe {
        let p = CountingAllocator.alloc_zeroed(layout);
        assert!(!p.is_null());
        assert_eq!(std::slice::from_raw_parts(p, 16), &[0; 16]);
        let q = CountingAllocator.realloc(p, layout, 32);
        assert!(!q.is_null());
        assert_eq!(std::slice::from_raw_parts(q, 16), &[0; 16]);
        CountingAllocator.dealloc(q, Layout::from_size_align(32, 8).unwrap());
    }
    assert_eq!(
        AllocationSnapshot::current_thread().since(before),
        AllocationSnapshot {
            allocations: 2,
            bytes: 48
        }
    );
}

#[test]
fn allocation_counters_are_thread_local_and_wrap_without_panicking() {
    let before = AllocationSnapshot::current_thread();
    let other = std::thread::spawn(|| {
        let before = AllocationSnapshot::current_thread();
        let layout = Layout::new::<u64>();
        // SAFETY: matching nonzero layouts and one deallocation.
        unsafe {
            let p = CountingAllocator.alloc(layout);
            assert!(!p.is_null());
            CountingAllocator.dealloc(p, layout);
        }
        AllocationSnapshot::current_thread().since(before)
    })
    .join()
    .unwrap();
    assert_eq!(
        other,
        AllocationSnapshot {
            allocations: 1,
            bytes: 8
        }
    );
    assert_eq!(AllocationSnapshot::current_thread(), before);
    assert_eq!(
        AllocationSnapshot {
            allocations: 0,
            bytes: 1
        }
        .since(AllocationSnapshot {
            allocations: u64::MAX,
            bytes: u64::MAX
        }),
        AllocationSnapshot {
            allocations: 1,
            bytes: 2
        }
    );
}
