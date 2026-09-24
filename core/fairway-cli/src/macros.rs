//! Command declarations and their linkme registrations.

/// Declares `pub(crate) static NAME: Namespace` for `fairway <name>`.
///
/// Namespace names must be unique across the linked application.
/// The application's registry test checks this.
///
/// ```
/// fairway_cli::namespace!(CLI, "text", "Text commands");
/// ```
#[macro_export]
macro_rules! namespace {
    ($id:ident, $name:literal, $about:literal $(,)?) => {
        #[$crate::__private::linkme::distributed_slice($crate::__private::FAIRWAY_CLI_NAMESPACES)]
        #[linkme(crate = $crate::__private::linkme)]
        #[allow(
            unsafe_code,
            reason = "linkme places the namespace in a linker section"
        )]
        pub(crate) static $id: $crate::Namespace = $crate::Namespace::new(
            $name,
            $about,
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

/// Registers a handler in a namespace declared by [`namespace!`].
///
/// `command!(CLI, "about", handler)` handles `fairway <namespace>`.
/// `command!(CLI, "name", "about", handler)` adds a subcommand.
/// A namespace supports one of these forms, never both; mixing them is
/// a compile error. Duplicate subcommand names are checked by the
/// application's registry test.
///
/// `workers = N` requests N Tokio worker threads from Fairway; zero uses
/// the available parallelism. Without `workers`, Fairway runs the command
/// on one execution thread. A selector such as `workers = |args: &Serve|
/// args.workers` reads the parsed arguments before Fairway creates the runtime.
///
/// ```
/// fairway_cli::namespace!(CLI, "text", "Text commands");
///
/// #[derive(clap::Args)]
/// struct Upper {
///     /// Text to convert to uppercase.
///     text: String,
/// }
///
/// async fn upper(args: Upper) -> anyhow::Result<()> {
///     println!("{}", args.text.to_uppercase());
///     Ok(())
/// }
///
/// fairway_cli::command!(CLI, "upper", "Convert text to uppercase", upper);
/// ```
#[macro_export]
macro_rules! command {
    ($ns:ident, $name:literal, $about:literal, $run:path $(, workers = $workers:expr)? $(,)?) => {
        const _: () = {
            #[allow(dead_code)]
            struct Named;
            impl $crate::__private::CommandKind<Named> for $ns::Key {}
            $crate::__declare!($ns, ::core::option::Option::Some($name), $about, $run $(, $workers)?);
        };
    };
    ($ns:ident, $about:literal, $run:path $(, workers = $workers:expr)? $(,)?) => {
        const _: () = {
            // Conflicts with every named command, or a second own handler.
            impl<T> $crate::__private::CommandKind<T> for $ns::Key {}
            $crate::__declare!($ns, ::core::option::Option::None, $about, $run $(, $workers)?);
        };
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __declare {
    ($ns:ident, $name:expr, $about:literal, $run:path $(, $workers:expr)?) => {
        fn augment(command: $crate::__private::clap::Command) -> $crate::__private::clap::Command {
            $crate::__private::augment($run, command)
        }

        fn prepare(
            matches: &mut $crate::__private::clap::ArgMatches,
        ) -> ::core::result::Result<$crate::__private::PreparedCommand, $crate::__private::clap::Error> {
            $crate::__private::prepare($run, matches, $crate::__workers!($($workers)?))
        }

        #[$crate::__private::linkme::distributed_slice($crate::__private::FAIRWAY_CLI_COMMANDS)]
        #[linkme(crate = $crate::__private::linkme)]
        #[allow(unsafe_code, reason = "linkme places the command in a linker section")]
        static COMMAND: $crate::__private::Command = $crate::__private::Command::new(
            &$ns,
            $name,
            $about,
            augment,
            prepare,
            ::core::concat!(::core::module_path!(), " (", ::core::file!(), ":", ::core::line!(), ")"),
        );
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __workers {
    () => {
        $crate::__private::Threading::CurrentThread
    };
    ($workers:expr) => {
        $workers
    };
}
