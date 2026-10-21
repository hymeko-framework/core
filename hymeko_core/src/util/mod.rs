/// Explicit measurement runners and opt-in allocation instrumentation.
pub mod measurement;
pub mod pretty_print;
pub mod real_parser;

/// Fixed-capacity, multi-producer multi-consumer circular FIFO buffer.
///
/// This is Crossbeam's `ArrayQueue`, re-exported without another synchronization
/// layer. Share it with `Arc` (or scoped thread references). A payload only needs
/// `Send` for the buffer to be both `Send` and `Sync`.
///
/// # Contract
///
/// - `new(capacity)` allocates fixed storage; capacity must be positive.
/// - `push(value)` returns `Err(value)` when full, retaining caller ownership.
/// - `force_push(value)` explicitly allows overwrite and returns the oldest
///   evicted value, if any. Only use it when discarding that value is acceptable.
/// - `pop()` transfers the oldest available value, or returns `None` when empty.
/// - Concurrent operations may interleave. `len`, `is_empty` and `is_full` are
///   observations, not reservations: use the operation's result for decisions.
///
/// # Panics
///
/// Construction panics for zero or excessively large capacity. Allocation is
/// fallible at the allocator level; this API does not provide allocation recovery.
///
/// # Qualification boundary
///
/// The dedicated `circular_buffer` correctness target covers FIFO, overwrite,
/// ownership, MPMC delivery and bounded concurrent histories. It is not a formal
/// proof over every schedule or architecture. No fairness, wait-free completion,
/// real-time deadline, persistence or process-shared memory contract is provided.
#[doc(inline)]
pub use crossbeam_queue::ArrayQueue as CircularBuffer;
