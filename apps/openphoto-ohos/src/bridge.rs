//! Async bridge calls from worker threads.
//!
//! Ability bridge calls (`show_file_dialog`, …) resolve on the ArkTS side while Rust waits. The
//! ability main thread must never block (it drives frames and owns the N-API environment), so
//! every dialog runs on a short-lived worker thread that parks until the future resolves. No async
//! runtime is needed: the future is woken by the bridge's thread-safe callback.
use std::future::Future;
use std::sync::Arc;
use std::task::{Poll, Waker};

struct ThreadWaker(std::thread::Thread);

impl std::task::Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Drives `future` to completion on the current (worker) thread. Only for bridge futures, which
/// resolve from the bridge's own threads; anything else would park forever.
pub fn block_on<F>(future: F) -> F::Output
where
    F: Future,
{
    let waker: Waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    // Pin the future on the heap without unsafe: `Box::pin` moves it there once.
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}
