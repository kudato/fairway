//! Type-checked adapters for handlers with a shutdown observer.
#![allow(
    missing_docs,
    unreachable_pub,
    reason = "helpers are used by downstream macro expansions"
)]

use std::future::Future;
use std::pin::Pin;

use clap::{ArgMatches, Args};

use crate::Shutdown;

type CommandFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>>>>;
type Start = Box<dyn FnOnce(Shutdown) -> CommandFuture>;

/// The execution threads requested by a command.
///
/// Fairway creates the runtime after parsing the command's arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Threading {
    /// Run on the calling thread, the default without `workers`.
    CurrentThread,
    /// Run with a pool of worker threads.
    MultiThread {
        /// Worker count; zero asks Fairway to use the available parallelism.
        workers: usize,
    },
}

/// A selected handler with its parsed arguments and execution requirements.
#[must_use = "the application must run the prepared command"]
pub struct PreparedCommand {
    threading: Threading,
    start: Start,
}

impl PreparedCommand {
    /// Returns the thread policy selected from the command declaration and arguments.
    #[must_use]
    pub fn threading(&self) -> Threading {
        self.threading
    }

    /// Calls the handler within the application's runtime.
    ///
    /// The application supplies the shutdown observer and handles the result.
    /// The handler is called when this future is polled.
    pub async fn run(self, shutdown: Shutdown) -> anyhow::Result<()> {
        (self.start)(shutdown).await
    }
}

// Distinct markers avoid overlapping Fn implementations. Rust infers
// the marker from the handler's signature.
pub struct ShutdownOnly;
pub struct ArgumentsAndShutdown;

/// The built-in action that displays a namespace's full help.
///
/// Register this value with [`command!`](crate::command) to describe a group
/// of subcommands. Invoking the namespace displays help and exits successfully,
/// before Fairway loads configuration or creates a runtime.
///
/// ```
/// fairway_cli::namespace!(CLI, "text");
/// fairway_cli::command!(CLI, "Text commands", fairway_cli::help,
///     long_about = "Commands for working with text.");
/// ```
#[derive(Clone, Copy)]
pub struct Help;

pub enum Action {
    Run(PreparedCommand),
    Help,
}

pub trait Handler<Signature>: 'static {
    type Args: 'static;
    const IS_HELP: bool = false;

    fn augment(command: clap::Command) -> clap::Command;
    fn prepare<W, Selection>(
        self,
        matches: &mut ArgMatches,
        workers: W,
    ) -> Result<Action, clap::Error>
    where
        W: Workers<Self::Args, Selection>;
}

impl<F, Fut> Handler<ShutdownOnly> for F
where
    F: FnOnce(Shutdown) -> Fut + 'static,
    Fut: Future<Output = anyhow::Result<()>> + 'static,
{
    type Args = ();

    fn augment(command: clap::Command) -> clap::Command {
        command
    }

    fn prepare<W, Selection>(self, _: &mut ArgMatches, workers: W) -> Result<Action, clap::Error>
    where
        W: Workers<(), Selection>,
    {
        Ok(Action::Run(PreparedCommand {
            threading: workers.threading(&()),
            start: Box::new(move |shutdown| Box::pin(self(shutdown))),
        }))
    }
}

impl<T, F, Fut> Handler<(ArgumentsAndShutdown, T)> for F
where
    T: Args + 'static,
    F: FnOnce(T, Shutdown) -> Fut + 'static,
    Fut: Future<Output = anyhow::Result<()>> + 'static,
{
    type Args = T;

    fn augment(command: clap::Command) -> clap::Command {
        T::augment_args(command)
    }

    fn prepare<W, Selection>(
        self,
        matches: &mut ArgMatches,
        workers: W,
    ) -> Result<Action, clap::Error>
    where
        W: Workers<T, Selection>,
    {
        let args = T::from_arg_matches_mut(matches)?;
        Ok(Action::Run(PreparedCommand {
            threading: workers.threading(&args),
            start: Box::new(move |shutdown| Box::pin(self(args, shutdown))),
        }))
    }
}

impl Handler<Help> for Help {
    type Args = ();
    const IS_HELP: bool = true;

    fn augment(command: clap::Command) -> clap::Command {
        command
    }

    fn prepare<W, Selection>(self, _: &mut ArgMatches, _: W) -> Result<Action, clap::Error>
    where
        W: Workers<(), Selection>,
    {
        Ok(Action::Help)
    }
}

pub const fn is_help<F, Signature>(_: &F) -> bool
where
    F: Handler<Signature>,
{
    F::IS_HELP
}

pub struct Fixed;
pub struct FromArguments;

pub trait Workers<T, Selection> {
    fn threading(self, args: &T) -> Threading;
}

impl<T> Workers<T, Fixed> for Threading {
    fn threading(self, _: &T) -> Threading {
        self
    }
}

impl<T> Workers<T, Fixed> for usize {
    fn threading(self, _: &T) -> Threading {
        Threading::MultiThread { workers: self }
    }
}

impl<T, F: FnOnce(&T) -> usize> Workers<T, FromArguments> for F {
    fn threading(self, args: &T) -> Threading {
        Threading::MultiThread {
            workers: self(args),
        }
    }
}

pub fn augment<F, Signature>(_: F, command: clap::Command) -> clap::Command
where
    F: Handler<Signature>,
{
    F::augment(command)
}

pub fn prepare<F, Signature, W, Selection>(
    handler: F,
    matches: &mut ArgMatches,
    workers: W,
) -> Result<Action, clap::Error>
where
    F: Handler<Signature>,
    W: Workers<F::Args, Selection>,
{
    handler.prepare(matches, workers)
}
