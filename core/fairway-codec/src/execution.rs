use std::{
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::OnceLock,
};

use tokio::sync::{Semaphore, oneshot};

struct Pool {
    workers: rayon::ThreadPool,
    slots: Semaphore,
}

impl Pool {
    fn new(threads: usize) -> Self {
        Self {
            workers: rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|index| format!("fairway-codec-{index}"))
                .build()
                .expect("could not start Fairway's compute pool"),
            slots: Semaphore::new(threads),
        }
    }

    async fn run<T: Send + 'static>(&'static self, work: impl FnOnce() -> T + Send + 'static) -> T {
        // Backpressure happens before submitting to Rayon. A cancelled waiter
        // drops its input instead of leaving an unbounded queue of CPU jobs.
        let permit = self.slots.acquire().await.expect("compute pool stays open");
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

pub(crate) async fn run<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(std::thread::available_parallelism().map_or(1, usize::from)))
        .run(work)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::poll_fn,
        pin::pin,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        task::Poll,
    };

    #[tokio::test]
    async fn cancelled_jobs_retain_capacity_and_queued_jobs_can_be_cancelled() {
        let pool: &'static Pool = Box::leak(Box::new(Pool::new(1)));
        let (started, wait_started) = oneshot::channel();
        let (release, wait_release) = mpsc::channel();
        let task = tokio::spawn(pool.run(move || {
            started.send(()).unwrap();
            wait_release.recv().unwrap();
        }));
        wait_started.await.unwrap();
        assert_eq!(pool.slots.available_permits(), 0);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(pool.slots.available_permits(), 0);

        let ran = Arc::new(AtomicBool::new(false));
        let capture = Arc::clone(&ran);
        {
            let mut queued = pin!(pool.run(move || capture.store(true, Ordering::SeqCst)));
            poll_fn(|context| {
                assert!(queued.as_mut().poll(context).is_pending());
                Poll::Ready(())
            })
            .await;
        }
        // Queue cancellation releases the captured input without running it.
        assert_eq!(Arc::strong_count(&ran), 1);
        release.send(()).unwrap();
        pool.run(|| ()).await;
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn worker_panics_reach_the_caller_and_do_not_destroy_the_pool() {
        let pool: &'static Pool = Box::leak(Box::new(Pool::new(1)));
        let task = tokio::spawn(pool.run(|| panic!("test codec panic")));
        assert!(task.await.unwrap_err().is_panic());
        assert_eq!(pool.run(|| 42).await, 42);
    }
}
