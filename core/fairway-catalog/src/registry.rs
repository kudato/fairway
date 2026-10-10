//! Namespaces and builtin tables registered by linked crates.

use std::borrow::Cow;

use crate::{Error, Namespace, address};

/// A declared namespace with its value type erased, as initialization sees it.
pub(crate) trait Store: Sync {
    fn name(&self) -> &'static str;

    /// Whether the namespace is declared with `builtin`.
    fn builtin(&self) -> bool;

    /// Builds the initial values, adds their problems, and returns the
    /// function that publishes them.
    fn prepare(
        &'static self,
        builtins: &[&Builtin],
        source: &'static str,
        problems: &mut Vec<String>,
    ) -> Box<dyn FnOnce()>;
}

#[doc(hidden)]
pub struct Registration {
    namespace: &'static dyn Store,
    source: &'static str,
}

impl Registration {
    #[doc(hidden)]
    pub const fn new<V: Send + Sync + 'static>(
        namespace: &'static Namespace<V>,
        source: &'static str,
    ) -> Self {
        Self { namespace, source }
    }
}

#[doc(hidden)]
pub struct Builtin {
    pub(crate) namespace: &'static str,
    pub(crate) entries: &'static [(&'static str, &'static [u8])],
    pub(crate) source: &'static str,
}

impl Builtin {
    #[doc(hidden)]
    pub const fn new(
        namespace: &'static str,
        entries: &'static [(&'static str, &'static [u8])],
        source: &'static str,
    ) -> Self {
        address::valid_name(namespace);
        Self {
            namespace,
            entries,
            source,
        }
    }
}

#[doc(hidden)]
#[linkme::distributed_slice]
pub static FAIRWAY_CATALOG_NAMESPACES: [Registration];

#[doc(hidden)]
#[linkme::distributed_slice]
pub static FAIRWAY_CATALOG_BUILTINS: [Builtin];

/// Checks every namespace and builtin table, then fills all namespaces or none.
pub(crate) fn initialize(namespaces: &[Registration], builtins: &[Builtin]) -> Result<(), Error> {
    let mut problems = Vec::new();

    let mut namespaces: Vec<_> = namespaces.iter().collect();
    namespaces
        .sort_unstable_by_key(|registration| (registration.namespace.name(), registration.source));
    let mut declared = Vec::new();
    for group in namespaces.chunk_by(|left, right| left.namespace.name() == right.namespace.name())
    {
        if group.len() > 1 {
            problems.push(format!(
                "duplicate namespace {:?}: {}",
                group[0].namespace.name(),
                sources(group.iter().map(|registration| registration.source)),
            ));
        }
        declared.push(group[0]);
    }

    let mut builtins: Vec<_> = builtins.iter().collect();
    builtins.sort_unstable_by_key(|builtin| (builtin.namespace, builtin.source));
    for builtin in &builtins {
        match declared.binary_search_by_key(&builtin.namespace, |registration| {
            registration.namespace.name()
        }) {
            Ok(index) if declared[index].namespace.builtin() => {}
            Ok(_) => problems.push(format!(
                "builtin data for namespace {:?} declared without builtin: {}",
                builtin.namespace, builtin.source,
            )),
            Err(_) => problems.push(format!(
                "builtin data for unknown namespace {:?}: {}",
                builtin.namespace, builtin.source,
            )),
        }
    }

    let publish: Vec<_> = declared
        .iter()
        .map(|registration| {
            let name = registration.namespace.name();
            let tables: Vec<_> = builtins
                .iter()
                .copied()
                .filter(|builtin| builtin.namespace == name)
                .collect();
            registration
                .namespace
                .prepare(&tables, registration.source, &mut problems)
        })
        .collect();

    if !problems.is_empty() {
        return Err(Error::new(format!(
            "cannot initialize catalog:\n  {}",
            problems.join("\n  ")
        )));
    }
    for publish in publish {
        publish();
    }
    Ok(())
}

/// Sorts one namespace's initial entries and adds a problem for every invalid
/// or repeated address. `kind` names the way the namespace is filled.
pub(crate) fn check<V>(
    kind: &str,
    namespace: &str,
    entries: &mut [(Cow<'static, str>, &'static str, V)],
    problems: &mut Vec<String>,
) {
    entries.sort_unstable_by(|left, right| (&left.0, left.1).cmp(&(&right.0, right.1)));
    for group in entries.chunk_by(|left, right| left.0 == right.0) {
        let address = address::full(namespace, &group[0].0);
        let sources = sources(group.iter().map(|entry| entry.1));
        if let Err(reason) = address::check(&group[0].0) {
            problems.push(format!("{kind} address {address:?} {reason}: {sources}"));
        }
        if group.len() > 1 {
            problems.push(format!("duplicate {kind} address {address:?}: {sources}"));
        }
    }
}

/// Joins the sorted sources of one name or address, each once.
fn sources(sources: impl Iterator<Item = &'static str>) -> String {
    let mut sources: Vec<_> = sources.collect();
    sources.dedup();
    sources.join(" and ")
}
