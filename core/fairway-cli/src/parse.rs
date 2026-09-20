//! CLI assembly, argument parsing, and preparation of the selected handler.

use std::ffi::OsString;

use clap::ArgMatches;

use crate::PreparedCommand;
use crate::registry::{Command, FAIRWAY_CLI_COMMANDS, FAIRWAY_CLI_NAMESPACES, Namespace};

pub(crate) struct Group<'a> {
    namespace: &'a Namespace,
    own: Option<&'a Command>,
    named: Vec<(&'static str, &'a Command)>,
}

pub(crate) fn parse(
    root: clap::Command,
    args: impl IntoIterator<Item = OsString>,
) -> Result<PreparedCommand, clap::Error> {
    let groups = grouped();
    let mut matches = tree(root, &groups).try_get_matches_from(args)?;
    let (command, mut matches) = selected(&groups, &mut matches)
        .expect("clap requires a namespace and its handler or a subcommand");
    (command.prepare)(&mut matches)
}

pub(crate) fn grouped() -> Vec<Group<'static>> {
    let mut groups: Vec<_> = FAIRWAY_CLI_NAMESPACES
        .iter()
        .map(|namespace| Group {
            namespace,
            own: None,
            named: Vec::new(),
        })
        .collect();
    for command in FAIRWAY_CLI_COMMANDS {
        let group = groups
            .iter_mut()
            .find(|group| std::ptr::eq(group.namespace, command.namespace))
            .expect("command! uses a namespace declared by namespace!");
        match command.name {
            Some(name) => group.named.push((name, command)),
            None => group.own = Some(command),
        }
    }
    groups.sort_unstable_by_key(|group| group.namespace.name);
    for group in &mut groups {
        group.named.sort_unstable_by_key(|(name, _)| *name);
    }
    groups
}

pub(crate) fn tree(root: clap::Command, groups: &[Group<'_>]) -> clap::Command {
    let mut root = root.subcommand_required(true).arg_required_else_help(true);
    for group in groups {
        let mut namespace = clap::Command::new(group.namespace.name);
        if let Some(own) = group.own {
            namespace = (own.augment)(namespace).about(group.namespace.about);
            if own.about != group.namespace.about {
                namespace =
                    namespace.long_about(format!("{}\n\n{}", group.namespace.about, own.about,));
            } else {
                namespace = namespace.long_about(None);
            }
        } else {
            namespace = namespace
                .about(group.namespace.about)
                .subcommand_required(true)
                .arg_required_else_help(true);
            for (name, command) in &group.named {
                namespace = namespace.subcommand(
                    (command.augment)(clap::Command::new(*name))
                        .about(command.about)
                        .long_about(None),
                );
            }
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
    let group = groups
        .iter()
        .find(|group| group.namespace.name == namespace)?;
    if let Some(own) = group.own {
        return Some((own, below));
    }
    let (name, arguments) = below.remove_subcommand()?;
    group
        .named
        .iter()
        .find(|(declared, _)| *declared == name)
        .map(|(_, command)| (*command, arguments))
}
