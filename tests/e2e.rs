//! Pins on the compiled binary, the way an agent meets it: a real
//! process with real streams, exit codes observed from outside.

use std::process::Command;

/// The binary under test, located by cargo.
fn fairway() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fairway"))
}

#[test]
fn the_happy_path_exits_zero() {
    let out = fairway().output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout, concat!("fairway ", env!("CARGO_PKG_VERSION"), "\n"));
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

    let (dead, peer) = UnixStream::pair().unwrap();
    drop(peer);
    let out = fairway()
        .stdout(Stdio::from(OwnedFd::from(dead)))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.starts_with("fairway: "), "{stderr:?}");
}
