//! Commands and arguments for Fairway plugins.
//!
//! ```
//! use fairway_cli::Shutdown;
//!
//! fairway_cli::namespace!(CLI, "hello", "Print a greeting");
//!
//! async fn hello(shutdown: Shutdown) -> anyhow::Result<()> {
//!     if shutdown.is_requested() {
//!         return Ok(());
//!     }
//!     println!("Hello, world!");
//!     Ok(())
//! }
//!
//! fairway_cli::command!(CLI, "Print a greeting", hello);
//! ```
//!
//! A handler takes a [`Shutdown`], optionally preceded by arguments
//! implementing [`clap::Args`]. Its future must be `Send` and return
//! `anyhow::Result<()>`.
//!
//! Fairway owns the runtime, signal handling, and process lifecycle.
//! Plugins use [`namespace!`], [`command!`], and [`Shutdown`].

mod handler;
mod macros;
mod parse;
mod registry;

#[cfg(doctest)]
#[doc = include_str!("../../../docs/ru/plugin-development/cli.md")]
mod guide_ru {}

#[cfg(doctest)]
#[doc = include_str!("../../../docs/en/plugin-development/cli.md")]
mod guide_en {}

use tokio_util::sync::CancellationToken;

pub use registry::Namespace;

// Paths used by Fairway and macro expansions in plugin crates.
#[doc(hidden)]
pub mod __private {
    pub use crate::handler::{PreparedCommand, Threading, augment, prepare};
    pub use crate::parse::Cli;
    pub use crate::registry::{Command, CommandKind, FAIRWAY_CLI_COMMANDS, FAIRWAY_CLI_NAMESPACES};
    pub use clap;
    pub use linkme;
}

/// An observer of Fairway's shutdown request.
///
/// Every command handler receives this observer and must handle its shutdown
/// request. Fairway handles termination signals and enforces its shutdown
/// deadline.
#[derive(Debug, Clone, Default)]
pub struct Shutdown {
    token: CancellationToken,
}

impl Shutdown {
    /// Creates an observer with no source of shutdown requests, for tests.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether Fairway has requested shutdown.
    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Waits for shutdown; returns immediately if it was already requested.
    pub async fn requested(&self) {
        self.token.cancelled().await;
    }
}

impl From<CancellationToken> for Shutdown {
    /// Observes the shutdown token controlled by the application.
    fn from(token: CancellationToken) -> Self {
        Self { token }
    }
}
