//! Handler signatures, argument parsing, and command preparation.

use std::ffi::OsString;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use fairway_cli::__private::{Cli, PreparedCommand, Threading};
use fairway_cli::{Shutdown, help as namespace_help};

fairway_cli::namespace!(CLI, "text");
fairway_cli::command!(
    CLI,
    "Text commands",
    namespace_help,
    long_about = "Commands for processing text.\n\nChoose a command from the list below."
);
fairway_cli::namespace!(HELLO, "hello");

static OUTPUT: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn note(value: impl Into<String>) {
    OUTPUT.lock().unwrap().push(value.into());
}

fn recorded(value: &str) -> bool {
    OUTPUT.lock().unwrap().iter().any(|item| item == value)
}

async fn hello(_: Shutdown) -> anyhow::Result<()> {
    note("hello");
    Ok(())
}
fairway_cli::command!(
    HELLO,
    "Greeting",
    hello,
    long_about = "Print a greeting.\n\nThe greeting is written to standard output."
);

/// Argument type description that must not become the command description.
///
/// Argument type details that must not become the command's full help.
#[derive(clap::Args)]
#[command(
    about = "Ignored argument about",
    long_about = "Ignored argument long_about",
    after_help = "Extra argument help"
)]
struct Repeat {
    /// Text to repeat.
    text: String,
    /// Number of repetitions.
    #[arg(long, default_value_t = 2)]
    times: usize,
}

async fn repeat(args: Repeat, _: Shutdown) -> anyhow::Result<()> {
    note(args.text.repeat(args.times));
    Ok(())
}
fairway_cli::command!(
    CLI,
    "repeat",
    "Repeat text",
    repeat,
    long_about =
        "Repeat text the requested number of times.\n\nExample: fairway text repeat hi --times 3"
);

fairway_cli::namespace!(ECHO, "echo");
fairway_cli::command!(
    ECHO,
    "Echo text",
    repeat,
    long_about = "Print the supplied text repeatedly.\n\nExample: fairway echo hi --times 3"
);

async fn observe(shutdown: Shutdown) -> anyhow::Result<()> {
    assert!(shutdown.is_requested());
    shutdown.requested().await;
    note("observed");
    Ok(())
}
fairway_cli::command!(
    CLI,
    "observe",
    "Observe shutdown",
    observe,
    long_about = "Observe shutdown"
);

async fn both(args: Repeat, shutdown: Shutdown) -> anyhow::Result<()> {
    assert!(shutdown.is_requested());
    note(args.text.repeat(args.times));
    Ok(())
}
fairway_cli::command!(
    CLI,
    "both",
    "Arguments and shutdown",
    both,
    long_about = "Arguments and shutdown"
);

async fn fail(_: Shutdown) -> anyhow::Result<()> {
    anyhow::bail!("command failed");
}
fairway_cli::command!(
    CLI,
    "fail",
    "Return an error",
    fail,
    long_about = "Return an error"
);

async fn flavor(_: Shutdown) -> anyhow::Result<()> {
    let handle = tokio::runtime::Handle::current();
    note(format!(
        "{:?}:{}",
        handle.runtime_flavor(),
        handle.metrics().num_workers(),
    ));
    Ok(())
}
fairway_cli::command!(
    CLI,
    "single",
    "One execution thread",
    flavor,
    long_about = "One execution thread"
);
fairway_cli::command!(
    CLI,
    "fixed",
    "Two worker threads",
    flavor,
    long_about = "Two worker threads",
    workers = 2
);
fairway_cli::command!(
    CLI,
    "available",
    "Available cores",
    flavor,
    long_about = "Available cores",
    workers = 0
);

#[derive(clap::Args)]
struct Pool {
    #[arg(long, default_value_t = 3)]
    workers: usize,
}

async fn pool(_: Pool, shutdown: Shutdown) -> anyhow::Result<()> {
    flavor(shutdown).await
}
fairway_cli::command!(
    CLI,
    "pool",
    "Workers from arguments",
    pool,
    long_about = "Workers from arguments",
    workers = |args: &Pool| args.workers
);

mod sibling {
    use super::CLI as ALIAS;

