//! The fairway command-line entry point.

mod cli;
mod commands;
mod signal;

use std::io::Write;
use std::process::ExitCode;

use anyhow::Context;
use clap::error::ErrorKind;
use clap::{CommandFactory, Parser};
use fairway::verdict::{MALFUNCTION, Verdict};

/// The instruction to an agent whose invocation could not be
/// parsed; the parser's diagnostics go to stderr.
const INVALID_INVOCATION: &str =
    "The invocation is invalid. Correct the command line against the usage on stderr and retry.";

fn main() -> ExitCode {
    // A panic would bypass the exit-code registry with 101; caught,
    // it reports as a malfunction like any other breakage. Not
    // hypothetical: tokio's signal driver panics on fd exhaustion
    // instead of returning an error.
    match std::panic::catch_unwind(run_to_completion) {
        Ok(Ok(code)) => code,
        Ok(Err(e)) => malfunction(&format!("{e:#}")),
        // The default panic hook has already put the cause on stderr.
        Err(_) => malfunction("panicked; details above"),
    }
}

/// Build the runtime and drive [`run`] on it. By hand, because
/// `#[tokio::main]` panics when the runtime cannot be built.
fn run_to_completion() -> anyhow::Result<ExitCode> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("could not start the async runtime")?;
    runtime.block_on(run_or_interrupted())
}

/// Drive [`run`] against the termination watcher: the first to finish
/// decides the outcome. A caught signal means no verdict was reached,
/// so it reports through the malfunction path.
async fn run_or_interrupted() -> anyhow::Result<ExitCode> {
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

async fn run() -> anyhow::Result<ExitCode> {
    // println! would panic on a dead stdout; write! returns the
    // error instead, and `?` routes it through the malfunction arm
    // of main().
    match cli::Cli::try_parse() {
        Ok(cli::Cli {}) => {
            // No subcommands exist yet; the useful answer to a bare
            // invocation is the usage itself.
            let mut stdout = std::io::stdout();
            write!(stdout, "{}", cli::Cli::command().render_help())?;
            stdout.flush()?;
            Ok(ExitCode::SUCCESS)
        }
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) => {
            let mut stdout = std::io::stdout();
            write!(stdout, "{e}")?;
            stdout.flush()?;
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            // The parser's diagnostics are for the human; the agent
            // gets the instruction and the Adjust code.
            let _ = write!(std::io::stderr(), "{e}");
            Ok(Verdict::Adjust(INVALID_INVOCATION.to_owned()).render())
        }
    }
}
