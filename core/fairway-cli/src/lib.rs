//! Commands and arguments for Fairway plugins.
//!
//! ```
//! fairway_cli::namespace!(CLI, "hello", "Print a greeting");
//!
//! async fn hello() -> anyhow::Result<()> {
//!     println!("Hello, world!");
//!     Ok(())
//! }
//!
//! fairway_cli::command!(CLI, "Print a greeting", hello);
//! ```
//!
//! A handler may take arguments implementing [`clap::Args`], a
//! [`Shutdown`], both in that order, or neither. Its future must be
//! `Send` and return `anyhow::Result<()>`.
//!
//! [`Cli::parse`] returns a [`PreparedCommand`] to the application.
//! Fairway owns the runtime, signal handling, and process lifecycle.
//! Plugins only need [`namespace!`], [`command!`], and optionally [`Shutdown`].

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

use std::ffi::OsString;

use tokio_util::sync::CancellationToken;

pub use handler::{PreparedCommand, Threading};
pub use registry::Namespace;

// Paths used by macro expansions in plugin crates, not a separate API.
#[doc(hidden)]
pub mod __private {
    pub use crate::handler::{augment, prepare};
    pub use crate::registry::{Command, CommandKind, FAIRWAY_CLI_COMMANDS, FAIRWAY_CLI_NAMESPACES};
    pub use clap;
    pub use linkme;
}

/// An observer of Fairway's shutdown request.
///
/// Fairway handles termination signals and enforces its shutdown
/// deadline whether or not a handler accepts this argument. Use the
/// handle when the command needs to finish requests or save state.
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

/// The command-line parser used by the Fairway application.
///
/// The application supplies its name, version, and description in
/// `root`. Registered plugins supply the namespaces and commands.
pub struct Cli {
    root: clap::Command,
}

impl Cli {
    /// Creates a parser using the application's metadata and linked commands.
    #[must_use]
    pub fn new(root: clap::Command) -> Self {
        Self { root }
    }

    /// Checks the linked namespaces, commands, and clap argument definitions.
    ///
    /// Call this in an application test with the same plugins and features
    /// as the shipped application. Use the default test profile: clap's
    /// argument checks require debug assertions. It does not run command
    /// handlers or worker selectors. `cargo build` alone does not run this check.
    ///
    /// # Panics
    ///
    /// Panics on duplicate names or incompatible registrations, and on
    /// invalid clap argument definitions when debug assertions are enabled.
    /// Registration conflicts include both declaration locations.
    pub fn assert_valid(self) {
        registry::assert_valid();
        parse::tree(self.root, &parse::grouped()).debug_assert();
    }

    /// Parses `args` and prepares the selected command for the application.
    ///
    /// `args` includes the executable name, as in `std::env::args_os()`.
    /// The command contains the parsed arguments and chosen thread count.
    /// The handler runs when the application awaits [`PreparedCommand::run`].
    ///
    /// Help, version, and invalid arguments return a [`clap::Error`]. The
    /// application uses its diagnostic and exit code to report the result.
    pub fn parse(
        self,
        args: impl IntoIterator<Item = OsString>,
    ) -> Result<PreparedCommand, clap::Error> {
        parse::parse(self.root, args)
    }
}
