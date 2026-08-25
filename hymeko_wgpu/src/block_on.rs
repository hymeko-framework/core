//! Minimal blocking executor for the two `async` calls in wgpu's init path.
//!
//! `Instance::request_adapter` and `Adapter::request_device` return futures.
//! On native backends they resolve without needing an event loop, so a
//! park/unpark driver is sufficient and no async runtime is required.
//!
//! This exists instead of a `pollster` dependency on purpose: `pollster` is not
//! part of the wgpu dependency tree that was authorised for vendoring, and
//! pulling in a crate for twenty lines would have widened that authorisation
//! for no benefit. It is written with the safe `std::task::Wake` trait — **no
//! `unsafe`**, no raw waker vtable.

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

/// Wakes the thread that is blocked inside [`block_on`].
struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Drive `future` to completion on the current thread.
///
/// # Invariants
/// Parks rather than spins, so a future that is slow to resolve does not burn a
/// core — the task's "avoid device busy-poll" rule applies here too.
///
/// Spurious unparks are harmless: the loop simply re-polls.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);

    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::block_on;

    #[test]
    fn resolves_an_immediately_ready_future() {
        assert_eq!(block_on(async { 41 + 1 }), 42);
    }

    #[test]
    fn resolves_a_future_woken_from_another_thread() {
        // Exercises the park/unpark path rather than the ready-on-first-poll
        // path, which is the case that would hang if `Wake` were wired wrong.
        let (tx, rx) = std::sync::mpsc::channel::<u32>();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let _ = tx.send(7);
        });
        let value = block_on(async move { rx.recv().unwrap() });
        assert_eq!(value, 7);
    }
}
