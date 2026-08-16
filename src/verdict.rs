//! The CLI-wide contract between commands and the agent.
//!
//! The exit code registry: Proceed → 0, Adjust → 1, Stop → 2, and
//! [`MALFUNCTION`] (3) when fairway itself could not do its job.

use std::io::Write;
use std::process::ExitCode;

/// The exit code for fairway's own breakage: no verdict was reached,
/// or the reached one could not be delivered. Not a verdict.
pub const MALFUNCTION: u8 = 3;

/// A command's answer to the agent: the full instruction text
/// goes to stdout, the variant sets the exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// All conditions hold; the agent may move on.
    Proceed(String),
    /// The agent must change course on its own: fix the state,
    /// take another route, or report back.
    Adjust(String),
    /// The agent must stop and report to the user.
    Stop(String),
}

impl Verdict {
    /// The exit code the verdict maps to in the CLI-wide registry:
    /// Proceed → 0, Adjust → 1, Stop → 2. Code 3 is [`MALFUNCTION`],
    /// which no verdict maps to.
    #[must_use]
    pub fn code(&self) -> u8 {
        match self {
            Verdict::Proceed(_) => 0,
            Verdict::Adjust(_) => 1,
            Verdict::Stop(_) => 2,
        }
    }

    /// Print the instruction to stdout and turn the verdict into
    /// the process exit code. A stdout that cannot take the
    /// instruction is a malfunction like any other: the cause goes
    /// to stderr, the code is [`MALFUNCTION`].
    #[must_use = "the exit code is the verdict; dropping it reports success"]
    pub fn render(self) -> ExitCode {
        ExitCode::from(self.render_to(&mut std::io::stdout()))
    }

    /// The testable core of [`Self::render`]: the same contract
    /// against any stream standing in for stdout.
    fn render_to(self, stdout: &mut impl Write) -> u8 {
        let code = self.code();
        let (Verdict::Proceed(text) | Verdict::Adjust(text) | Verdict::Stop(text)) = self;
        if let Err(e) = writeln!(stdout, "{text}").and_then(|()| stdout.flush()) {
            // Best effort: a dead stderr on top of a dead stdout
            // cannot make things worse.
            let _ = writeln!(
                std::io::stderr(),
                "fairway: the verdict could not be written to stdout: {e}"
            );
            return MALFUNCTION;
        }
        code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_the_contract() {
        assert_eq!(Verdict::Proceed(String::new()).code(), 0);
        assert_eq!(Verdict::Adjust(String::new()).code(), 1);
        assert_eq!(Verdict::Stop(String::new()).code(), 2);
        assert_eq!(MALFUNCTION, 3);
    }

    #[test]
    fn the_instruction_reaches_the_stream_verbatim() {
        let mut stream = Vec::new();
        let code = Verdict::Adjust(String::from("go another way")).render_to(&mut stream);
        assert_eq!(code, 1);
        assert_eq!(stream, b"go another way\n");
    }

    /// A stream that takes nothing, standing in for a dead stdout.
    /// Its rendering test writes one diagnostic line to the real
    /// stderr; that noise is the point of the path being pinned.
    struct DeadStream;

    impl Write for DeadStream {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_dead_stream_is_a_malfunction() {
        let code = Verdict::Proceed(String::from("carry on")).render_to(&mut DeadStream);
        assert_eq!(code, MALFUNCTION);
    }
}
