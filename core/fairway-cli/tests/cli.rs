//! Handler signatures, argument parsing, and command preparation.

use std::ffi::OsString;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use fairway_cli::__private::{Cli, PreparedCommand, Threading};
use fairway_cli::Shutdown;

fairway_cli::namespace!(CLI, "text", "Text commands");
fairway_cli::namespace!(HELLO, "hello", "Greeting");

static OUTPUT: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn note(value: impl Into<String>) {
    OUTPUT.lock().unwrap().push(value.into());
}

fn recorded(value: &str) -> bool {
    OUTPUT.lock().unwrap().iter().any(|item| item == value)
}

async fn hello() -> anyhow::Result<()> {
    note("hello");
    Ok(())
}
fairway_cli::command!(HELLO, "Print a greeting", hello);

#[derive(clap::Args)]
struct Repeat {
    /// Text to repeat.
    text: String,
    /// Number of repetitions.
    #[arg(long, default_value_t = 2)]
    times: usize,
}

async fn repeat(args: Repeat) -> anyhow::Result<()> {
    note(args.text.repeat(args.times));
    Ok(())
}
fairway_cli::command!(CLI, "repeat", "Repeat text", repeat);

async fn observe(shutdown: Shutdown) -> anyhow::Result<()> {
    assert!(shutdown.is_requested());
    shutdown.requested().await;
    note("observed");
    Ok(())
}
fairway_cli::command!(CLI, "observe", "Observe shutdown", observe);

async fn both(args: Repeat, shutdown: Shutdown) -> anyhow::Result<()> {
    assert!(shutdown.is_requested());
    note(args.text.repeat(args.times));
    Ok(())
}
fairway_cli::command!(CLI, "both", "Arguments and shutdown", both);

async fn fail() -> anyhow::Result<()> {
    anyhow::bail!("command failed");
}
fairway_cli::command!(CLI, "fail", "Return an error", fail);

async fn flavor() -> anyhow::Result<()> {
    let handle = tokio::runtime::Handle::current();
    note(format!(
        "{:?}:{}",
        handle.runtime_flavor(),
        handle.metrics().num_workers(),
    ));
    Ok(())
}
fairway_cli::command!(CLI, "single", "One execution thread", flavor);
fairway_cli::command!(CLI, "fixed", "Two worker threads", flavor, workers = 2);
fairway_cli::command!(CLI, "available", "Available cores", flavor, workers = 0);

#[derive(clap::Args)]
struct Pool {
    #[arg(long, default_value_t = 3)]
    workers: usize,
}

async fn pool(_: Pool) -> anyhow::Result<()> {
    flavor().await
}
fairway_cli::command!(
    CLI,
    "pool",
    "Workers from arguments",
    pool,
    workers = |args: &Pool| args.workers
);

mod sibling {
    use super::CLI as ALIAS;

    async fn upper() -> anyhow::Result<()> {
        super::note("sibling");
        Ok(())
    }
    fairway_cli::command!(ALIAS, "upper", "Registered from another module", upper);
}

fn root() -> clap::Command {
    clap::Command::new("fairway").version("1.0")
}

fn parse(args: &[&str]) -> Result<PreparedCommand, clap::Error> {
    Cli::new(root()).parse(args.iter().map(OsString::from))
}

async fn run(args: &[&str]) -> anyhow::Result<()> {
    parse(args)?.run(Shutdown::new()).await
}

#[tokio::test]
async fn commands_receive_positionals_options_and_defaults() {
    run(&["fairway", "hello"]).await.unwrap();
    assert!(recorded("hello"));
    run(&["fairway", "text", "repeat", "hi"]).await.unwrap();
    assert!(recorded("hihi"));
    run(&["fairway", "text", "repeat", "a", "--times", "3"])
        .await
        .unwrap();
    assert!(recorded("aaa"));
    run(&["fairway", "text", "upper"]).await.unwrap();
    assert!(recorded("sibling"));
}

#[tokio::test]
async fn both_shutdown_signatures_receive_the_shared_request() {
    let source = CancellationToken::new();
    let observer = Shutdown::from(source.clone());
    assert!(!observer.is_requested());
    source.cancel();
    observer.requested().await;

    for args in [
        vec!["fairway", "text", "observe"],
        vec!["fairway", "text", "both", "shutdown", "--times", "1"],
    ] {
        parse(&args).unwrap().run(observer.clone()).await.unwrap();
    }
    assert!(recorded("observed"));
    assert!(recorded("shutdown"));
}

#[test]
fn threading_is_selected_without_a_running_runtime() {
    assert!(tokio::runtime::Handle::try_current().is_err());
    for (command, expected) in [
        ("single", Threading::CurrentThread),
        ("fixed", Threading::MultiThread { workers: 2 }),
        ("available", Threading::MultiThread { workers: 0 }),
        ("pool", Threading::MultiThread { workers: 3 }),
    ] {
        let prepared = parse(&["fairway", "text", command]).unwrap();
        assert_eq!(prepared.threading(), expected);
    }

    let prepared = parse(&["fairway", "text", "pool", "--workers", "4"]).unwrap();
    assert_eq!(prepared.threading(), Threading::MultiThread { workers: 4 });
}

#[tokio::test]
async fn handler_uses_the_callers_runtime_and_returns_its_error() {
    // The request for two workers is metadata. The application owns the
    // runtime; awaiting this command uses our existing current-thread runtime.
    run(&["fairway", "text", "fixed"]).await.unwrap();
    assert!(recorded("CurrentThread:1"));
    let error = run(&["fairway", "text", "fail"]).await.unwrap_err();
    assert_eq!(error.to_string(), "command failed");
}

#[test]
fn help_uses_declarations_and_field_comments() {
    Cli::new(root()).assert_valid();
    let help = parse(&["fairway", "text", "repeat", "--help"])
        .err()
        .expect("expected help")
        .to_string();
    assert!(help.contains("Repeat text"), "{help}");
    assert!(help.contains("Number of repetitions"), "{help}");
    assert!(help.contains("--times"), "{help}");
    assert!(help.contains("--help"), "{help}");
    let hello = parse(&["fairway", "hello", "--help"])
        .err()
        .expect("expected help")
        .to_string();
    assert!(hello.contains("Greeting\n\nPrint a greeting"), "{hello}");
}

#[test]
fn help_version_and_invalid_arguments_are_returned_to_the_application() {
    for (args, code) in [
        (vec!["fairway", "--help"], 0),
        (vec!["fairway", "--version"], 0),
        (vec!["fairway", "text", "--help"], 0),
        (vec!["fairway", "text", "repeat", "--help"], 0),
        (vec!["fairway"], 2),
        (vec!["fairway", "text"], 2),
        (vec!["fairway", "text", "repeat"], 2),
        (vec!["fairway", "text", "repeat", "x", "--times", "bad"], 2),
        (vec!["fairway", "hello", "--unknown"], 2),
    ] {
        let error = parse(&args).err().expect("expected a clap diagnostic");
        assert_eq!(error.exit_code(), code);
    }
}
