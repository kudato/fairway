//! The Fairway application: startup, runtime, and process lifecycle.
//!
//! Core libraries provide their own APIs; Fairway coordinates their execution.

mod app;
mod plugins;
mod signal;

use std::process::ExitCode;
use std::time::Duration;

use fairway_cli::__private::parse;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

fn main() -> ExitCode {
    let command = parse(
        "fairway",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_DESCRIPTION"),
        std::env::args_os(),
    );
    app::run(command, SHUTDOWN_TIMEOUT, || Ok(()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn cli_registry_is_valid() {
        fairway_cli::__private::assert_valid(
            "fairway",
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_DESCRIPTION"),
        );
    }

    #[test]
    fn config_registry_is_valid() {
        fairway_config::validate_registry();
    }

    #[test]
    fn skill_registry_is_valid() {
        fairway_skill::validate_registry();
    }

    #[test]
    fn catalog_registry_is_valid() {
        if let Err(error) = fairway_catalog::initialize() {
            panic!("{error}");
        }
    }
}
