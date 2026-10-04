//! Command declarations and their linkme registrations.

/// Declares `pub(crate) static NAME: Namespace` for `fairway <name>`.
///
/// Namespace names must be unique across the linked application.
/// Register the namespace's descriptions and action with [`command!`](crate::command).
/// Use [`help`](crate::help) for a namespace containing subcommands.
/// The application's registry test checks unique names and missing commands.
/// The name `help` is reserved for built-in help and is a compile error.
///
/// ```
/// fairway_cli::namespace!(CLI, "text");
/// ```
#[macro_export]
macro_rules! namespace {
    ($id:ident, $name:literal $(,)?) => {
        #[$crate::__private::linkme::distributed_slice($crate::__private::FAIRWAY_CLI_NAMESPACES)]
        #[linkme(crate = $crate::__private::linkme)]
        #[allow(
            unsafe_code,
            reason = "linkme places the namespace in a linker section"
        )]
        pub(crate) static $id: $crate::Namespace = $crate::Namespace::new(
            $name,
            ::core::concat!(
                ::core::module_path!(),
                " (",
                ::core::file!(),
                ":",
                ::core::line!(),
                ")"
            ),
        );

        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub(crate) mod $id {
            #[allow(dead_code)]
            pub(crate) enum Key {}
        }
    };
}

/// Registers a command in a namespace declared by [`namespace!`].
///
/// `command!(CLI, "about", handler, long_about = "details")` handles
/// `fairway <namespace>`. Adding a name before `"about"` declares a subcommand.
/// Both descriptions are required: `about` is used in command lists and `-h`,
/// and `long_about` in `--help`. They override descriptions on the argument type.
///
/// Register [`help`](crate::help) as the namespace's action to allow subcommands.
/// Invoking that namespace prints its full help and exits with code 0.
/// A regular namespace handler cannot coexist with subcommands.
/// Missing namespace registrations and incompatible actions are compile errors.
/// Duplicate subcommand names are checked by the application's registry test.
/// The subcommand name `help` is reserved for built-in help and is a compile error.
///
/// The handler takes [`Shutdown`](crate::Shutdown), optionally preceded by
/// arguments implementing `clap::Args`, and returns `anyhow::Result<()>`.
///
/// `workers = N` requests N Tokio worker threads from Fairway; zero uses
/// the available parallelism. Without `workers`, Fairway runs the command
/// on one execution thread. A selector such as `workers = |args: &Serve|
/// args.workers` reads the parsed arguments before Fairway creates the runtime.
/// The built-in `help` action does not accept `workers`.
///
/// ```
/// use fairway_cli::Shutdown;
///
/// fairway_cli::namespace!(CLI, "text");
/// fairway_cli::command!(CLI, "Text commands", fairway_cli::help,
///     long_about = "Commands for working with text.");
///
/// #[derive(clap::Args)]
/// struct Upper {
///     /// Text to convert to uppercase.
///     text: String,
/// }
///
/// async fn upper(args: Upper, shutdown: Shutdown) -> anyhow::Result<()> {
///     if shutdown.is_requested() {
///         return Ok(());
///     }
///     println!("{}", args.text.to_uppercase());
///     Ok(())
/// }
///
/// fairway_cli::command!(CLI, "upper", "Convert text to uppercase", upper,
///     long_about = "Convert the supplied text to uppercase and print the result.");
/// ```
#[macro_export]
macro_rules! command {
    ($ns:ident, $name:literal, $about:literal, $run:path, long_about = $long_about:literal $(, workers = $workers:expr)? $(,)?) => {
        const _: () = {
            assert!(
                <$ns::Key as $crate::__private::NamespaceCommand>::IS_HELP,
                "a namespace with subcommands must use fairway_cli::help",
            );
            assert!(
                !$crate::__private::is_help(&$run),
                "fairway_cli::help can only handle a namespace",
            );
            $crate::__declare!($ns, ::core::option::Option::Some($name), $about, $long_about, $run $(, $workers)?);
        };
    };
    ($ns:ident, $about:literal, $run:path, long_about = $long_about:literal $(, workers = $workers:expr)? $(,)?) => {
        const _: () = {
            impl $crate::__private::NamespaceCommand for $ns::Key {
                const IS_HELP: bool = $crate::__private::is_help(&$run);
            }
            $crate::__declare!($ns, ::core::option::Option::None, $about, $long_about, $run $(, $workers)?);
        };
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __declare {
    ($ns:ident, $name:expr, $about:literal, $long_about:literal, $run:path $(, $workers:expr)?) => {
        fn augment(command: $crate::__private::clap::Command) -> $crate::__private::clap::Command {
            $crate::__private::augment($run, command)
        }

        fn prepare(
            matches: &mut $crate::__private::clap::ArgMatches,
        ) -> ::core::result::Result<$crate::__private::Action, $crate::__private::clap::Error> {
            $crate::__private::prepare($run, matches, $crate::__workers!($run $(, $workers)?))
        }

        #[$crate::__private::linkme::distributed_slice($crate::__private::FAIRWAY_CLI_COMMANDS)]
        #[linkme(crate = $crate::__private::linkme)]
        #[allow(unsafe_code, reason = "linkme places the command in a linker section")]
        static COMMAND: $crate::__private::Command = $crate::__private::Command::new(
            &$ns,
            $name,
            ($about, $long_about),
            $crate::__private::is_help(&$run),
            augment,
            prepare,
            ::core::concat!(::core::module_path!(), " (", ::core::file!(), ":", ::core::line!(), ")"),
        );
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __workers {
    ($run:path) => {
        $crate::__private::Threading::CurrentThread
    };
    ($run:path, $workers:expr) => {{
        const _: () = assert!(
            !$crate::__private::is_help(&$run),
            "fairway_cli::help does not use workers",
        );
        $workers
    }};
}
