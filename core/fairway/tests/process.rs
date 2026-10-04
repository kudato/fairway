//! Observe the Fairway application's runtime, signals, output, and exit codes.

#[path = "../src/app.rs"]
mod app;
#[path = "../src/signal.rs"]
mod signal;

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Barrier};
use std::time::Duration;

#[cfg(unix)]
use std::{
    io::{BufRead, BufReader},
    process::{Child, Output},
    sync::mpsc,
    time::Instant,
};

use fairway_cli::__private::Cli;
use fairway_cli::Shutdown;

fairway_cli::namespace!(CLI, "probe");
fairway_cli::command!(
    CLI,
    "Process tests",
    fairway_cli::help,
    long_about = "Process tests"
);

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    value: u64,
}
fairway_config::namespace!(SETTINGS: Settings, "probe");
fairway_config::namespace!(OTHER: Settings, "other");

async fn settings(_: Shutdown) -> anyhow::Result<()> {
    println!("SETTINGS:{}:{}", SETTINGS.get().value, OTHER.get().value);
    Ok(())
}
fairway_cli::command!(
    CLI,
    "settings",
    "Loaded settings",
    settings,
    long_about = "Loaded settings"
);

async fn flavor(_: Shutdown) -> anyhow::Result<()> {
    let caller = std::rc::Rc::new(std::thread::current().id());
    let task = tokio::spawn(async {
        let handle = tokio::runtime::Handle::current();
        println!(
            "RUNTIME:{:?}:{}",
            handle.runtime_flavor(),
            handle.metrics().num_workers(),
        );
        std::thread::current().id()
    });
    tokio::task::yield_now().await;
    let worker = task.await?;
    assert_eq!(*caller, std::thread::current().id());
    let multi_thread = tokio::runtime::Handle::current().runtime_flavor()
        == tokio::runtime::RuntimeFlavor::MultiThread;
    assert_eq!(*caller != worker, multi_thread);
    println!("HANDLER:{caller:?}");
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
    "pool-single",
    "Arguments without workers",
    pool,
    long_about = "Arguments without workers"
);
fairway_cli::command!(
    CLI,
    "pool",
    "Workers from arguments",
    pool,
    long_about = "Workers from arguments",
    workers = |args: &Pool| args.workers
);

async fn prepared(_: Shutdown) -> anyhow::Result<()> {
    println!("HANDLER");
    Ok(())
}
fairway_cli::command!(
    CLI,
    "prepared",
    "Prepared application",
    prepared,
    long_about = "Prepared application"
);
fairway_cli::command!(
    CLI,
    "preparation-error",
    "Failed preparation",
    prepared,
    long_about = "Failed preparation"
);

async fn error(_: Shutdown) -> anyhow::Result<()> {
    anyhow::bail!("command failed");
}
fairway_cli::command!(
    CLI,
    "error",
    "Handler error",
    error,
    long_about = "Handler error"
);

fn ready() {
    println!("READY");
    std::io::stdout().flush().unwrap();
}

async fn graceful(shutdown: Shutdown) -> anyhow::Result<()> {
    ready();
    shutdown.requested().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    println!("FINISHED");
    Ok(())
}
fairway_cli::command!(
    CLI,
    "graceful",
    "Clean shutdown",
    graceful,
    long_about = "Clean shutdown"
);

async fn failing(shutdown: Shutdown) -> anyhow::Result<()> {
    ready();
    shutdown.requested().await;
    anyhow::bail!("cleanup failed");
}
fairway_cli::command!(
    CLI,
    "failing",
    "Shutdown error",
    failing,
    long_about = "Shutdown error"
);

async fn pending(_: Shutdown) -> anyhow::Result<()> {
    ready();
    std::future::pending().await
}
fairway_cli::command!(
    CLI,
    "pending",
    "Ignored shutdown request",
    pending,
    long_about = "Ignored shutdown request"
);

async fn blocked(_: Shutdown) -> anyhow::Result<()> {
    ready();
    loop {
        std::thread::park();
    }
}
fairway_cli::command!(
    CLI,
    "blocked",
    "Blocked execution thread",
    blocked,
    long_about = "Blocked execution thread"
);

