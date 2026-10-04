# Commands

`fairway-cli` lets a plugin declare CLI commands and arguments.

## Quick start

`Namespace` groups commands under a common CLI name: `fairway <namespace>`.

`namespace!(CLI, "name")` declares a namespace: `CLI` is the object name
in Rust code, `"name"` is the CLI name.

`command!(CLI, "about", handler, long_about = "details")` binds the `fairway <namespace>` invocation
to a handler.

```rust
// src/lib.rs

use fairway_cli::Shutdown;

fairway_cli::namespace!(CLI, "hello");

async fn hello(shutdown: Shutdown) -> anyhow::Result<()> {
    if shutdown.is_requested() {
        return Ok(());
    }
    println!("Hello, world!");
    Ok(())
}

fairway_cli::command!(
    CLI,
    "Print a greeting",
    hello,
    long_about = "Write Hello, world! to standard output.",
);
```

```console
$ fairway hello
Hello, world!

```

## Arguments and subcommands

To register a subcommand, add its name to `command!`:
`command!(CLI, "name", "about", handler, long_about = "details")` registers the invocation
`fairway <namespace> <name>`.

To display help when the namespace itself is invoked, register
`fairway_cli::help`, as in the example below.

CLI arguments are described by a type implementing `clap::Args`;
the handler receives a value of that type.

```rust
// src/lib.rs

use fairway_cli::Shutdown;

fairway_cli::namespace!(CLI, "text");
fairway_cli::command!(
    CLI,
    "Text commands",
    fairway_cli::help,
    long_about = "Convert text to uppercase or repeat it a chosen number of times.",
);

#[derive(clap::Args)]
struct Upper {
    /// Text to convert to uppercase.
    text: String,
}

#[derive(clap::Args)]
struct Repeat {
    /// Text to repeat.
    text: String,

    /// Number of repetitions.
    #[arg(long, default_value_t = 2)]
    times: usize,
}

async fn upper(args: Upper, shutdown: Shutdown) -> anyhow::Result<()> {
    if shutdown.is_requested() {
        return Ok(());
    }
    println!("{}", args.text.to_uppercase());
    Ok(())
}

async fn repeat(args: Repeat, shutdown: Shutdown) -> anyhow::Result<()> {
    for _ in 0..args.times {
        if shutdown.is_requested() {
            break;
        }
        println!("{}", args.text);
    }
    Ok(())
}

fairway_cli::command!(
    CLI,
    "upper",
    "Convert text to uppercase",
    upper,
    long_about = "Convert the supplied text to uppercase and print the result.",
);
fairway_cli::command!(
    CLI,
    "repeat",
    "Repeat text",
    repeat,
    long_about = "Print the supplied text on separate lines. Use --times to choose how many times.",
);
```

```console
$ fairway text upper "Hello, world!"
HELLO, WORLD!

$ fairway text repeat hello --times 3
hello
hello
hello

```

### Own handler and subcommands

A namespace can have both its own handler and subcommands.
Without a subcommand, Fairway calls the namespace's handler;
otherwise, it calls the selected subcommand's handler.

For example, replace `fairway_cli::help` with `repeat` in the `text`
namespace registration above. Then `fairway text hello` calls `repeat`,
and `fairway text upper hello` calls `upper`.

Arguments and `workers` belong to the selected handler.
Required namespace arguments do not apply to a subcommand.
Arguments for the namespace's own handler cannot be combined with a subcommand.

## Threads and shutdown

`Shutdown` is passed as the handler's last parameter.
`requested().await` waits for a shutdown request.

Plugin developers must handle shutdown requests received through `Shutdown`.

`workers = N` sets the number of Tokio worker threads; `0` uses the available cores.
To choose the count from arguments, pass a function: `workers = |args: &Serve| args.workers`.
Fairway computes the thread count before running the handler.

