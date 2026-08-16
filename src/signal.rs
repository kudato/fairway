//! Termination signals, turned into a future.

use std::io;

/// Resolves when the process receives a termination request: SIGINT,
/// SIGTERM, or SIGHUP. The resolved value names the signal for
/// diagnostics. Handlers are installed on the first poll; SIGKILL is
/// uncatchable and stays outside the exit-code registry.
#[cfg(unix)]
pub(crate) async fn termination() -> io::Result<&'static str> {
    Ok(Watcher::install()?.caught().await)
}

/// Resolves when the process receives Ctrl+C or a console event the
/// platform maps to it.
#[cfg(not(unix))]
pub(crate) async fn termination() -> io::Result<&'static str> {
    tokio::signal::ctrl_c().await?;
    Ok("Ctrl+C")
}

/// The installed handlers. Split from [`termination`] so tests can
/// install before signalling themselves: installation is the moment
/// the disposition flips from lethal to caught.
#[cfg(unix)]
struct Watcher {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Watcher {
    /// Install the OS handlers for the three termination signals.
    fn install() -> io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            hangup: signal(SignalKind::hangup())?,
        })
    }

    /// Wait for the first caught signal.
    async fn caught(mut self) -> &'static str {
        tokio::select! {
            _ = self.interrupt.recv() => "SIGINT",
            _ = self.terminate.recv() => "SIGTERM",
            _ = self.hangup.recv() => "SIGHUP",
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A real signal must resolve the watcher. The test process
    /// signals itself; installing first keeps the default lethal
    /// disposition from ever firing. SIGHUP, so that SIGINT and
    /// SIGTERM stay free for the harness running the tests.
    #[tokio::test]
    async fn a_signal_resolves_the_watcher() {
        let watcher = Watcher::install().unwrap();
        let sent = std::process::Command::new("kill")
            .args(["-s", "HUP", &std::process::id().to_string()])
            .status()
            .unwrap();
        assert!(sent.success(), "kill must deliver the signal");
        let name = tokio::time::timeout(std::time::Duration::from_secs(5), watcher.caught())
            .await
            .expect("the signal must arrive");
        assert_eq!(name, "SIGHUP");
    }
}
