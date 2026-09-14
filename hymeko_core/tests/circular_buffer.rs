//! Bounded functional qualification, without benchmarks or timing assertions.
use hymeko::util::CircularBuffer;
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn fifo_wrap_and_overwrite_match_sequential_model() {
    for capacity in [1, 2, 3, 7] {
        let queue = CircularBuffer::new(capacity);
        let mut model = VecDeque::new();
        // Fixed deterministic operations, including many reuses of every slot.
        for value in 0..128 {
            let expected = if model.len() == capacity {
                Err(value)
            } else {
                model.push_back(value);
                Ok(())
            };
            assert_eq!(queue.push(value), expected);
            if value % 3 == 0 {
                assert_eq!(queue.pop(), model.pop_front());
            }
            if value % 5 == 0 {
                let evicted = (model.len() == capacity)
                    .then(|| model.pop_front())
                    .flatten();
                model.push_back(value + 1000);
                assert_eq!(queue.force_push(value + 1000), evicted);
            }
            assert_eq!(queue.capacity(), capacity);
            assert_eq!(queue.len(), model.len());
            assert_eq!(queue.is_empty(), model.is_empty());
            assert_eq!(queue.is_full(), model.len() == capacity);
        }
        while let Some(value) = model.pop_front() {
            assert_eq!(queue.pop(), Some(value));
        }
        assert_eq!(queue.pop(), None);
    }
}

#[test]
#[should_panic(expected = "capacity must be non-zero")]
fn zero_capacity_is_rejected() {
    let _ = CircularBuffer::<u8>::new(0);
}

#[derive(Debug)]
struct TrackedDrop(Arc<AtomicUsize>);

impl Drop for TrackedDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, SeqCst);
    }
}

#[test]
fn rejected_evicted_popped_and_retained_values_drop_once() {
    let counts: Vec<_> = (0..5).map(|_| Arc::new(AtomicUsize::new(0))).collect();
    {
        let queue = CircularBuffer::new(2);
        queue
            .push(TrackedDrop(counts[0].clone()))
            .expect("free slot");
        queue
            .push(TrackedDrop(counts[1].clone()))
            .expect("free slot");
        let rejected = queue
            .push(TrackedDrop(counts[2].clone()))
            .expect_err("full");
        let evicted = queue.force_push(TrackedDrop(counts[3].clone()));
        let popped = queue.pop();
        queue
            .push(TrackedDrop(counts[4].clone()))
            .expect("released slot");
        assert!(counts.iter().all(|count| count.load(SeqCst) == 0));
        drop((rejected, evicted, popped));
        assert_eq!(
            counts.iter().map(|c| c.load(SeqCst)).collect::<Vec<_>>(),
            [1, 1, 1, 0, 0]
        );
    }
    assert!(counts.iter().all(|count| count.load(SeqCst) == 1));
}

#[test]
fn send_only_payload_can_cross_threads() {
    let queue = CircularBuffer::new(1);
    thread::scope(|scope| {
        scope.spawn(|| queue.push(Cell::new(42)).expect("empty queue"));
    });
    assert_eq!(queue.pop().expect("published payload").get(), 42);
}

