# Commands

`fairway-cli` lets a plugin declare CLI commands and arguments.

## Quick start

`Namespace` groups commands under a common CLI name: `fairway <namespace>`.

`namespace!(CLI, "name", "about")` declares a namespace: `CLI` is the object name
in Rust code, `"name"` is the CLI name, and `"about"` is the help description.

`command!(CLI, "about", handler)` binds the `fairway <namespace>` invocation
to a handler.

```rust
// src/lib.rs

fairway_cli::namespace!(CLI, "hello", "Print a greeting");

async fn hello() -> anyhow::Result<()> {
    println!("Hello, world!");
    Ok(())
}

fairway_cli::command!(CLI, "Print a greeting", hello);
```

```console
$ fairway hello
Hello, world!

```

## Arguments and subcommands

To register a subcommand, add its name to `command!`:
`command!(CLI, "name", "about", handler)` registers the invocation
`fairway <namespace> <name>`.

CLI arguments are described by a type implementing `clap::Args`;
the handler receives a value of that type.

```rust
// src/lib.rs

fairway_cli::namespace!(CLI, "text", "Text commands");

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

async fn upper(args: Upper) -> anyhow::Result<()> {
    println!("{}", args.text.to_uppercase());
    Ok(())
}

async fn repeat(args: Repeat) -> anyhow::Result<()> {
    for _ in 0..args.times {
        println!("{}", args.text);
    }
    Ok(())
}

fairway_cli::command!(CLI, "upper", "Convert text to uppercase", upper);
fairway_cli::command!(CLI, "repeat", "Repeat text", repeat);
```

```console
$ fairway text upper "Hello, world!"
HELLO, WORLD!

$ fairway text repeat hello --times 3
hello
hello
hello

```

## Threads and shutdown

A handler can also accept `Shutdown`, a notification of a shutdown request.
`requested().await` waits for that request.

`workers = N` sets the number of Tokio worker threads; `0` uses the available cores.
To choose the count from arguments, pass a function: `workers = |args: &Serve| args.workers`.
Fairway computes the thread count before running the handler.

```rust
// src/lib.rs

use std::net::SocketAddr;

use axum::{Router, routing::get};
use fairway_cli::Shutdown;
use tokio::net::TcpListener;

fairway_cli::namespace!(CLI, "http", "HTTP server");

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

fairway_cli::command!(CLI, "serve", "Start the server", serve, workers = |args: &Serve| args.workers);
```

```console,ignore
$ fairway http serve --listen 127.0.0.1:9000 --workers 4
```

## API

### Registration

- `namespace!(CLI, "name", "about")` declares
  `pub(crate) static CLI: Namespace` for `fairway name`.
- `command!(CLI, "name", "about", handler)` adds a subcommand.
- `command!(CLI, "about", handler)` sets the handler for `fairway <namespace>`.

Namespace names are unique across the application; command names are unique within a namespace.
A namespace registers either subcommands or a handler for the namespace itself.
The application test checks for duplicate names: `cargo test -p fairway`.
The check covers the plugins and Cargo features included in that build;
`cargo build` does not run it. Mixing the two kinds of commands or registering
a second handler for the namespace itself causes a compilation error.
Names must be nonempty and contain no whitespace or control characters.

### Help

`fairway-cli` automatically generates `--help` output for the application,
a command namespace, and an individual subcommand.

`about` provides the help description; doc comments on argument fields
provide CLI parameter descriptions.

### Handlers

A handler is an `async fn` returning `anyhow::Result<()>` with a `Send` future.
Supported parameters:

- `()`.
- `(args: T)`.
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
- `new()` creates an object with no request source, for tests.

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
