//! Git subsystem: the process runner and fact checks.
//!
//! Every check answers a yes/no question; a [`GitError`] means the
//! question could not be answered. What the answers imply belongs
//! to the callers.
//!
//! stdout is the answer channel: enumerated success is judged on it
//! alone, and stderr chatter (hints, advice) does not turn an answer
//! into a failure.

mod checks;

#[cfg(test)]
mod live;

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::Stdio;

use tokio::process::Command;

/// Configuration pinned on every invocation so paths and messages
/// stay stable regardless of the user's settings.
const PINS: [&str; 2] = ["-c", "core.quotePath=false"];

/// Environment variables that redirect git at another repository,
/// index, or object store. Hermetic runs remove them so a throwaway
/// repository stays the only thing a test can touch.
const REPO_TARGETING_VARS: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

/// Environment variables that carry configuration past the pinned
/// config files: inline config entries, and the template directory
/// that seeds `git init` with host-controlled hooks.
const CONFIG_CARRYING_VARS: [&str; 3] = [
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_TEMPLATE_DIR",
];

/// Environment variables that make git write traces. A trace target
/// can be a host file or socket, so an inherited value is a write
/// channel out of the sandbox, not just stderr chatter.
const TRACE_VARS: [&str; 8] = [
    "GIT_TRACE",
    "GIT_TRACE2",
    "GIT_TRACE2_EVENT",
    "GIT_TRACE2_PERF",
    "GIT_TRACE_PACK_ACCESS",
    "GIT_TRACE_PACKET",
    "GIT_TRACE_PERFORMANCE",
    "GIT_TRACE_SETUP",
];

/// A git invocation that could not produce an enumerated answer.
#[derive(Debug)]
#[non_exhaustive]
pub enum GitError {
    /// The git binary could not be started at all.
    #[non_exhaustive]
    SpawnFailed {
        /// The full command line, for diagnostics.
        command: String,
        /// The error the operating system gave when starting git.
        source: std::io::Error,
    },
    /// git exited with a status outside the enumerated outcomes.
    #[non_exhaustive]
    CommandFailed {
        /// The full command line, for diagnostics.
        command: String,
        /// The exit code, or -1 when the process died without one.
        status: i32,
        /// The signal that killed the process, when it did not exit
        /// on its own (unix only).
        signal: Option<i32>,
        /// Captured standard error, lossily decoded as UTF-8.
        stderr: String,
    },
    /// git exited successfully but printed something outside
    /// the enumerated outputs.
    #[non_exhaustive]
    UnexpectedOutput {
        /// The full command line, for diagnostics.
        command: String,
        /// The stdout that matched no enumerated answer.
        output: String,
        /// Captured standard error, lossily decoded as UTF-8.
        stderr: String,
    },
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::SpawnFailed { command, source } => {
                write!(f, "git call failed\ncommand: {command}\ncause: {source}")
            }
            GitError::CommandFailed {
                command,
                status,
                signal,
                stderr,
            } => {
                write!(
                    f,
                    "git call failed\ncommand: {command}\nexit status: {status}"
                )?;
                if let Some(signal) = signal {
                    write!(f, " (killed by signal {signal})")?;
                }
                write!(f, "\nstderr: {}", field(stderr))
            }
            GitError::UnexpectedOutput {
                command,
                output,
                stderr,
            } => write!(
                f,
                "git call failed\ncommand: {command}\nexit status: 0\nunexpected output: {}\nstderr: {}",
                field(output),
                field(stderr)
            ),
        }
    }
}

// The io source is already rendered inside the block, so source()
// stays at its default None and chain printers do not repeat it.
impl std::error::Error for GitError {}

/// A captured stream as a block field: trailing newline dropped,
/// emptiness made visible.
fn field(text: &str) -> &str {
    let text = text.trim_end();
    if text.is_empty() { "(empty)" } else { text }
}

/// Everything a finished git process left behind.
#[derive(Debug)]
#[non_exhaustive]
pub struct GitOutput {
    /// The full command line, for diagnostics.
    pub command: String,
    /// The exit code, or -1 when the process died without one; -1 is
    /// no real exit code, so enumerations can never mistake it for
    /// an answer.
    pub status: i32,
    /// The signal that killed the process, when it did not exit on
    /// its own (unix only).
    pub signal: Option<i32>,
    /// Captured standard output, lossily decoded as UTF-8.
    pub stdout: String,
    /// Captured standard error, lossily decoded as UTF-8.
    pub stderr: String,
}

