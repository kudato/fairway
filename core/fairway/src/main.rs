//! The Fairway application: startup, runtime, and process lifecycle.
//!
//! Core libraries provide their own APIs; Fairway coordinates their execution.

mod app;
mod signal;

use std::process::ExitCode;
use std::time::Duration;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

fn main() -> ExitCode {
    app::run(cli(), std::env::args_os(), SHUTDOWN_TIMEOUT, || Ok(()))
}

fn cli() -> fairway_cli::Cli {
    let root = clap::Command::new("fairway")
        .version(env!("CARGO_PKG_VERSION"))
        .about(env!("CARGO_PKG_DESCRIPTION"));
    fairway_cli::Cli::new(root)
}

#[cfg(test)]
mod tests {
    #[test]
    fn cli_registry_is_valid() {
        super::cli().assert_valid();
    }
}