async fn locked(_: Shutdown) -> anyhow::Result<()> {
    let _stderr = std::io::stderr().lock();
    ready();
    loop {
        std::thread::park();
    }
}
fairway_cli::command!(
    CLI,
    "locked",
    "Blocked stderr",
    locked,
    long_about = "Blocked stderr"
);

async fn saturated(_: Shutdown) -> anyhow::Result<()> {
    let barrier = Arc::new(Barrier::new(3));
    for _ in 0..2 {
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait();
            loop {
                std::thread::park();
            }
        });
    }
    barrier.wait();
    ready();
    std::future::pending().await
}
fairway_cli::command!(
    CLI,
    "saturated",
    "Blocked worker pool",
    saturated,
    long_about = "Blocked worker pool",
    workers = 2
);

async fn teardown(_: Shutdown) -> anyhow::Result<()> {
    let barrier = Arc::new(Barrier::new(2));
    let task_barrier = barrier.clone();
    tokio::task::spawn_blocking(move || {
        task_barrier.wait();
        loop {
            std::thread::park();
        }
    });
    barrier.wait();
    ready();
    Ok(())
}
fairway_cli::command!(
    CLI,
    "teardown",
    "Blocked runtime teardown",
    teardown,
    long_about = "Blocked runtime teardown"
);

#[test]
fn child_process() {
    let Ok(argument) = std::env::var("FAIRWAY_TEST_CHILD") else {
        return;
    };
    let mut args = vec!["fairway"];
    if !argument.starts_with('-') {
        args.push("probe");
    }
    args.extend(argument.split_ascii_whitespace());
    println!("CALLER:{:?}", std::thread::current().id());
    let code = app::run(
        Cli::new("fairway", "1.0", "Process tests"),
        args.into_iter().map(Into::into),
        Duration::from_millis(200),
        || {
            println!("PREPARING");
            if argument == "preparation" {
                ready();
                loop {
                    std::thread::park();
                }
            }
            if argument == "preparation-error" {
                anyhow::bail!("preparation failed");
            }
            Ok(())
        },
    );
    std::process::exit(if code == std::process::ExitCode::SUCCESS {
        0
    } else if code == std::process::ExitCode::from(2) {
        2
    } else {
        1
    });
}

async fn preparation(_: Shutdown) -> anyhow::Result<()> {
    unreachable!("the application preparation is blocked");
}
fairway_cli::command!(
    CLI,
    "preparation",
    "Blocked application preparation",
    preparation,
    long_about = "Blocked application preparation"
);

fn child(argument: &str, home: &std::path::Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "child_process", "--nocapture"])
        .env("FAIRWAY_TEST_CHILD", argument)
        .env("FAIRWAY_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[test]
fn help_and_usage_errors_use_the_correct_stream_and_exit_code() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.toml"), "invalid = [").unwrap();
    for (argument, code) in [("", 0), ("--help", 0), ("--version", 0), ("--unknown", 2)] {
        let output = child(argument, home.path()).output().unwrap();
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains("PREPARING"), "{stdout}");
        if code == 0 {
            assert!(stderr.is_empty(), "{stderr}");
            assert!(stdout.contains("fairway"), "{stdout}");
        } else {
            assert!(stderr.contains("--unknown"), "{stderr}");
            assert!(!stdout.contains("Usage:"), "{stdout}");
        }
    }
}

#[test]
fn application_runs_non_send_handlers_with_the_requested_runtime() {
    let home = tempfile::tempdir().unwrap();
    let available = std::thread::available_parallelism().map_or(1, usize::from);
    for (argument, expected) in [
        ("single", "CurrentThread:1".to_owned()),
        ("fixed", "MultiThread:2".to_owned()),
        ("available", format!("MultiThread:{available}")),
        ("pool-single", "CurrentThread:1".to_owned()),
        ("pool", "MultiThread:3".to_owned()),
        ("pool --workers 4", "MultiThread:4".to_owned()),
    ] {
        let output = child(argument, home.path()).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(&format!("RUNTIME:{expected}")), "{stdout}");
        let caller = stdout
            .lines()
            .find_map(|line| line.strip_prefix("CALLER:"))
            .expect("missing caller thread");
        let handler = stdout
            .lines()
            .find_map(|line| line.strip_prefix("HANDLER:"))
            .expect("missing handler thread");
        assert_eq!(caller, handler, "{stdout}");
    }
}

