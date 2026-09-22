# Computation

`fairway-compute` runs synchronous computations in a shared thread pool
and returns their results to asynchronous code. Use it for processing data
in memory: sorting, text analysis, building indexes, and other computations.

## Quick start

`run` accepts a closure containing the computation. `.await` waits for pool
capacity and then for the result without blocking the waiting thread.

```rust
use fairway_compute as compute;

async fn sorted(mut values: Vec<u64>) -> Vec<u64> {
    compute::run(move || {
        values.sort_unstable();
        values
    })
    .await
}
```

`move` transfers the input into the closure. The result is returned directly,
with no additional wrapper around `Vec<u64>`.

## Text processing example

In this example, `run` receives text that has already been read and returns
the number of words.

```rust
use std::{io, path::Path};

use fairway_compute as compute;
use fairway_fs as fs;

async fn count_words(path: &Path) -> io::Result<usize> {
    let text: String = fs::read(path).await?;
    let count = compute::run(move || text.split_whitespace().count()).await;
    Ok(count)
}
```

## Returning an error

A closure can return `Result<T, E>`. `run` passes it back unchanged,
so the caller can use the usual `?` after `.await`.

```rust
use std::num::ParseIntError;

use fairway_compute as compute;

async fn parse_numbers(text: String) -> Result<Vec<u64>, ParseIntError> {
    compute::run(move || text.lines().map(str::parse::<u64>).collect()).await
}
```

## Parallel computation

Multiple concurrently awaited `run` calls can execute in parallel within
the shared limit. Consecutive calls with `.await` run their work in sequence.

One `run` call moves a closure to a pool thread. It does not automatically
parallelize the algorithm. Inside the closure, use parallel iterators or
`join` from [Rayon](https://docs.rs/rayon/latest/rayon/struct.ThreadPool.html)
to run subtasks in the same pool.

```rust
use fairway_compute as compute;
use rayon::prelude::*;

async fn parallel_sorted(mut values: Vec<u64>) -> Vec<u64> {
    compute::run(move || {
        values.par_sort_unstable();
        values
    })
    .await
}
```

This example requires a direct dependency on `rayon` in the plugin.
Subtasks must finish before the closure returns, as parallel iterators and
`rayon::join` do. `run` does not track detached background tasks.

## API

### run

- `async fn run<T>(work: impl FnOnce() -> T + Send + 'static) -> T`
  runs `work` in the shared compute pool and returns its result.
  Requires `T: Send + 'static`.

The closure is synchronous and returns a completed result. An `async` block
returned by the closure is only constructed, not executed by the compute pool.

The compute pool is intended for processing data in memory.
I/O and waiting for external operations take place outside the pool.

`Send` allows the closure and result to move between threads. `'static`
rules out shorter-lived borrows but does not require keeping the data until
the process exits. Typically, the closure owns its inputs through `move`;
use `Arc` for shared ownership. `move` alone does not extend the lifetime
of captured references.

If the result is `Result<T, E>`, both types must be `Send + 'static`.
`run` imposes no `Sync` or `std::error::Error` bounds.
The returned future implements `Send`.

Creating the future does not start the work. Work is submitted when the future
is polled and obtains pool capacity. Once submitted, `work` is called once.
Independent calls have no guaranteed start or completion order.

### Pool and capacity

The Rayon pool is shared by all `compute::run` calls from core crates and plugins.
Work always runs in the pool regardless of input size.

The pool is created automatically when `run` is first polled and lives until
the process exits. Its thread count is determined once through
`std::thread::available_parallelism()`, falling back to one thread if it
cannot be determined. Configuring Tokio threads does not change this limit.

The number of unfinished `run` calls submitted to the pool is limited to
the number of threads. Other calls wait asynchronously for capacity before
submitting work to Rayon. This limit applies to `run` calls; Rayon subtasks
use the same threads. Waiting calls retain their inputs, so this limit does
not bound total memory usage or the number of futures created by callers.

Computations occupy neither Tokio worker threads nor its blocking I/O pool.
`run` does not create an asynchronous runtime. The crate can be used in tests
and other applications without starting `fairway`; the pool is not tied to
a particular runtime. Waiting uses [Tokio primitives compatible with different executors](https://docs.rs/tokio/latest/tokio/sync/index.html#runtime-compatibility).

Shutting down the runtime does not stop computations already submitted to
the pool. To let them finish before the application exits, the caller must
await the corresponding `run` calls.

Within a job, call nested computations synchronously or through Rayon.
Blocking on a nested `compute::run` can deadlock: the outer job holds pool
capacity needed by the nested job.

### Cancellation

Cancellation means dropping the future returned by `run`:

- Before submission to the pool, the closure is never called, and its captures
  are released along with the future.
- After submission, the work runs even if it has not started yet. Cancelling
  the wait does not interrupt it. It retains its pool capacity until the work
  and cleanup of its remaining resources finish.
- If the wait is cancelled, the completed result is discarded.

A timeout follows the same rules: a computation already submitted to the pool
continues running. To finish early, a long-running algorithm must check
a cancellation signal passed to it and return a result itself.

### Errors and panics

`run` returns the closure's result directly. It has no error type of its own
and adds no `Result` wrapper.

With `panic = "unwind"`, a panic from `work` is propagated to the awaiting code
with its original payload. The pool remains usable for subsequent calls.
If the wait has already been cancelled, there is no caller to propagate the
panic to; pool capacity is released after the work finishes.
With `panic = "abort"`, a panic terminates the process.

Failure to create the compute pool causes a panic during its initialization.