```rust
// src/lib.rs

use std::net::SocketAddr;

use axum::{Router, routing::get};
use fairway_cli::Shutdown;
use tokio::net::TcpListener;

fairway_cli::namespace!(CLI, "http");
fairway_cli::command!(
    CLI,
    "HTTP server",
    fairway_cli::help,
    long_about = "Commands for running an HTTP server.",
);

#[derive(clap::Args)]
struct Serve {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: SocketAddr,

    /// Worker threads; zero uses the available cores.
    #[arg(long, default_value_t = 0)]
    workers: usize,
}

async fn serve(args: Serve, shutdown: Shutdown) -> anyhow::Result<()> {
    let listener = TcpListener::bind(args.listen).await?;
    let app = Router::new().route("/health", get(|| async { "ok" }));

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.requested().await;
        })
        .await?;

    Ok(())
}

fairway_cli::command!(
    CLI,
    "serve",
    "Start the server",
    serve,
    long_about = "Serve the /health endpoint. Use --listen to choose the address.",
    workers = |args: &Serve| args.workers,
);
```

```console,ignore
$ fairway http serve --listen 127.0.0.1:9000 --workers 4
```

## API

### Registration

- `namespace!(CLI, "name")` declares
  `pub(crate) static CLI: Namespace` for `fairway name`.
- `command!(CLI, "name", "about", handler, long_about = "details")` adds a subcommand.
- `command!(CLI, "about", handler, long_about = "details")` sets the handler for `fairway <namespace>`.

Namespace names are unique across the application; command names are unique within a namespace.
A namespace registers a regular handler or `fairway_cli::help`.
Both permit subcommands. When invoked without a subcommand,
`fairway_cli::help` displays full help and exits with code 0.
The application test checks for namespaces without commands and duplicate
names: `cargo test -p fairway`.
The check covers the plugins and Cargo features included in that build;
`cargo build` does not run it. Registering a second namespace action
causes a compilation error.
Names must be nonempty and contain no whitespace or control characters.

The name `help` is reserved for built-in help. A namespace or subcommand
with this name is a compile error.

### Help

`fairway-cli` automatically generates `--help` output for the application,
a command namespace, and an individual subcommand.

Every `command!` requires both descriptions: `about` for command lists
and `-h`, and `long_about` for `--help`. Doc comments on argument fields
provide CLI parameter descriptions. Descriptions on the argument type itself
do not replace the text from `command!`.

`fairway_cli::help` displays help before loading configuration or creating a runtime.
It can be imported with `use fairway_cli::help;` or under another name.
The `workers` parameter is not allowed for this action.

### Handlers

A handler is an `async fn` returning `anyhow::Result<()>`. It runs on the
main thread, including when `workers` is set, so its future need not
implement `Send`. Tasks passed to `tokio::spawn` still require `Send`.

Supported parameters:

- `(shutdown: Shutdown)`.
- `(args: T, shutdown: Shutdown)`.

`T` implements `clap::Args`. Handler compatibility is checked at compile time.

### Threads

`command!(…, workers = N)` sets the number of Tokio worker threads;
`0` uses the available cores. Without `workers`, execution uses a single thread.
`workers = |args: &T| args.workers` selects the count from the parsed arguments.
This setting does not limit separate threads created by the plugin itself.

### Shutdown

`Shutdown` notifies the handler of a shutdown request:

- `requested().await` waits for the request; returns immediately if it has already arrived.
- `is_requested()` checks whether shutdown has been requested.
- `clone()` creates a copy that observes the same request.

In a test, pass `Shutdown::from(token.clone())` to the handler, where `token`
is a `tokio_util::sync::CancellationToken`. Call `token.cancel()` to request
shutdown.

Fairway handles signals and gives the command time to shut down:
15 seconds by default, configurable at the Fairway level.
Once the deadline expires, the process is forcibly terminated, even if
the handler does not use `Shutdown` or blocks execution.

### Errors

Return errors through `Result`: Fairway prints them to stderr and sets
the exit code. The handler must not print the error again or call `exit`.

- `0` — `Ok(())`, help, or version.
- `1` — a command or preparation error, or an exceeded shutdown deadline.
- `2` — a CLI argument error.
