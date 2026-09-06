//! Command-line argument parsing.

use clap::Parser;

/// The top-level command line. No subcommands exist yet; the
/// surface grows with each command that lands.
#[derive(Debug, Parser)]
#[command(name = "fairway", version, about = env!("CARGO_PKG_DESCRIPTION"))]
pub(crate) struct Cli {}
