//! End-to-end tests of the compiled binary: the usage it prints, the
//! version, the streams it writes to, and its exit codes, observed
//! from outside the process.
//!
//! No subcommand is named here, so these tests hold for any set of
//! registered commands. Each command is covered by the crate that
//! registers it.

use std::process::Command;

/// The binary under test, located by cargo.
fn fairway() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fairway"))
}

#[test]
fn a_bare_invocation_answers_with_the_usage() {
    let out = fairway().output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("Usage: fairway"), "{stderr:?}");
}

#[test]
fn the_version_flag_reports_the_version() {
    let out = fairway().arg("--version").output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout, concat!("fairway ", env!("CARGO_PKG_VERSION"), "\n"));
}