#[test]
fn mpmc_accounts_for_every_value_exactly_once() {
    for capacity in [1, 3, 7] {
        for overwrite in [false, true] {
            let queue = CircularBuffer::new(capacity);
            let start = Barrier::new(4);
            let mut returned = thread::scope(|scope| {
                let handles: Vec<_> = (0..4)
                    .map(|producer| {
                        let queue = &queue;
                        let start = &start;
                        scope.spawn(move || {
                            let mut returned = Vec::new();
                            start.wait();
                            for sequence in 0..64 {
                                let value = producer * 64 + sequence;
                                let displaced = if overwrite {
                                    queue.force_push(value)
                                } else {
                                    queue.push(value).err()
                                };
                                returned.extend(displaced);
                                returned.extend(queue.pop());
                                thread::yield_now();
                            }
                            returned
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .flat_map(|h| h.join().expect("worker must not panic"))
                    .collect::<Vec<_>>()
            });
            while let Some(value) = queue.pop() {
                returned.push(value);
            }
            returned.sort_unstable();
            assert_eq!(returned, (0..256).collect::<Vec<_>>());
        }
    }
}

#[test]
fn concurrent_producers_preserve_each_producers_order() {
    let queue = CircularBuffer::new(128);
    let start = Barrier::new(4);
    thread::scope(|scope| {
        for producer in 0..4 {
            let queue = &queue;
            let start = &start;
            scope.spawn(move || {
                start.wait();
                for sequence in 0..32 {
                    queue
                        .push((producer, sequence))
                        .expect("capacity covers all producers");
                }
            });
        }
    });
    let mut next = [0; 4];
    while let Some((producer, sequence)) = queue.pop() {
        assert_eq!(sequence, next[producer]);
        next[producer] += 1;
    }
    assert_eq!(next, [32; 4]);
}

#[derive(Clone, Copy, Debug)]
enum Operation {
    Push(usize),
    ForcePush(usize),
    Pop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Pushed(Result<(), usize>),
    Removed(Option<usize>),
}

#[derive(Clone, Copy, Debug)]
struct Event {
    begin: usize,
    end: usize,
    operation: Operation,
    outcome: Outcome,
}

impl Operation {
    fn execute(self, queue: &CircularBuffer<usize>, clock: &AtomicUsize) -> Event {
        let begin = clock.fetch_add(1, SeqCst);
        let outcome = match self {
            Self::Push(value) => Outcome::Pushed(queue.push(value)),
            Self::ForcePush(value) => Outcome::Removed(queue.force_push(value)),
            Self::Pop => Outcome::Removed(queue.pop()),
        };
        Event {
            begin,
            end: clock.fetch_add(1, SeqCst),
            operation: self,
            outcome,
        }
    }

    fn model(self, queue: &mut VecDeque<usize>, capacity: usize) -> Outcome {
        match self {
            Self::Push(value) if queue.len() == capacity => Outcome::Pushed(Err(value)),
            Self::Push(value) => {
                queue.push_back(value);
                Outcome::Pushed(Ok(()))
            }
            Self::ForcePush(value) => {
                let evicted = if queue.len() == capacity {
                    queue.pop_front()
                } else {
                    None
                };
                queue.push_back(value);
                Outcome::Removed(evicted)
            }
            Self::Pop => Outcome::Removed(queue.pop_front()),
        }
    }
}

struct History(Vec<Event>);

impl History {
    fn is_linearizable(&self, capacity: usize) -> bool {
        assert!(self.0.len() <= 16, "bounded correctness oracle");
        let complete = (1u32 << self.0.len()) - 1;
        let mut pending = vec![(0u32, VecDeque::new())];
        while let Some((used, model)) = pending.pop() {
            if used == complete {
                return true;
            }
            for (index, event) in self.0.iter().enumerate() {
                let precedes = self
                    .0
                    .iter()
                    .enumerate()
                    .any(|(other, prior)| prior.end < event.begin && used & (1 << other) == 0);
                if used & (1 << index) != 0 || precedes {
                    continue;
                }
                let mut candidate = model.clone();
                if event.operation.model(&mut candidate, capacity) == event.outcome {
                    pending.push((used | (1 << index), candidate));
                }
            }
        }
        false
    }
}

#[test]
fn concurrent_histories_have_a_legal_fifo_linearization() {
    for capacity in [1, 2, 3] {
        let queue = CircularBuffer::new(capacity);
        let clock = AtomicUsize::new(0);
        let start = Barrier::new(4);
        let mut events = thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|worker| {
                    let queue = &queue;
                    let clock = &clock;
                    let start = &start;
                    scope.spawn(move || {
                        start.wait();
                        let operation = if worker % 2 == 0 {
                            Operation::Push(worker)
                        } else {
                            Operation::ForcePush(worker)
                        };
                        let pushed = operation.execute(queue, clock);
                        thread::yield_now();
                        [pushed, Operation::Pop.execute(queue, clock)]
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().expect("worker must not panic"))
                .collect::<Vec<_>>()
        });
        // Include the final contents (and empty observation) in the same witness.
        loop {
            let event = Operation::Pop.execute(&queue, &clock);
            events.push(event);
            if event.outcome == Outcome::Removed(None) {
                break;
            }
        }
        let history = History(events);
        assert!(
            history.is_linearizable(capacity),
            "{history:?}",
            history = history.0
        );
    }
}

#[test]
fn history_oracle_accepts_overlap_and_rejects_impossible_results() {
    let pushed = Event {
        begin: 0,
        end: 3,
        operation: Operation::Push(7),
        outcome: Outcome::Pushed(Ok(())),
    };
    let popped = Event {
        begin: 1,
        end: 2,
        operation: Operation::Pop,
        outcome: Outcome::Removed(Some(7)),
    };
    assert!(History(vec![pushed, popped]).is_linearizable(1));
    assert!(
        !History(vec![
            pushed,
            Event {
                outcome: Outcome::Removed(Some(8)),
                ..popped
            }
        ])
        .is_linearizable(1)
    );
    // Real-time precedence forbids moving a later push before an earlier pop.
    assert!(
        !History(vec![
            Event {
                begin: 4,
                end: 5,
                ..pushed
            },
            popped
        ])
        .is_linearizable(1)
    );
    assert!(
        !History(vec![
            pushed,
            popped,
            Event {
                begin: 4,
                end: 5,
                ..popped
            }
        ])
        .is_linearizable(1)
    );
    assert!(
        !History(vec![Event {
            outcome: Outcome::Pushed(Err(7)),
            ..pushed
        }])
        .is_linearizable(1)
    );
}
