//! Handler signatures, argument parsing, and command preparation.

use std::ffi::OsString;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use fairway_cli::__private::{PreparedCommand, assert_valid};
use fairway_cli::{Shutdown, help as namespace_help};

fairway_cli::namespace!(CLI, "gamma");
fairway_cli::command!(
    CLI,
    "Lorem ipsum",
    namespace_help,
    long_about = "Dolor sit amet.\n\nConsectetur adipiscing elit."
);
fairway_cli::namespace!(BETA, "beta");

static OUTPUT: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn note(value: impl Into<String>) {
    OUTPUT.lock().unwrap().push(value.into());
}

fn recorded(value: &str) -> bool {
    OUTPUT.lock().unwrap().iter().any(|item| item == value)
}

async fn beta(_: Shutdown) -> anyhow::Result<()> {
    note("beta");
    Ok(())
}
fairway_cli::command!(
    BETA,
    "Sed do eiusmod",
    beta,
    long_about = "Tempor incididunt.\n\nUt labore et dolore magna."
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
struct Words {
    /// Aliquip ex ea.
    word: String,
    /// Commodo consequat.
    #[arg(long, default_value_t = 2)]
    count: usize,
}

async fn foo(args: Words, _: Shutdown) -> anyhow::Result<()> {
    note(args.word.repeat(args.count));
    Ok(())
}
fairway_cli::command!(
    CLI,
    "foo",
    "Aliqua enim",
    foo,
    long_about = "Minim veniam quis.\n\nExample: fairway gamma foo lorem --count 3"
);

fairway_cli::namespace!(ALPHA, "alpha");
fairway_cli::command!(
    ALPHA,
    "Nostrud exercitation",
    foo,
    long_about = "Ullamco laboris nisi.\n\nExample: fairway alpha lorem --count 3"
);

async fn baz(shutdown: Shutdown) -> anyhow::Result<()> {
    assert!(shutdown.is_requested());
    shutdown.requested().await;
    note("observed");
    Ok(())
}
fairway_cli::command!(
    CLI,
    "baz",
    "Observe shutdown",
    baz,
    long_about = "Observe shutdown"
);

async fn bar(args: Words, shutdown: Shutdown) -> anyhow::Result<()> {
    assert!(shutdown.is_requested());
    note(args.word.repeat(args.count));
    Ok(())
}
fairway_cli::command!(
    CLI,
    "bar",
    "Arguments and shutdown",
    bar,
    long_about = "Arguments and shutdown"
);

mod sibling {
    use super::CLI as ALIAS;

    async fn qux(_: fairway_cli::Shutdown) -> anyhow::Result<()> {
        super::note("sibling");
        Ok(())
    }
    fairway_cli::command!(
        ALIAS,
        "qux",
        "Registered from another module",
        qux,
        long_about = "Registered from another module"
    );
}

fn parse(args: &[&str]) -> Result<PreparedCommand, clap::Error> {
    fairway_cli::__private::parse(
        "fairway",
        "1.0",
        "CLI tests",
        args.iter().map(OsString::from),
    )
}

async fn run(args: &[&str]) -> anyhow::Result<()> {
    parse(args)?.run(CancellationToken::new().into()).await
}

#[tokio::test]
async fn commands_receive_positionals_options_and_defaults() {
    run(&["fairway", "beta"]).await.unwrap();
    assert!(recorded("beta"));
    run(&["fairway", "gamma", "foo", "lorem"]).await.unwrap();
    assert!(recorded("loremlorem"));
    run(&["fairway", "gamma", "foo", "a", "--count", "3"])
        .await
        .unwrap();
    assert!(recorded("aaa"));
    run(&["fairway", "gamma", "qux"]).await.unwrap();
    assert!(recorded("sibling"));
    run(&["fairway", "alpha", "alpha", "--count", "1"])
        .await
        .unwrap();
    assert!(recorded("alpha"));
}

#[tokio::test]
async fn both_shutdown_signatures_receive_the_shared_request() {
    let source = CancellationToken::new();
    let observer = Shutdown::from(source.clone());
    assert!(!observer.is_requested());
    source.cancel();
    observer.requested().await;

    for args in [
        vec!["fairway", "gamma", "baz"],
        vec!["fairway", "gamma", "bar", "shutdown", "--count", "1"],
    ] {
        parse(&args).unwrap().run(observer.clone()).await.unwrap();
    }
    assert!(recorded("observed"));
    assert!(recorded("shutdown"));
}

#[test]
fn help_uses_declarations_and_field_comments() {
    assert_valid("fairway", "1.0", "CLI tests");
    let root = parse(&["fairway", "--help"])
        .err()
        .expect("expected help")
        .to_string();
    assert!(root.contains("CLI tests"), "{root}");
    assert!(root.contains("Lorem ipsum"), "{root}");
    assert!(root.contains("Sed do eiusmod"), "{root}");
    // Commands are listed by name, whatever the declaration order or module.
    let gamma = parse(&["fairway", "gamma", "--help"])
        .err()
        .expect("expected help")
        .to_string();
    for (help, names) in [
        (&root, &["alpha", "beta", "gamma"][..]),
        (&gamma, &["bar", "baz", "foo", "qux"][..]),
    ] {
        let positions: Vec<_> = names
            .iter()
            .map(|name| help.find(&format!("\n  {name} ")).expect(name))
            .collect();
        assert!(positions.is_sorted(), "{help}");
    }
    for (args, description, example) in [
        (
            vec!["fairway", "gamma", "foo", "--help"],
            "Minim veniam quis.",
            "Example: fairway gamma foo lorem --count 3",
        ),
        (
            vec!["fairway", "alpha", "--help"],
            "Ullamco laboris nisi.",
            "Example: fairway alpha lorem --count 3",
        ),
    ] {
        let help = parse(&args).err().expect("expected help").to_string();
        assert!(help.starts_with(description), "{help}");
        assert!(help.contains(example), "{help}");
        assert!(help.contains("Commodo consequat"), "{help}");
        assert!(help.contains("--count"), "{help}");
        assert!(help.contains("--help"), "{help}");
        assert!(help.contains("Extra argument help"), "{help}");
        assert!(!help.contains("Ignored argument"), "{help}");
        assert!(!help.contains("Argument type"), "{help}");
    }
    let beta = parse(&["fairway", "beta", "--help"])
        .err()
        .expect("expected help")
        .to_string();
    assert!(
        beta.contains("Tempor incididunt.\n\nUt labore et dolore magna."),
        "{beta}"
    );
}

#[test]
fn namespace_action_shows_full_help_and_short_help_uses_about() {
    let default = parse(&["fairway", "gamma"]).err().expect("expected help");
    let explicit = parse(&["fairway", "gamma", "--help"])
        .err()
        .expect("expected help");
    assert_eq!(default.kind(), clap::error::ErrorKind::DisplayHelp);
    assert_eq!(default.exit_code(), 0);
    assert!(!default.use_stderr());
    assert_eq!(default.to_string(), explicit.to_string());
    assert!(default.to_string().contains("Dolor sit amet."));
    assert!(default.to_string().contains("Usage: fairway gamma"));
    for (args, short, long) in [
        (
            vec!["fairway", "gamma", "-h"],
            "Lorem ipsum",
            "Dolor sit amet.",
        ),
        (
            vec!["fairway", "beta", "-h"],
            "Sed do eiusmod",
            "Tempor incididunt.",
        ),
        (
            vec!["fairway", "gamma", "foo", "-h"],
            "Aliqua enim",
            "Minim veniam quis.",
        ),
        (
            vec!["fairway", "alpha", "-h"],
            "Nostrud exercitation",
            "Ullamco laboris nisi.",
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
        (vec!["fairway", "gamma", "--help"], 0),
        (vec!["fairway", "gamma", "foo", "--help"], 0),
        (vec!["fairway"], 2),
        (vec!["fairway", "gamma"], 0),
        (vec!["fairway", "gamma", "unknown"], 2),
        (vec!["fairway", "gamma", "foo"], 2),
        (vec!["fairway", "gamma", "foo", "x", "--count", "bad"], 2),
        (vec!["fairway", "beta", "--unknown"], 2),
    ] {
        let error = parse(&args).err().expect("expected a clap diagnostic");
        assert_eq!(error.exit_code(), code);
    }
}
