//! Concurrent registry initialization must preserve per-call clap configuration.

use std::cell::Cell;
use std::ffi::OsString;
use std::sync::Barrier;

use fairway_cli::__private::{Cli, PreparedCommand, Threading};

fairway_cli::namespace!(FIRST, "first", "First namespace");
fairway_cli::namespace!(TOOLS, "tools", "Named commands");
fairway_cli::namespace!(LAST, "last", "Last namespace");

thread_local! {
    static OPTION: Cell<&'static str> = const { Cell::new("workers-a") };
    static AUGMENTS: Cell<usize> = const { Cell::new(0) };
    static SELECTED: Cell<usize> = const { Cell::new(0) };
}

fn option_name() -> &'static str {
    AUGMENTS.set(AUGMENTS.get() + 1);
    OPTION.get()
}

#[derive(clap::Args)]
struct Arguments {
    #[arg(long = option_name(), default_value_t = 1)]
    workers: usize,
}

async fn named(_: Arguments) -> anyhow::Result<()> {
    panic!("parsing must not run handlers");
}

fn workers(args: &Arguments) -> usize {
    SELECTED.set(SELECTED.get() + 1);
    args.workers
}

fairway_cli::command!(TOOLS, "alpha", "First command", named, workers = workers);
fairway_cli::command!(TOOLS, "middle", "Middle command", named, workers = workers);
fairway_cli::command!(TOOLS, "zulu", "Last command", named, workers = workers);

async fn own() -> anyhow::Result<()> {
    panic!("parsing must not run handlers");
}
fairway_cli::command!(FIRST, "First handler", own, workers = 2);
fairway_cli::command!(LAST, "Last handler", own, workers = 3);

fn parse(
    name: &'static str,
    version: &'static str,
    args: &[&str],
) -> Result<PreparedCommand, clap::Error> {
    let before = AUGMENTS.get();
    let result =
        Cli::new(clap::Command::new(name).version(version)).parse(args.iter().map(OsString::from));
    assert_eq!(
        AUGMENTS.get(),
        before + 3,
        "each parse must rebuild arguments"
    );
    result
}

// Keep this as the sole test in its binary: all threads race the first use
// of the registry, independently of tests that already parsed arguments.
#[test]
fn concurrent_parses_keep_roots_and_argument_callbacks_independent() {
    let barrier = Barrier::new(8);
    std::thread::scope(|scope| {
        for worker in 0..8 {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for round in 0..2 {
                    let (name, version, option, previous) = if (worker + round) % 2 == 0 {
                        ("first-app", "1.0", "workers-a", "--workers-b")
                    } else {
                        ("second-app", "2.0", "workers-b", "--workers-a")
                    };
                    OPTION.set(option);
                    let flag = format!("--{option}");

                    for command in ["alpha", "middle", "zulu"] {
                        let before = SELECTED.get();
                        let prepared =
                            parse(name, version, &[name, "tools", command, &flag, "5"]).unwrap();
                        assert_eq!(prepared.threading(), Threading::MultiThread { workers: 5 });
                        assert_eq!(SELECTED.get(), before + 1);
                    }
                    let before = SELECTED.get();
                    for (namespace, count) in [("first", 2), ("last", 3)] {
                        let prepared = parse(name, version, &[name, namespace]).unwrap();
                        assert_eq!(
                            prepared.threading(),
                            Threading::MultiThread { workers: count }
                        );
                    }

                    let version_output = parse(name, version, &[name, "--version"])
                        .err()
                        .expect("expected version");
                    assert_eq!(version_output.exit_code(), 0);
                    assert_eq!(version_output.to_string(), format!("{name} {version}\n"));

                    let help = parse(name, version, &[name, "tools", "alpha", "--help"])
                        .err()
                        .expect("expected help");
                    assert_eq!(help.exit_code(), 0);
                    let help = help.to_string();
                    assert!(help.contains(name), "{help}");
                    assert!(help.contains(&flag), "{help}");
                    assert!(!help.contains(previous), "{help}");

                    let invalid = parse(name, version, &[name, "tools", "alpha", previous, "5"])
                        .err()
                        .expect("previous call's option must be rejected");
                    assert_eq!(invalid.exit_code(), 2);
                    assert_eq!(
                        SELECTED.get(),
                        before,
                        "help and errors must not prepare handlers"
                    );
                }
            });
        }
    });
}
