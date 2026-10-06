//! CLI assembly, argument parsing, and preparation of the selected handler.

use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::OnceLock;

use clap::ArgMatches;

use crate::handler::{Action, PreparedCommand};
use crate::registry::{Command, FAIRWAY_CLI_COMMANDS, FAIRWAY_CLI_NAMESPACES, Namespace};

/// Parses `args` and prepares the selected command for the application.
///
/// The application supplies its name, version, and description.
/// Namespaces and commands come from linked registrations.
///
/// `args` includes the executable name, as in `std::env::args_os()`.
/// The command contains the parsed arguments and chosen thread count.
/// The handler runs when the application awaits [`PreparedCommand::run`].
///
/// Help, version, and invalid arguments return a [`clap::Error`]. The
/// application uses its diagnostic and exit code to report the result.
pub fn parse(
    name: &'static str,
    version: &'static str,
    about: &'static str,
    args: impl IntoIterator<Item = OsString>,
) -> Result<PreparedCommand, clap::Error> {
    let groups = grouped();
    let mut root = tree(
        clap::Command::new(name).version(version).about(about),
        groups,
    );
    let mut matches = root.try_get_matches_from_mut(args)?;
    let (command, mut matches) = selected(groups, &mut matches)
        .expect("clap requires a namespace and its handler or a subcommand");
    match (command.prepare)(&mut matches)? {
        Action::Run(prepared) => Ok(prepared),
        Action::Help => {
            let namespace = root
                .find_subcommand_mut(command.namespace.name)
                .expect("the selected namespace is in the command tree");
            Err(namespace
                .try_get_matches_from_mut([command.namespace.name, "--help"])
                .expect_err("--help returns clap's help diagnostic"))
        }
    }
}

/// Checks the linked namespaces, commands, and clap argument definitions.
///
/// Call this in an application test with the same metadata, plugins, and
/// features as the shipped application. Use the default test profile: clap's
/// argument checks require debug assertions. It does not run command
/// handlers or worker selectors. `cargo build` alone does not run this check.
///
/// # Panics
///
/// Panics on namespaces without commands, duplicate names, or incompatible
/// registrations, and on invalid clap argument definitions when debug
/// assertions are enabled.
/// Registration conflicts include both declaration locations.
pub fn assert_valid(name: &'static str, version: &'static str, about: &'static str) {
    crate::registry::assert_valid();
    tree(
        clap::Command::new(name).version(version).about(about),
        grouped(),
    )
    .debug_assert();
}

struct Group<'a> {
    namespace: &'a Namespace,
    own: Option<&'a Command>,
    named: Vec<(&'static str, &'a Command)>,
}

fn grouped() -> &'static [Group<'static>] {
    // Only linked declarations are cached; clap configuration remains per call.
    static GROUPS: OnceLock<Vec<Group<'static>>> = OnceLock::new();
    GROUPS.get_or_init(build_groups)
}

fn build_groups() -> Vec<Group<'static>> {
    let mut groups: Vec<_> = FAIRWAY_CLI_NAMESPACES
        .iter()
        .map(|namespace| Group {
            namespace,
            own: None,
            named: Vec::new(),
        })
        .collect();
    // Use declaration identity so duplicate names remain separate registrations.
    let namespaces: HashMap<_, _> = groups
        .iter()
        .enumerate()
        .map(|(index, group)| (std::ptr::from_ref(group.namespace), index))
        .collect();
    for command in FAIRWAY_CLI_COMMANDS {
        let index = namespaces
            .get(&std::ptr::from_ref(command.namespace))
            .expect("command! uses a namespace declared by namespace!");
        let group = &mut groups[*index];
        match command.name {
            Some(name) => group.named.push((name, command)),
            None => group.own = Some(command),
        }
    }
    drop(namespaces);
    groups.sort_unstable_by_key(|group| group.namespace.name);
    for group in &mut groups {
        group.named.sort_unstable_by_key(|(name, _)| *name);
    }
    groups
}

fn tree(root: clap::Command, groups: &[Group<'_>]) -> clap::Command {
    let mut root = root.subcommand_required(true).arg_required_else_help(true);
    for group in groups {
        let mut namespace = clap::Command::new(group.namespace.name);
        if let Some(own) = group.own {
            namespace = (own.augment)(namespace)
                .about(own.about)
                .long_about(own.long_about);
        } else {
            namespace = namespace
                .subcommand_required(true)
                .arg_required_else_help(true);
        }
        if namespace.get_arguments().next().is_some() {
            namespace = namespace
                .subcommand_negates_reqs(true)
                .args_conflicts_with_subcommands(true);
        }
        for (name, command) in &group.named {
            namespace = namespace.subcommand(
                (command.augment)(clap::Command::new(*name))
                    .about(command.about)
                    .long_about(command.long_about),
            );
        }
        root = root.subcommand(namespace);
    }
    root
}

fn selected<'a>(
    groups: &'a [Group<'a>],
    matches: &mut ArgMatches,
) -> Option<(&'a Command, ArgMatches)> {
    let (namespace, mut below) = matches.remove_subcommand()?;
    let group = &groups[groups
        .binary_search_by_key(&namespace.as_str(), |group| group.namespace.name)
        .ok()?];
    let Some((name, arguments)) = below.remove_subcommand() else {
        return Some((group.own?, below));
    };
    let index = group
        .named
        .binary_search_by_key(&name.as_str(), |(declared, _)| *declared)
        .ok()?;
    Some((group.named[index].1, arguments))
}

#[cfg(test)]
mod tests {
    use super::{grouped, tree};

    crate::namespace!(TEXT, "text");
    crate::command!(
        TEXT,
        "Text commands",
        crate::help,
        long_about = "Text commands"
    );
    crate::namespace!(HELLO, "hello");

    async fn run(_: crate::Shutdown) -> anyhow::Result<()> {
        Ok(())
    }
    crate::command!(
        TEXT,
        "upper",
        "Uppercase text",
        run,
        long_about = "Uppercase text"
    );
    crate::command!(
        TEXT,
        "repeat",
        "Repeat text",
        run,
        long_about = "Repeat text"
    );
    crate::command!(
        HELLO,
        "Print a greeting",
        run,
        long_about = "Print a greeting"
    );

    #[test]
    fn namespaces_and_subcommands_are_sorted() {
        let command = tree(clap::Command::new("fairway"), grouped());
        let names: Vec<_> = command
            .get_subcommands()
            .map(clap::Command::get_name)
            .collect();
        assert_eq!(names, ["hello", "text"]);

        let text = command.find_subcommand("text").unwrap();
        let names: Vec<_> = text
            .get_subcommands()
            .map(clap::Command::get_name)
            .collect();
        assert_eq!(names, ["repeat", "upper"]);
    }
}
