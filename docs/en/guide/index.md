# Using Fairway

Fairway is a CLI whose commands are added by plugins built together with it,
so the available commands depend on which plugins are included in the build.
The first word after `fairway` is the command namespace, followed by a
subcommand, if any, and arguments:

```text
fairway <namespace> [subcommand] [arguments]
```

## Installation

Fairway is published on PyPI and crates.io. To run it without installing,
use [uv](https://docs.astral.sh/uv/):

```sh
uvx fairway --help
```

To install it:

- with uv: `uv tool install fairway`;
- with pip: `pip install fairway`;
- with Cargo: `cargo install fairway --locked`.

Prebuilt packages for uv and pip are available for Linux x86_64 and aarch64
(glibc and musl), macOS on Apple silicon and Intel, and Windows x86_64: on
these platforms, Rust is not needed. On other platforms, uv and pip build
Fairway from source, as `cargo install` does. Building requires Rust 1.98.1
or later.

To check the installation:

```sh
fairway --version
```

## Help

`--help` shows help for Fairway, a command namespace, or a subcommand:

```sh
fairway --help
fairway <namespace> --help
fairway <namespace> <subcommand> --help
```

`-h` prints short help, and `--help` prints full help with detailed
descriptions if the plugin provides them.

## Exit codes

| Code | When |
|---|---|
| 0 | the command succeeded, including a clean stop after a signal; help or version was printed |
| 1 | the command failed; startup failed, for example when settings could not be loaded; the command did not finish within 15 seconds after a signal |
| 2 | invalid arguments; the command namespace or subcommand is missing |
| 101 | internal error in the command |

Fairway prints error messages to stderr as `error: ...`.

## Interrupting a command

On Unix, SIGINT (Ctrl+C), SIGTERM, and SIGHUP interrupt a command; on
Windows, Ctrl+C does. Fairway does not stop the command right away: it asks
the command to finish and waits up to 15 seconds so that it can, for
example, finish sending responses and save its state. If the command does
not finish in time, Fairway prints a message with the name of the received
signal and exits with code 1:

```text
error: still running 15s after SIGINT
```

Another signal does not shorten the deadline.

## Configuration

Plugin settings are kept in the Fairway directory, `~/.fairway` by default.
Where it is, which files Fairway reads, and how it merges their values is
described in [Configuring Fairway](configuration.md).
