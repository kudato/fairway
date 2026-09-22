use std::{
    future::{Future, poll_fn},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    task::Poll,
    time::Duration,
};

use tokio::sync::oneshot;

use super::Pool;

const DEADLINE: Duration = Duration::from_secs(5);

async fn finish<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(DEADLINE, future)
        .await
        .expect("compute operation timed out")
}

async fn pending<F: Future>(mut future: Pin<&mut F>) {
    poll_fn(|context| {
        assert!(future.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
}

struct Cleanup {
    started: Option<oneshot::Sender<()>>,
    release: mpsc::Receiver<()>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.started.take().unwrap().send(());
        // A failed test drops the sender, so cleanup cannot hang or panic.
        let _ = self.release.recv_timeout(DEADLINE);
    }
}

#[tokio::test]
async fn cancellation_retains_capacity_through_work_and_cleanup() {
    let pool = Pool::new(1);
    let (started, wait_started) = oneshot::channel();
    let (release, wait_release) = mpsc::channel();
    let (cleanup_started, wait_cleanup) = oneshot::channel();
    let (release_cleanup, wait_release_cleanup) = mpsc::channel();
    let mut running = Box::pin(pool.run(move || {
        started.send(()).unwrap();
        wait_release.recv_timeout(DEADLINE).unwrap();
        Cleanup {
            started: Some(cleanup_started),
            release: wait_release_cleanup,
        }
    }));
    pending(running.as_mut()).await;
    finish(wait_started).await.unwrap();
    drop(running);
    assert_eq!(pool.slots.available_permits(), 0);

    let ran = Arc::new(AtomicBool::new(false));
    let capture = Arc::clone(&ran);
    let mut queued = Box::pin(pool.run(move || capture.store(true, Ordering::SeqCst)));
    pending(queued.as_mut()).await;
    drop(queued);
    assert_eq!(Arc::strong_count(&ran), 1, "queued input must be released");

    release.send(()).unwrap();
    finish(wait_cleanup).await.unwrap();
    assert_eq!(
        pool.slots.available_permits(),
        0,
        "cleanup still owns capacity"
    );
    let mut next = Box::pin(pool.run(|| 42));
    pending(next.as_mut()).await;
    release_cleanup.send(()).unwrap();
    assert_eq!(finish(next).await, 42);
    assert!(
        !ran.load(Ordering::SeqCst),
        "cancelled queued work must not run"
    );
}

#[tokio::test]
async fn submitted_work_runs_after_cancellation_even_if_it_has_not_started() {
    let pool = Pool::new(1);
    let (started, wait_started) = oneshot::channel();
    let (release, wait_release) = mpsc::channel();
    // Occupy the worker without consuming an admission slot, keeping the next
    // run submitted to Rayon but unable to start until after cancellation.
    pool.workers.spawn(move || {
        let _ = started.send(());
        let _ = wait_release.recv_timeout(DEADLINE);
    });
    finish(wait_started).await.unwrap();

    let ran = Arc::new(AtomicBool::new(false));
    let capture = Arc::clone(&ran);
    let mut submitted = Box::pin(pool.run(move || capture.store(true, Ordering::SeqCst)));
    pending(submitted.as_mut()).await;
    assert_eq!(pool.slots.available_permits(), 0);
    assert!(!ran.load(Ordering::SeqCst));
    drop(submitted);

    release.send(()).unwrap();
    finish(pool.run(|| ())).await;
    assert!(ran.load(Ordering::SeqCst));
}

#[tokio::test]
async fn panic_payload_is_preserved_and_single_worker_capacity_is_released() {
    let pool = Arc::new(Pool::new(1));
    let worker_pool = Arc::clone(&pool);
    let task = tokio::spawn(async move {
        worker_pool.run(|| std::panic::panic_any(17_u32)).await;
    });
    let error = finish(task).await.unwrap_err();
    assert_eq!(*error.into_panic().downcast::<u32>().unwrap(), 17);
    assert_eq!(finish(pool.run(|| 42)).await, 42);
}
