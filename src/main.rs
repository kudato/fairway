//! The fairway command-line entry point.

mod cli;
mod commands;
mod signal;

use std::io::Write;
use std::process::ExitCode;

use anyhow::Context;
use fairway::verdict::MALFUNCTION;

fn main() -> ExitCode {
    // A panic would bypass the exit-code registry with 101; caught,
    // it reports as a malfunction like any other breakage. Not
    // hypothetical: tokio's signal driver panics on fd exhaustion
    // instead of returning an error.
    match std::panic::catch_unwind(run_to_completion) {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(e)) => malfunction(&format!("{e:#}")),
        // The default panic hook has already put the cause on stderr.
        Err(_) => malfunction("panicked; details above"),
    }
}

/// Build the runtime and drive [`run`] on it. By hand, because
/// `#[tokio::main]` panics when the runtime cannot be built.
fn run_to_completion() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("could not start the async runtime")?;
    runtime.block_on(run_or_interrupted())
}

/// Drive [`run`] against the termination watcher: the first to finish
/// decides the outcome. A caught signal means no verdict was reached,
/// so it reports through the malfunction path.
async fn run_or_interrupted() -> anyhow::Result<()> {
    tokio::select! {
        result = run() => result,
        caught = signal::termination() => {
            let name = caught.context("could not install the signal handlers")?;
            anyhow::bail!("terminated by {name}")
        }
    }
}

/// Report fairway's own breakage on both channels and produce the
/// reserved exit code.
fn malfunction(cause: &str) -> ExitCode {
    // A dead stream cannot make the malfunction being reported any
    // worse; write errors are ignored.
    let mut stdout = std::io::stdout();
    let _ = writeln!(
        stdout,
        "fairway failed: stop and report the error output to the user."
    )
    .and_then(|()| stdout.flush());
    let _ = writeln!(std::io::stderr(), "fairway: {cause}");
    ExitCode::from(MALFUNCTION)
}

async fn run() -> anyhow::Result<()> {
    // println! would panic on a dead stdout; `?` routes the failure
    // through the malfunction arm of main() instead.
    let mut stdout = std::io::stdout();
    writeln!(stdout, "fairway {}", env!("CARGO_PKG_VERSION"))?;
    stdout.flush()?;
    Ok(())
}
