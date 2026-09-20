//! Signal observation and a deadline independent of the command's runtime.

use std::io::{self, Write};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(crate) struct Supervisor {
    shutdown: CancellationToken,
    finished: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Supervisor {
    pub(crate) fn start(timeout: Duration) -> io::Result<Self> {
        let shutdown = CancellationToken::new();
        let request = shutdown.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (finished, mut done) = oneshot::channel();
        let thread = thread::Builder::new()
            .name("fairway-shutdown".into())
            .spawn(move || {
                let setup = (|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    let watcher = {
                        let _entered = runtime.enter();
                        Watcher::install()?
                    };
                    Ok::<_, io::Error>((runtime, watcher))
                })();
                let (runtime, watcher) = match setup {
                    Ok(ready) => ready,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                if ready_tx.send(Ok(())).is_err() {
                    return;
                }
                runtime.block_on(async {
                    let signal = tokio::select! {
                        biased;
                        _ = &mut done => return,
                        signal = watcher.caught() => signal,
                    };
                    let deadline = tokio::time::Instant::now() + timeout;
                    request.cancel();
                    tokio::select! {
                        biased;
                        _ = done => {},
                        _ = tokio::time::sleep_until(deadline) => {
                            force_exit(timeout, signal);
                        }
                    }
                });
            })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                shutdown,
                finished: Some(finished),
                thread: Some(thread),
            }),
            result => {
                let _ = thread.join();
                Err(match result {
                    Ok(Err(error)) => error,
                    Err(_) => io::Error::other("the signal supervisor stopped before installation"),
                    Ok(Ok(())) => unreachable!(),
                })
            }
        }
    }

    pub(crate) fn shutdown(&self) -> CancellationToken {
        self.shutdown.clone()
    }
}

fn force_exit(timeout: Duration, signal: &'static str) -> ! {
    // A command may hold stderr's lock or fill its pipe. Give the
    // diagnostic a bounded opportunity to print without blocking exit.
    let (written, received) = mpsc::sync_channel(1);
    let _ = thread::Builder::new()
        .name("fairway-error".into())
        .spawn(move || {
            let _ = writeln!(
                io::stderr(),
                "error: still running {}s after {signal}",
                timeout.as_secs_f64(),
            );
            let _ = written.send(());
        });
    let _ = received.recv_timeout(Duration::from_millis(10));
    std::process::exit(1);
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        if let Some(finished) = self.finished.take() {
            let _ = finished.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(unix)]
struct Watcher {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Watcher {
    fn install() -> io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            hangup: signal(SignalKind::hangup())?,
        })
    }

    async fn caught(mut self) -> &'static str {
        tokio::select! {
            _ = self.interrupt.recv() => "SIGINT",
            _ = self.terminate.recv() => "SIGTERM",
            _ = self.hangup.recv() => "SIGHUP",
        }
    }
}

#[cfg(windows)]
struct Watcher {
    interrupt: tokio::signal::windows::CtrlC,
}

#[cfg(windows)]
impl Watcher {
    fn install() -> io::Result<Self> {
        Ok(Self {
            interrupt: tokio::signal::windows::ctrl_c()?,
        })
    }

    async fn caught(mut self) -> &'static str {
        self.interrupt.recv().await;
        "Ctrl+C"
    }
}