#[test]
fn application_prepares_once_and_reports_errors() {
    let home = tempfile::tempdir().unwrap();
    for (argument, code, handler_runs, diagnostic) in [
        ("prepared", 0, 1, ""),
        ("preparation-error", 1, 0, "error: preparation failed"),
        ("error", 1, 0, "error: command failed"),
    ] {
        let output = child(argument, home.path()).output().unwrap();
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(stdout.matches("PREPARING").count(), 1, "{stdout}");
        assert_eq!(stdout.matches("HANDLER").count(), handler_runs, "{stdout}");
        if code == 0 {
            assert!(stderr.is_empty(), "{stderr}");
        } else {
            assert!(stderr.contains(diagnostic), "{stderr}");
        }
    }
}

#[test]
fn configuration_precedes_handlers_and_errors_leave_help_available() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("conf.d/nested")).unwrap();
    std::fs::write(home.path().join("config.toml"), "[probe]\nvalue = 1\n").unwrap();
    std::fs::write(
        home.path().join("conf.d/nested/probe.toml"),
        "[probe]\nvalue = 2\n",
    )
    .unwrap();
    let output = child("settings", home.path()).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("SETTINGS:2:0"));

    // Even an unused namespace must be valid before the selected handler runs.
    for (settings, expected) in [
        (
            "[other]\nvalue = 'invalid'\n",
            "invalid settings in namespace \"other\"",
        ),
        (
            "[othre]\nvalue = 1\n",
            "unknown configuration namespace \"othre\"",
        ),
    ] {
        std::fs::write(home.path().join("conf.d/other.toml"), settings).unwrap();
        let output = child("settings", home.path()).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("SETTINGS:"));
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostic.contains("other.toml") && diagnostic.contains(expected),
            "{diagnostic}"
        );
        for (argument, code) in [("--help", 0), ("--version", 0), ("--unknown", 2)] {
            let output = child(argument, home.path()).output().unwrap();
            assert_eq!(output.status.code(), Some(code), "{output:?}");
            assert!(!String::from_utf8_lossy(&output.stderr).contains("other.toml"));
        }
    }
}

// Always reap children, including when an assertion or readiness wait fails.
#[cfg(unix)]
struct Running(Option<Child>);

#[cfg(unix)]
impl Drop for Running {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(unix)]
fn signalled(argument: &str, signal: &str) -> (Output, Duration, String) {
    let home = tempfile::tempdir().unwrap();
    let mut running = Running(Some(child(argument, home.path()).spawn().unwrap()));
    let child = running.0.as_mut().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        for line in BufReader::new(stdout).lines() {
            let line = line.unwrap();
            if line == "READY" {
                let _ = ready_tx.send(());
            }
            text.push_str(&line);
            text.push('\n');
        }
        text
    });
    ready_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("child must become ready");
    let start = Instant::now();
    assert!(
        Command::new("kill")
            .args(["-s", signal, &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    while child.try_wait().unwrap().is_none() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{argument} ignored the deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let elapsed = start.elapsed();
    let output = running.0.take().unwrap().wait_with_output().unwrap();
    (output, elapsed, reader.join().unwrap())
}

#[cfg(unix)]
#[test]
fn termination_signals_allow_cooperative_cleanup() {
    for signal in ["INT", "TERM", "HUP"] {
        let (output, _, stdout) = signalled("graceful", signal);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert!(stdout.contains("FINISHED"), "{stdout}");
    }
    let (output, _, _) = signalled("failing", "TERM");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("error: cleanup failed"));
}

#[cfg(unix)]
#[test]
fn deadline_survives_ignored_shutdown_blocked_threads_and_runtime_teardown() {
    for command in [
        "pending",
        "blocked",
        "saturated",
        "teardown",
        "preparation",
        "locked",
    ] {
        let (output, elapsed, _) = signalled(command, "TERM");
        assert_eq!(output.status.code(), Some(1), "{command}: {output:?}");
        assert!(
            elapsed >= Duration::from_millis(150),
            "{command}: {elapsed:?}"
        );
        if command != "locked" {
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(
                error.contains("still running 0.2s after SIGTERM"),
                "{command}: {error}"
            );
        }
    }
}
