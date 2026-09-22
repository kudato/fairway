//! Application preparation, runtime ownership, and command completion.

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use fairway_cli::{Cli, Threading};

use crate::signal::Supervisor;

pub(crate) fn run(
    cli: Cli,
    args: impl IntoIterator<Item = OsString>,
    timeout: Duration,
    prepare: impl FnOnce() -> anyhow::Result<()>,
) -> ExitCode {
    let command = match cli.parse(args) {
        Ok(command) => command,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(2));
        }
    };

    let result = (|| {
        // Keep supervision active through application preparation, command
        // execution, and runtime teardown, even if any of them blocks.
        let supervisor =
            Supervisor::start(timeout).context("could not watch for termination signals")?;
        fairway_fs::__private::initialize()
            .context("could not initialize the Fairway filesystem")?;
        prepare()?;
        let runtime =
            build_runtime(command.threading()).context("could not start the async runtime")?;
        let result = runtime.block_on(async {
            fairway_config::__private::initialize()
                .await
                .context("could not load Fairway configuration")?;
            command.run(supervisor.shutdown().into()).await
        });
        drop(runtime);
        drop(supervisor);
        result
    })();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn build_runtime(threading: Threading) -> std::io::Result<tokio::runtime::Runtime> {
    let mut builder = match threading {
        Threading::CurrentThread => tokio::runtime::Builder::new_current_thread(),
        Threading::MultiThread { workers } => {
            let mut builder = tokio::runtime::Builder::new_multi_thread();
            let workers = if workers == 0 {
                std::thread::available_parallelism().map_or(1, usize::from)
            } else {
                workers
            };
            builder.worker_threads(workers);
            builder
        }
    };
    builder.enable_all().build()
}