    async fn upper(_: fairway_cli::Shutdown) -> anyhow::Result<()> {
        super::note("sibling");
        Ok(())
    }
    fairway_cli::command!(
        ALIAS,
        "upper",
        "Registered from another module",
        upper,
        long_about = "Registered from another module"
    );
}

fn cli() -> Cli {
    Cli::new("fairway", "1.0", "CLI tests")
}

fn parse(args: &[&str]) -> Result<PreparedCommand, clap::Error> {
    cli().parse(args.iter().map(OsString::from))
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
    run(&["fairway", "echo", "echo", "--times", "1"])
        .await
        .unwrap();
    assert!(recorded("echo"));
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
    cli().assert_valid();
    let root = parse(&["fairway", "--help"])
        .err()
        .expect("expected help")
        .to_string();
    assert!(root.contains("CLI tests"), "{root}");
    assert!(root.contains("Text commands"), "{root}");
    assert!(root.contains("Greeting"), "{root}");
    for (args, description, example) in [
        (
            vec!["fairway", "text", "repeat", "--help"],
            "Repeat text the requested number of times.",
            "Example: fairway text repeat hi --times 3",
        ),
        (
            vec!["fairway", "echo", "--help"],
            "Print the supplied text repeatedly.",
            "Example: fairway echo hi --times 3",
        ),
    ] {
        let help = parse(&args).err().expect("expected help").to_string();
        assert!(help.starts_with(description), "{help}");
        assert!(help.contains(example), "{help}");
        assert!(help.contains("Number of repetitions"), "{help}");
        assert!(help.contains("--times"), "{help}");
        assert!(help.contains("--help"), "{help}");
        assert!(help.contains("Extra argument help"), "{help}");
        assert!(!help.contains("Ignored argument"), "{help}");
        assert!(!help.contains("Argument type"), "{help}");
    }
    let hello = parse(&["fairway", "hello", "--help"])
        .err()
        .expect("expected help")
        .to_string();
    assert!(
        hello.contains("Print a greeting.\n\nThe greeting is written to standard output."),
        "{hello}"
    );
}

#[test]
fn namespace_action_shows_full_help_and_short_help_uses_about() {
    let default = parse(&["fairway", "text"]).err().expect("expected help");
    let explicit = parse(&["fairway", "text", "--help"])
        .err()
        .expect("expected help");
    assert_eq!(default.kind(), clap::error::ErrorKind::DisplayHelp);
    assert_eq!(default.exit_code(), 0);
    assert!(!default.use_stderr());
    assert_eq!(default.to_string(), explicit.to_string());
    assert!(
        default
            .to_string()
            .contains("Commands for processing text.")
    );
    assert!(default.to_string().contains("Usage: fairway text"));
    for (args, short, long) in [
        (
            vec!["fairway", "text", "-h"],
            "Text commands",
            "Commands for processing text.",
        ),
        (
            vec!["fairway", "hello", "-h"],
            "Greeting",
            "Print a greeting.",
        ),
        (
            vec!["fairway", "text", "repeat", "-h"],
            "Repeat text",
            "Repeat text the requested number of times.",
        ),
        (
            vec!["fairway", "echo", "-h"],
            "Echo text",
            "Print the supplied text repeatedly.",
        ),
    ] {
        let help = parse(&args).err().expect("expected help").to_string();
        assert!(help.starts_with(short), "{help}");
        assert!(!help.contains(long), "{help}");
    }
}

#[test]
fn help_version_and_invalid_arguments_are_returned_to_the_application() {
    for (args, code) in [
        (vec!["fairway", "--help"], 0),
        (vec!["fairway", "--version"], 0),
        (vec!["fairway", "text", "--help"], 0),
        (vec!["fairway", "text", "repeat", "--help"], 0),
        (vec!["fairway"], 2),
        (vec!["fairway", "text"], 0),
        (vec!["fairway", "text", "unknown"], 2),
        (vec!["fairway", "text", "repeat"], 2),
        (vec!["fairway", "text", "repeat", "x", "--times", "bad"], 2),
        (vec!["fairway", "hello", "--unknown"], 2),
    ] {
        let error = parse(&args).err().expect("expected a clap diagnostic");
        assert_eq!(error.exit_code(), code);
    }
}