impl GitOutput {
    /// Classify an unenumerated result: success with unrecognized
    /// output and failure are different kinds of breakage.
    /// Interpreters call this on every outcome outside their
    /// enumeration, so unknown git behavior fails loud instead of
    /// passing for an answer.
    #[must_use]
    pub fn into_error(self) -> GitError {
        if self.status == 0 {
            GitError::UnexpectedOutput {
                command: self.command,
                output: self.stdout,
                stderr: self.stderr,
            }
        } else {
            // stdout is dropped deliberately: git explains failures
            // on stderr, and carrying the almost-always-empty field
            // would cost every diagnostic block a noise line.
            GitError::CommandFailed {
                command: self.command,
                status: self.status,
                signal: self.signal,
                stderr: self.stderr,
            }
        }
    }
}

/// A way to talk to the user's git: carries the working directory
/// its commands run in.
///
/// The child git inherits the process environment, so variables like
/// `GIT_DIR` retarget it exactly as they would retarget any other
/// git command the caller runs.
#[derive(Debug, Default)]
pub struct Git {
    workdir: Option<PathBuf>,
    hermetic: bool,
}

impl Git {
    /// Talk to git in the process's current directory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Talk to git in the given directory.
    #[must_use]
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self {
            workdir: Some(dir.into()),
            hermetic: false,
        }
    }

    /// Talk to git in the given directory with the environment's
    /// repository-targeting variables and personal configuration
    /// neutralized. Tests build throwaway repositories with this, so
    /// an inherited `GIT_DIR` or a hook-laden global config cannot
    /// reach outside them.
    #[cfg(test)]
    pub(crate) fn hermetic_at(dir: impl Into<PathBuf>) -> Self {
        Self {
            workdir: Some(dir.into()),
            hermetic: true,
        }
    }

    /// Run git with the given arguments, no shell in between.
    ///
    /// # Errors
    ///
    /// Only [`GitError::SpawnFailed`], when the process cannot be
    /// started; a git that ran and exited is `Ok` whatever its
    /// status.
    pub async fn run<I, S>(&self, args: I) -> Result<GitOutput, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
        let command = display_command(&args);
        let output =
            self.command(&args)
                .output()
                .await
                .map_err(|source| GitError::SpawnFailed {
                    command: command.clone(),
                    source,
                })?;

        let (status, signal) = exit_status_parts(&output.status);
        Ok(GitOutput {
            command,
            status,
            signal,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Assemble the child invocation: pins, arguments, pinned locale,
    /// working directory, and — on hermetic runs — the environment
    /// scrub.
    fn command(&self, args: &[OsString]) -> Command {
        let mut process = Command::new("git");
        process
            .args(PINS)
            .args(args)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            // A termination caught mid-run drops the in-flight child;
            // read-only git is safe to kill rather than orphan.
            .kill_on_drop(true);
        if let Some(dir) = &self.workdir {
            process.current_dir(dir);
        }
        if self.hermetic {
            self.scrub(&mut process);
        }
        process
    }

    /// Cut every verified leak channel between a hermetic child and
    /// the host: repository targeting, smuggled configuration, trace
    /// output, upward discovery, and the global ignore/attributes
    /// files that no config file covers.
    fn scrub(&self, process: &mut Command) {
        for var in REPO_TARGETING_VARS
            .iter()
            .chain(&CONFIG_CARRYING_VARS)
            .chain(&TRACE_VARS)
        {
            process.env_remove(var);
        }
        process
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_ATTR_NOSYSTEM", "1")
            // The inline-config channel is repurposed: pinning the
            // ignore/attributes paths also keeps the channel itself
            // occupied. Hostile entries beyond the count stay in the
            // environment but are dead.
            .env("GIT_CONFIG_COUNT", "2")
            .env("GIT_CONFIG_KEY_0", "core.excludesFile")
            .env("GIT_CONFIG_VALUE_0", "/dev/null")
            .env("GIT_CONFIG_KEY_1", "core.attributesFile")
            .env("GIT_CONFIG_VALUE_1", "/dev/null");
        if let Some(parent) = self.workdir.as_ref().and_then(|dir| dir.parent()) {
            // Discovery must not climb out of the throwaway
            // directory; the workdir itself does not block the
            // climb, so the ceiling is its parent.
            process.env("GIT_CEILING_DIRECTORIES", parent);
        }
    }
}

/// Map a finished process onto the [`GitOutput`] status fields: the
/// exit code, or the -1 sentinel with the killing signal when the
/// process died without one.
fn exit_status_parts(status: &std::process::ExitStatus) -> (i32, Option<i32>) {
    (status.code().unwrap_or(-1), exit_signal(status))
}

/// The human-readable command line for diagnostic blocks.
fn display_command(args: &[OsString]) -> String {
    let mut command = String::from("git");
    for pin in PINS {
        command.push(' ');
        command.push_str(pin);
    }
    for arg in args {
        command.push(' ');
        command.push_str(&arg.to_string_lossy());
    }
    command
}

/// The signal that killed the process, if any; never present off unix.
#[cfg(unix)]
fn exit_signal(status: &std::process::ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(status)
}

#[cfg(not(unix))]
fn exit_signal(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    mod display {
        use super::*;

        #[test]
        fn command_failure_renders_the_labeled_block() {
            let error = GitError::CommandFailed {
                command: "git -c core.quotePath=false status".into(),
                status: 128,
                signal: None,
                stderr: "fatal: broken\n".into(),
            };
            assert_eq!(
                error.to_string(),
                "git call failed\n\
                 command: git -c core.quotePath=false status\n\
                 exit status: 128\n\
                 stderr: fatal: broken"
            );
        }

        #[test]
        fn signal_death_renders_the_signal() {
            let error = GitError::CommandFailed {
                command: "git -c core.quotePath=false status".into(),
                status: -1,
                signal: Some(9),
                stderr: String::new(),
            };
            assert_eq!(
                error.to_string(),
                "git call failed\n\
                 command: git -c core.quotePath=false status\n\
                 exit status: -1 (killed by signal 9)\n\
                 stderr: (empty)"
            );
        }

        #[test]
        fn unexpected_output_renders_status_zero() {
            let error = GitError::UnexpectedOutput {
                command: "git -c core.quotePath=false rev-parse --is-inside-work-tree".into(),
                output: "maybe\n".into(),
                stderr: "hint: something\n".into(),
            };
            assert_eq!(
                error.to_string(),
                "git call failed\n\
                 command: git -c core.quotePath=false rev-parse --is-inside-work-tree\n\
                 exit status: 0\n\
                 unexpected output: maybe\n\
                 stderr: hint: something"
            );
        }

        #[test]
        fn spawn_failure_renders_the_cause() {
            let error = GitError::SpawnFailed {
                command: "git -c core.quotePath=false --version".into(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "no git in PATH"),
            };
            assert_eq!(
                error.to_string(),
                "git call failed\n\
                 command: git -c core.quotePath=false --version\n\
                 cause: no git in PATH"
            );
        }

        #[test]
        fn empty_streams_are_visible() {
            let error = GitError::CommandFailed {
                command: "git -c core.quotePath=false status".into(),
                status: 1,
                signal: None,
                stderr: String::new(),
            };
            assert_eq!(
                error.to_string(),
                "git call failed\n\
                 command: git -c core.quotePath=false status\n\
                 exit status: 1\n\
                 stderr: (empty)"
            );
        }
    }

    mod invocation {
        use std::collections::HashMap;
        use std::ffi::OsStr;
        use std::path::Path;

        use super::*;

        #[test]
        fn every_child_carries_the_pins() {
            let command = Git::new().command(&["status".into()]);
            assert_eq!(command.as_std().get_program(), "git");
            let args: Vec<_> = command
                .as_std()
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert_eq!(args, ["-c", "core.quotePath=false", "status"]);
        }

        #[test]
        fn the_displayed_command_carries_the_pins() {
            assert_eq!(
                display_command(&["status".into(), "--porcelain".into()]),
                "git -c core.quotePath=false status --porcelain"
            );
        }

        #[test]
        fn the_workdir_reaches_the_child() {
            let command = Git::at("/somewhere/repo").command(&[]);
            assert_eq!(
                command.as_std().get_current_dir(),
                Some(Path::new("/somewhere/repo"))
            );
            assert_eq!(Git::new().command(&[]).as_std().get_current_dir(), None);
        }

        /// The documented inherit-the-environment behavior: a plain
        /// child gets no scrub, only the locale pin.
        #[test]
        fn plain_children_inherit_the_environment() {
            let command = Git::at("/somewhere/repo").command(&["status".into()]);
            let envs: HashMap<_, _> = command.as_std().get_envs().collect();
            assert_eq!(envs.get(OsStr::new("LC_ALL")), Some(&Some(OsStr::new("C"))));
            assert_eq!(envs.len(), 1, "no other overrides expected: {envs:?}");
        }

        /// The full environment plan of a hermetic child, spelled out
        /// literally: the test is the second, independent copy of the
        /// scrub list, so silently dropping a line on either side
        /// fails here.
        #[test]
        fn hermetic_children_get_the_full_scrub() {
            let command = Git::hermetic_at("/tmp/throwaway/repo").command(&["status".into()]);
            let envs: HashMap<_, _> = command.as_std().get_envs().collect();
            let removed = [
                "GIT_DIR",
                "GIT_WORK_TREE",
                "GIT_INDEX_FILE",
                "GIT_COMMON_DIR",
                "GIT_OBJECT_DIRECTORY",
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                "GIT_CONFIG_PARAMETERS",
                "GIT_TEMPLATE_DIR",
                "GIT_TRACE",
                "GIT_TRACE2",
                "GIT_TRACE2_EVENT",
                "GIT_TRACE2_PERF",
                "GIT_TRACE_PACK_ACCESS",
                "GIT_TRACE_PACKET",
                "GIT_TRACE_PERFORMANCE",
                "GIT_TRACE_SETUP",
            ];
            for var in removed {
                assert_eq!(
                    envs.get(OsStr::new(var)),
                    Some(&None),
                    "{var} must be removed"
                );
            }
            let set = [
                ("LC_ALL", "C"),
                ("GIT_CONFIG_GLOBAL", "/dev/null"),
                ("GIT_CONFIG_NOSYSTEM", "1"),
                ("GIT_ATTR_NOSYSTEM", "1"),
                ("GIT_CONFIG_COUNT", "2"),
                ("GIT_CONFIG_KEY_0", "core.excludesFile"),
                ("GIT_CONFIG_VALUE_0", "/dev/null"),
                ("GIT_CONFIG_KEY_1", "core.attributesFile"),
                ("GIT_CONFIG_VALUE_1", "/dev/null"),
                ("GIT_CEILING_DIRECTORIES", "/tmp/throwaway"),
            ];
            for (var, value) in set {
                assert_eq!(
                    envs.get(OsStr::new(var)),
                    Some(&Some(OsStr::new(value))),
                    "{var} must be set to {value}"
                );
            }
            assert_eq!(
                envs.len(),
                removed.len() + set.len(),
                "an unlisted override crept in: {envs:?}"
            );
        }
    }

    mod into_error {
        use super::*;

        #[test]
        fn a_failure_keeps_the_diagnostic_payload() {
            let out = GitOutput {
                command: "git -c core.quotePath=false status".into(),
                status: 128,
                signal: None,
                stdout: "half an answer\n".into(),
                stderr: "fatal: broken\n".into(),
            };
            assert_eq!(
                out.into_error().to_string(),
                "git call failed\n\
                 command: git -c core.quotePath=false status\n\
                 exit status: 128\n\
                 stderr: fatal: broken"
            );
        }

        #[test]
        fn a_signal_death_keeps_the_signal() {
            let out = GitOutput {
                command: "git -c core.quotePath=false status".into(),
                status: -1,
                signal: Some(15),
                stdout: String::new(),
                stderr: String::new(),
            };
            assert_eq!(
                out.into_error().to_string(),
                "git call failed\n\
                 command: git -c core.quotePath=false status\n\
                 exit status: -1 (killed by signal 15)\n\
                 stderr: (empty)"
            );
        }

        #[test]
        fn an_unexpected_success_keeps_the_diagnostic_payload() {
            let out = GitOutput {
                command: "git -c core.quotePath=false rev-parse --is-inside-work-tree".into(),
                status: 0,
                signal: None,
                stdout: "maybe\n".into(),
                stderr: "hint: something\n".into(),
            };
            assert_eq!(
                out.into_error().to_string(),
                "git call failed\n\
                 command: git -c core.quotePath=false rev-parse --is-inside-work-tree\n\
                 exit status: 0\n\
                 unexpected output: maybe\n\
                 stderr: hint: something"
            );
        }
    }

    #[cfg(unix)]
    mod exit_status {
        use std::os::unix::process::ExitStatusExt;
        use std::process::ExitStatus;

        use super::*;

        #[test]
        fn a_real_exit_code_passes_through() {
            let status = ExitStatus::from_raw(128 << 8);
            assert_eq!(exit_status_parts(&status), (128, None));
        }

        /// The sentinel invariant: a signal death maps to -1, which
        /// no enumeration accepts as an answer.
        #[test]
        fn a_signal_death_maps_to_the_sentinel() {
            let status = ExitStatus::from_raw(9);
            assert_eq!(exit_status_parts(&status), (-1, Some(9)));
        }
    }
}
