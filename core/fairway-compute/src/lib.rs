//! Synchronous computations in a shared pool with asynchronous result delivery.
//!
//! The pool is initialized on first use and shared across callers and async runtimes.
//! Its thread count is [`std::thread::available_parallelism()`], falling back to one.
//! Waiting for capacity and results requires no Tokio runtime.
//!
//! ```
//! use fairway_compute as compute;
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! let mut values = vec![3, 1, 2];
//! let sorted = compute::run(move || {
//!     values.sort_unstable();
//!     values
//! }).await;
//! assert_eq!(sorted, [1, 2, 3]);
//! # }
//! ```

use std::{
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{Arc, OnceLock},
};

use tokio::sync::{Semaphore, oneshot};

struct Pool {
    workers: rayon::ThreadPool,
    slots: Arc<Semaphore>,
}

impl Pool {
    fn new(threads: usize) -> Self {
        Self {
            workers: rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|index| format!("fairway-compute-{index}"))
                .build()
                .expect("could not start Fairway's compute pool"),
            slots: Arc::new(Semaphore::new(threads)),
        }
    }

    async fn run<T: Send + 'static>(&self, work: impl FnOnce() -> T + Send + 'static) -> T {
        // Backpressure happens before submitting to Rayon. A cancelled waiter
        // drops its input instead of leaving an unbounded queue of CPU jobs.
        let permit = Arc::clone(&self.slots)
            .acquire_owned()
            .await
            .expect("compute pool stays open");
        let (send, receive) = oneshot::channel();
        self.workers.spawn(move || {
            // A detached task retains its slot until work and cleanup finish.
            let _permit = permit;
            let result = catch_unwind(AssertUnwindSafe(work));
            let _ = send.send(result);
        });
        match receive
            .await
            .expect("compute worker must deliver its result")
        {
            Ok(value) => value,
            Err(panic) => resume_unwind(panic),
        }
    }
}

/// Runs synchronous work in the shared compute pool and awaits its result.
///
/// The closure and its result must be `Send + 'static`; neither needs to be `Sync`.
/// A returned `Result` is passed through unchanged, without another error wrapper.
/// The returned future is `Send`, and creating it does not start the work.
///
/// At most one unfinished call per pool thread is admitted at a time. Other calls
/// wait asynchronously while retaining their inputs. Work always runs in the pool,
/// leaving async executor threads and blocking I/O pools available.
///
/// The closure should compute in memory and finish its work before returning.
/// Rayon parallel iterators and `rayon::join` use the same pool inside the closure.
/// Blocking on a nested `run` can deadlock because the outer call holds capacity.
///
/// # Cancellation
///
/// Dropping the future before submission drops the closure without calling it.
/// Once submitted, the work runs even if it has not started yet. It retains its
/// capacity until work and cleanup finish; a result without a receiver is discarded.
/// Stopping an async runtime does not stop the pool's submitted work.
///
/// # Panics
///
/// Panics if the pool cannot be created. With unwinding enabled, a panic from
/// `work` is resumed in the awaiting code with its original payload. The pool
/// remains usable. With `panic = "abort"`, a panic terminates the process.
pub async fn run<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(std::thread::available_parallelism().map_or(1, usize::from)))
        .run(work)
        .await
}

#[cfg(test)]
mod tests;
