//! Pins on the compiled binary, the way an agent meets it: a real
//! process with real streams, exit codes observed from outside.

use std::process::Command;

use tempfile::TempDir;

/// The binary under test, located by cargo, with its home pinned
/// inside a per-test directory: an e2e run must never read or
/// write the real `~/.fairway`.
fn fairway(home: &TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fairway"));
    command.env("FAIRWAY_HOME", home.path());
    command
}

#[test]
fn a_bare_invocation_answers_with_the_usage() {
    let home = TempDir::new().unwrap();
    let out = fairway(&home).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("Usage: fairway"), "{stdout:?}");
}

#[test]
fn the_help_flag_answers_with_the_usage() {
    let home = TempDir::new().unwrap();
    let out = fairway(&home).arg("--help").output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("Usage: fairway"), "{stdout:?}");
}

#[test]
fn the_version_flag_reports_the_version() {
    let home = TempDir::new().unwrap();
    let out = fairway(&home).arg("--version").output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout, concat!("fairway ", env!("CARGO_PKG_VERSION"), "\n"));
}

/// A misuse the parser refuses: the agent is told to adjust, the
/// human diagnostics go to stderr.
#[test]
fn an_unparsed_invocation_is_an_adjust() {
    let home = TempDir::new().unwrap();
    let out = fairway(&home).arg("--no-such-flag").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(!out.stderr.is_empty(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        stdout,
        "The invocation is invalid. Correct the command line against the usage on stderr and retry.\n"
    );
}

/// A stdout nobody will ever read: the very first write fails, and
/// the failure must surface as the reserved malfunction code with
/// the cause on stderr. The dead stream is a socket whose peer is
/// closed before the child starts, so there is no race to lose.
#[cfg(unix)]
#[test]
fn a_dead_stdout_is_a_malfunction() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;

    let home = TempDir::new().unwrap();
    let (dead, peer) = UnixStream::pair().unwrap();
    drop(peer);
    let out = fairway(&home)
        .stdout(Stdio::from(OwnedFd::from(dead)))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.starts_with("fairway: "), "{stderr:?}");
}
