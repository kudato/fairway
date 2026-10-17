//! Namespaces declared by linked crates and the namespaces of their builtins.

use crate::{address, error::report};

#[doc(hidden)]
pub struct Registration {
    name: &'static str,
    module: &'static str,
    source: &'static str,
}

impl Registration {
    #[doc(hidden)]
    pub const fn new(name: &'static str, module: &'static str, source: &'static str) -> Self {
        address::valid_namespace(name);
        Self {
            name,
            module,
            source,
        }
    }
}

/// The namespaces of the builtins that one crate embeds.
#[doc(hidden)]
pub struct Embedded {
    namespaces: &'static [&'static str],
    module: &'static str,
    source: &'static str,
}

impl Embedded {
    #[doc(hidden)]
    pub const fn new(
        namespaces: &'static [&'static str],
        module: &'static str,
        source: &'static str,
    ) -> Self {
        Self {
            namespaces,
            module,
            source,
        }
    }
}

#[doc(hidden)]
#[linkme::distributed_slice]
pub static FAIRWAY_SKILL_NAMESPACES: [Registration];

#[doc(hidden)]
#[linkme::distributed_slice]
pub static FAIRWAY_SKILL_EMBEDDED: [Embedded];

/// Returns every problem of the declarations, one per line.
pub(crate) fn check(namespaces: &[Registration], embedded: &[Embedded]) -> Result<(), String> {
    let mut problems = Vec::new();

    let mut namespaces: Vec<_> = namespaces.iter().collect();
    namespaces.sort_unstable_by_key(|registration| (registration.name, registration.source));
    for group in namespaces.chunk_by(|left, right| left.name == right.name) {
        if group.len() > 1 {
            let sources: Vec<_> = group
                .iter()
                .map(|registration| registration.source)
                .collect();
            problems.push(format!(
                "duplicate skill namespace {:?}: {}",
                group[0].name,
                sources.join(" and "),
            ));
        }
    }

    let mut embedded: Vec<_> = embedded.iter().collect();
    embedded.sort_unstable_by_key(|embedded| embedded.source);
    for embedded in embedded {
        let owner = krate(embedded.module);
        for &name in embedded.namespaces {
            let declared = namespaces.iter().any(|registration| {
                registration.name == name && krate(registration.module) == owner
            });
            if !declared {
                problems.push(format!(
                    "builtins in skill namespace {name:?} that crate `{owner}` does not \
                     declare with `namespace!`: {}",
                    embedded.source,
                ));
            }
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(report("invalid skill namespaces", &problems))
    }
}

/// The crate of a module path.
fn krate(module: &str) -> &str {
    module.split("::").next().unwrap_or(module)
}

#[cfg(test)]
mod tests {
    use super::{Embedded, Registration, check};

    #[test]
    fn every_problem_is_reported_with_its_sources() {
        let error = check(
            &[
                Registration::new("writing", "plugin_b", "plugin_b (b.rs:1)"),
                Registration::new("writing", "plugin_a", "plugin_a (a.rs:1)"),
                Registration::new("editing", "plugin_b", "plugin_b (b.rs:2)"),
                // A registering submodule is reduced to its crate before matching.
                Registration::new("drafts", "plugin_a::skills", "plugin_a::skills (a.rs:2)"),
                // Crate `plugin` does not own the namespaces of crate `plugin_a`.
                Registration::new("notes", "plugin", "plugin (p.rs:1)"),
            ],
            &[Embedded::new(
                &["drafts", "editing", "notes", "writing"],
                "plugin_a::skills",
                "plugin_a::skills (a.rs:9)",
            )],
        )
        .unwrap_err();
        assert_eq!(
            error,
            "invalid skill namespaces:\n  \
             duplicate skill namespace \"writing\": plugin_a (a.rs:1) and plugin_b (b.rs:1)\n  \
             builtins in skill namespace \"editing\" that crate `plugin_a` does not declare \
             with `namespace!`: plugin_a::skills (a.rs:9)\n  \
             builtins in skill namespace \"notes\" that crate `plugin_a` does not declare \
             with `namespace!`: plugin_a::skills (a.rs:9)"
        );
    }
}
