//! Statically registered namespaces and commands.

use crate::handler::PreparedCommand;

/// A namespace declared by [`namespace!`](crate::namespace).
///
/// Its CLI name is shared by its subcommands, or used directly when
/// the namespace has its own handler. The Rust static belongs to the
/// declaring crate.
pub struct Namespace {
    pub(crate) name: &'static str,
    pub(crate) about: &'static str,
    source: &'static str,
}

impl Namespace {
    #[doc(hidden)]
    pub const fn new(name: &'static str, about: &'static str, source: &'static str) -> Self {
        valid_name(name);
        Self {
            name,
            about,
            source,
        }
    }
}

const fn valid_name(name: &str) {
    assert!(!name.is_empty(), "a CLI name cannot be empty");
    let bytes = name.as_bytes();
    assert!(
        !matches!(bytes, b"help"),
        "the CLI name `help` is reserved for built-in help"
    );
    let mut i = 0;
    while i < bytes.len() {
        // Decode one scalar in const evaluation; str::chars is not const.
        // The input is &str, so every sequence is already valid UTF-8.
        let first = bytes[i];
        let (mut scalar, width) = match first {
            0..=0x7f => (first as u32, 1),
            0xc0..=0xdf => ((first & 0x1f) as u32, 2),
            0xe0..=0xef => ((first & 0x0f) as u32, 3),
            _ => ((first & 0x07) as u32, 4),
        };
        let mut offset = 1;
        while offset < width {
            scalar = (scalar << 6) | (bytes[i + offset] & 0x3f) as u32;
            offset += 1;
        }
        let character = char::from_u32(scalar).expect("a str contains valid Unicode scalars");
        assert!(
            !character.is_whitespace() && !character.is_control(),
            "a CLI name cannot contain whitespace or control characters"
        );
        i += width;
    }
}

#[doc(hidden)]
pub trait CommandKind<T> {}

#[doc(hidden)]
pub struct Command {
    pub(crate) namespace: &'static Namespace,
    pub(crate) name: Option<&'static str>,
    pub(crate) about: &'static str,
    pub(crate) augment: fn(clap::Command) -> clap::Command,
    pub(crate) prepare: fn(&mut clap::ArgMatches) -> Result<PreparedCommand, clap::Error>,
    source: &'static str,
}

impl Command {
    #[doc(hidden)]
    pub const fn new(
        namespace: &'static Namespace,
        name: Option<&'static str>,
        about: &'static str,
        augment: fn(clap::Command) -> clap::Command,
        prepare: fn(&mut clap::ArgMatches) -> Result<PreparedCommand, clap::Error>,
        source: &'static str,
    ) -> Self {
        if let Some(name) = name {
            valid_name(name);
        }
        Self {
            namespace,
            name,
            about,
            augment,
            prepare,
            source,
        }
    }
}

#[doc(hidden)]
#[linkme::distributed_slice]
pub static FAIRWAY_CLI_NAMESPACES: [Namespace];

#[doc(hidden)]
#[linkme::distributed_slice]
pub static FAIRWAY_CLI_COMMANDS: [Command];

// Run in the application's test binary, where every linked plugin is visible.
pub(crate) fn assert_valid() {
    let mut namespaces: Vec<_> = FAIRWAY_CLI_NAMESPACES.iter().collect();
    namespaces.sort_unstable_by_key(|namespace| (namespace.name, namespace.source));
    for pair in namespaces.windows(2) {
        let (first, second) = (pair[0], pair[1]);
        assert!(
            first.name != second.name,
            "namespace {:?} is declared twice:\n  {}\n  {}",
            first.name,
            first.source,
            second.source,
        );
    }

    for namespace in namespaces {
        let mut commands: Vec<_> = FAIRWAY_CLI_COMMANDS
            .iter()
            .filter(|command| std::ptr::eq(command.namespace, namespace))
            .collect();
        commands.sort_unstable_by_key(|command| (command.name, command.source));
        for pair in commands.windows(2) {
            let (first, second) = (pair[0], pair[1]);
            if first.name == second.name {
                let name = match first.name {
                    Some(name) => format!("{} {name}", namespace.name),
                    None => namespace.name.to_owned(),
                };
                panic!(
                    "command {name:?} is declared twice:\n  {}\n  {}",
                    first.source, second.source,
                );
            }
        }
        if let Some(own) = commands.first().filter(|command| command.name.is_none())
            && let Some(named) = commands.iter().find(|command| command.name.is_some())
        {
            panic!(
                "namespace {:?} mixes its own handler and subcommands:\n  {}\n  {}",
                namespace.name, own.source, named.source,
            );
        }
    }
}
