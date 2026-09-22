//! Public execution contracts, including operation without a Tokio runtime.

use std::{
    cell::Cell,
    collections::HashSet,
    future::Future,
    pin::pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    task::{Context, Poll, Wake, Waker},
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

use fairway_compute as compute;

const DEADLINE: Duration = Duration::from_secs(5);

fn block_on<F: Future>(future: F) -> F::Output {
    struct Unpark(thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "compute operation timed out");
        thread::park_timeout(remaining);
    }
}

#[test]
fn lazy_execution_moves_non_sync_values_and_custom_errors_without_a_runtime() {
    let caller = thread::current().id();
    let ran = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ran);
    let input = Cell::new(41);
    let work = compute::run(move || {
        assert_ne!(thread::current().id(), caller);
        flag.store(true, Ordering::SeqCst);
        Cell::new(input.get() + 1)
    });
    assert!(!ran.load(Ordering::SeqCst));
    assert_eq!(block_on(work).get(), 42);
    assert!(ran.load(Ordering::SeqCst));
    assert_eq!(block_on(compute::run(|| Err::<(), _>(7_u8))), Err(7));

    let input = Arc::new(());
    let capture = Arc::clone(&input);
    let unpolled = compute::run(move || {
        drop(capture);
        panic!("unpolled work must never execute");
    });
    drop(unpolled);
    assert_eq!(Arc::strong_count(&input), 1);
}

#[test]
fn callers_and_rayon_subtasks_share_one_pool_across_runtimes() {
    fn workers() -> HashSet<ThreadId> {
        rayon::broadcast(|_| {
            assert!(
                thread::current()
                    .name()
                    .unwrap_or_default()
                    .starts_with("fairway-compute-")
            );
            thread::current().id()
        })
        .into_iter()
        .collect()
    }

    let first = {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(compute::run(workers))
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let second = runtime.block_on(compute::run(workers));
    assert_eq!(first, second);
    assert_eq!(first, block_on(compute::run(workers)));
    assert_eq!(
        first.len(),
        thread::available_parallelism().map_or(1, usize::from)
    );
}

#[test]
fn submitted_work_survives_runtime_shutdown() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let (release, wait_release) = mpsc::channel();
    let (finished, wait_finished) = mpsc::channel();
    let task = runtime.spawn(compute::run(move || {
        started.send(()).unwrap();
        wait_release.recv_timeout(DEADLINE).unwrap();
        finished.send(()).unwrap();
    }));
    runtime.block_on(async {
        tokio::time::timeout(DEADLINE, wait_started)
            .await
            .unwrap()
            .unwrap();
    });
    drop(runtime);
    release.send(()).unwrap();
    wait_finished.recv_timeout(DEADLINE).unwrap();
    assert!(block_on(task).unwrap_err().is_cancelled());
}
