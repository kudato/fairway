# Plugins

A plugin is a domain: the flows of one field of work, together with
the skills, subagent configurations, and tools those flows need.
General flows and the mechanisms they rest on belong to `fairway-core`.

One plugin is one crate. Dependencies point one way: a plugin depends
on `fairway-core`, and the `fairway` crate depends on the plugins it
ships. Plugins are published to crates.io alongside `fairway-core`,
at the same version.
