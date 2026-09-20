//! Type-checked adapters for the four supported handler signatures.
#![allow(
    missing_docs,
    unreachable_pub,
    reason = "helpers are used by downstream macro expansions"
)]

use std::future::Future;
use std::pin::Pin;

use clap::{ArgMatches, Args};

use crate::Shutdown;

type CommandFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;
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
pub struct NoArguments;
pub struct Arguments;
pub struct ShutdownOnly;
pub struct ArgumentsAndShutdown;

pub trait Handler<Signature>: 'static {
    type Args: 'static;

    fn augment(command: clap::Command) -> clap::Command;
    fn parse(matches: &mut ArgMatches) -> Result<Self::Args, clap::Error>;
    fn start(self, args: Self::Args, shutdown: Shutdown) -> CommandFuture;
}

impl<F, Fut> Handler<NoArguments> for F
where
    F: FnOnce() -> Fut + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    type Args = ();

    fn augment(command: clap::Command) -> clap::Command {
        command
    }

    fn parse(_: &mut ArgMatches) -> Result<(), clap::Error> {
        Ok(())
    }

    fn start(self, (): (), _: Shutdown) -> CommandFuture {
        Box::pin(self())
    }
}

impl<T, F, Fut> Handler<(Arguments, T)> for F
where
    T: Args + 'static,
    F: FnOnce(T) -> Fut + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    type Args = T;

    fn augment(command: clap::Command) -> clap::Command {
        T::augment_args(command)
    }

    fn parse(matches: &mut ArgMatches) -> Result<T, clap::Error> {
        T::from_arg_matches_mut(matches)
    }

    fn start(self, args: T, _: Shutdown) -> CommandFuture {
        Box::pin(self(args))
    }
}

impl<F, Fut> Handler<ShutdownOnly> for F
where
    F: FnOnce(Shutdown) -> Fut + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    type Args = ();

    fn augment(command: clap::Command) -> clap::Command {
        command
    }

    fn parse(_: &mut ArgMatches) -> Result<(), clap::Error> {
        Ok(())
    }

    fn start(self, (): (), shutdown: Shutdown) -> CommandFuture {
        Box::pin(self(shutdown))
    }
}

impl<T, F, Fut> Handler<(ArgumentsAndShutdown, T)> for F
where
    T: Args + 'static,
    F: FnOnce(T, Shutdown) -> Fut + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    type Args = T;

    fn augment(command: clap::Command) -> clap::Command {
        T::augment_args(command)
    }

    fn parse(matches: &mut ArgMatches) -> Result<T, clap::Error> {
        T::from_arg_matches_mut(matches)
    }

    fn start(self, args: T, shutdown: Shutdown) -> CommandFuture {
        Box::pin(self(args, shutdown))
    }
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
) -> Result<PreparedCommand, clap::Error>
where
    F: Handler<Signature>,
    W: Workers<F::Args, Selection>,
{
    let args = F::parse(matches)?;
    let threading = workers.threading(&args);
    Ok(PreparedCommand {
        threading,
        start: Box::new(move |shutdown| handler.start(args, shutdown)),
    })
}
