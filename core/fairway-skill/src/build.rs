//! Builtin skills and references, checked and compressed by build scripts.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    BuildError,
    address::{self, Kind},
    document,
    error::{chain, report},
    store,
};

/// A builtin skill or reference for [`build`](fn@build): its address and file.
#[derive(Clone, Debug)]
pub struct Entry {
    address: String,
    path: PathBuf,
}

impl Entry {
    /// Gives the file at `path` the address `address`.
    ///
    /// The address is `skill/<namespace>/<name>` or
    /// `reference/<namespace>/<name>`; `build` checks it. A relative path
    /// starts at the root of the crate, the directory of its `Cargo.toml`.
    pub fn new(address: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            address: address.into(),
            path: path.into(),
        }
    }
}

/// Checks a crate's builtin skills and references and prepares them for
/// [`embed!`](crate::embed).
///
/// Call it once from the crate's build script, with the `build` feature
/// enabled in the build dependency, and list every builtin of every namespace
/// of the crate. The crate declares each of these namespaces with
/// [`namespace!`](crate::namespace) and includes the result with `embed!`.
///
/// Every file is checked as [`initialize`](crate::initialize) checks
/// overrides: a skill has frontmatter with only a nonempty `description`, a
/// reference has no frontmatter, and the template of either compiles. Each
/// file is compressed; Cargo runs the build script again when one of them
/// changes.
///
/// ```no_run
/// // build.rs
/// use fairway_skill::Entry;
///
/// fn main() -> Result<(), fairway_skill::BuildError> {
///     fairway_skill::build([
///         Entry::new("skill/alpha/example-skill", "skills/example-skill.md"),
///         Entry::new(
///             "reference/alpha/example-reference",
///             "skills/references/example-reference.md",
///         ),
///         Entry::new("skill/beta/other-skill", "skills/other-skill.md"),
///     ])
/// }
/// ```
///
/// # Errors
///
/// Returns an error listing every invalid or repeated address, unreadable
/// file, and invalid skill or reference, with its address and file. Returns
/// an error if Cargo's `CARGO_MANIFEST_DIR` or `OUT_DIR` is absent or if the
/// artifacts cannot be written. Return it from the build script's `main`:
/// Cargo then shows the message and stops the build.
pub fn build(entries: impl IntoIterator<Item = Entry>) -> Result<(), BuildError> {
    let entries: Vec<_> = entries.into_iter().collect();
    let root = cargo_directory("CARGO_MANIFEST_DIR")?;
    let out_dir = cargo_directory("OUT_DIR")?;
    for entry in &entries {
        println!(
            "cargo::rerun-if-changed={}",
            root.join(&entry.path).display()
        );
    }
    let builtins = prepare(&root, &entries)?;
    write(builtins, &out_dir)
}

fn cargo_directory(variable: &str) -> Result<PathBuf, BuildError> {
    env::var_os(variable)
        .map(PathBuf::from)
        .ok_or_else(|| BuildError::new(format!("building skills requires Cargo's {variable}")))
}

/// The checked and compressed builtins of a crate.
#[derive(Debug, Default)]
struct Builtins {
    skills: Vec<(String, Vec<u8>)>,
    references: Vec<(String, Vec<u8>)>,
    namespaces: BTreeSet<String>,
}

fn prepare(root: &Path, entries: &[Entry]) -> Result<Builtins, BuildError> {
    let mut builtins = Builtins::default();
    let mut problems = Vec::new();
    let mut paths = BTreeMap::new();
    for entry in entries {
        let key = &entry.address;
        let path = entry.path.display();
        let address = match address::parse(key) {
            Ok(address) => address,
            Err(reason) => {
                problems.push(format!("{key:?} ({path}): the address {reason}"));
                continue;
            }
        };
        if let Some(previous) = paths.insert(key, &entry.path) {
            problems.push(format!(
                "duplicate address {key:?}: {} and {path}",
                previous.display()
            ));
            continue;
        }
        let text = match fs::read_to_string(root.join(&entry.path)) {
            Ok(text) => text,
            Err(error) => {
                problems.push(format!("{key:?} ({path}): cannot read the file: {error}"));
                continue;
            }
        };
        let found = document::check(key, address.kind, &text);
        if !found.is_empty() {
            for reason in found {
                problems.push(format!("{key:?} ({path}): {reason}"));
            }
            continue;
        }
        let bytes = match store::encode(text) {
            Ok(bytes) => bytes,
            Err(error) => {
                problems.push(format!("{key:?} ({path}): {}", chain(&error)));
                continue;
            }
        };
        let table = match address.kind {
            Kind::Skill => &mut builtins.skills,
            Kind::Reference => &mut builtins.references,
        };
        table.push((address.entry.to_owned(), bytes));
        builtins.namespaces.insert(address.namespace.to_owned());
    }
    if problems.is_empty() {
        Ok(builtins)
    } else {
        Err(BuildError::new(report("cannot build skills", &problems)))
    }
}

fn write(builtins: Builtins, out_dir: &Path) -> Result<(), BuildError> {
    for (kind, table) in [
        (Kind::Skill, builtins.skills),
        (Kind::Reference, builtins.references),
    ] {
        fairway_catalog::build(kind.as_str(), table).map_err(|error| {
            BuildError::caused_by(format!("cannot write builtin {kind}s"), error)
        })?;
    }

    // Joined as text, like the path that `embed!` includes.
    let mut directory = OsString::from(out_dir);
    directory.push("/fairway-skill");
    let directory = PathBuf::from(directory);
    fs::create_dir_all(&directory).map_err(|source| {
        BuildError::caused_by(
            format!(
                "cannot create skill artifact directory {}",
                directory.display()
            ),
            source,
        )
    })?;
    let path = directory.join("namespaces.rs");
    fs::write(&path, namespaces(&builtins.namespaces)).map_err(|source| {
        BuildError::caused_by(
            format!("cannot write skill artifact {}", path.display()),
            source,
        )
    })
}

/// The namespaces as the Rust expression that `embed!` includes.
fn namespaces(namespaces: &BTreeSet<String>) -> String {
    let literals: Vec<_> = namespaces.iter().map(|name| format!("{name:?}")).collect();
    format!("&[{}]\n", literals.join(", "))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::{Entry, namespaces, prepare};
    use crate::store;

    fn files(root: &Path, files: &[(&str, &str)]) {
        for (path, text) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    #[test]
    fn builtins_are_compressed_by_kind_with_their_namespaces() {
        let root = tempfile::tempdir().unwrap();
        let example = "---\ndescription: Lorem ipsum.\n---\n# Lorem ipsum\n";
        files(
            root.path(),
            &[
                ("skills/example-skill.md", example),
                ("skills/example-reference.md", "# Dolor sit\n"),
                ("other-skill.md", "---\ndescription: Consectetur.\n---\n"),
            ],
        );
        // Skills keep the order of the entries; namespaces are sorted.
        let builtins = prepare(
            root.path(),
            &[
                Entry::new("skill/beta/other-skill", "other-skill.md"),
                Entry::new(
                    "reference/alpha/example-reference",
                    "skills/example-reference.md",
                ),
                Entry::new("skill/alpha/example-skill", "skills/example-skill.md"),
            ],
        )
        .unwrap();
        let skills: Vec<_> = builtins
            .skills
            .iter()
            .map(|(address, bytes)| (address.as_str(), store::decode(bytes.clone().into())))
            .collect();
        assert_eq!(
            skills,
            [
                (
                    "beta/other-skill",
                    "---\ndescription: Consectetur.\n---\n".to_owned()
                ),
                ("alpha/example-skill", example.to_owned()),
            ]
        );
        assert_eq!(builtins.references.len(), 1);
        assert_eq!(builtins.references[0].0, "alpha/example-reference");
        assert_eq!(
            store::decode(builtins.references[0].1.clone().into()),
            "# Dolor sit\n"
        );
        assert_eq!(namespaces(&builtins.namespaces), "&[\"alpha\", \"beta\"]\n");
        assert_eq!(namespaces(&Default::default()), "&[]\n");
    }

    #[test]
    fn every_problem_is_reported_with_its_address_and_file() {
        let root = tempfile::tempdir().unwrap();
        files(
            root.path(),
            &[
                ("ok.md", "---\ndescription: x\n---\n"),
                ("named.md", "---\nname: x\ndescription: x\n---\n"),
                ("plain.md", "Text\n"),
                ("front.md", "---\ndescription: x\n---\n"),
                ("syntax.md", "{{ \n"),
            ],
        );
        let error = prepare(
            root.path(),
            &[
                Entry::new("skill/alpha", "ok.md"),
                Entry::new("skill/alpha/ok", "ok.md"),
                Entry::new("skill/alpha/ok", "named.md"),
                Entry::new("skill/alpha/named", "named.md"),
                Entry::new("skill/alpha/plain", "plain.md"),
                Entry::new("reference/alpha/front", "front.md"),
                Entry::new("reference/alpha/syntax", "syntax.md"),
                Entry::new("reference/alpha/missing", "missing.md"),
            ],
        )
        .unwrap_err()
        .to_string();
        // A problem's further lines are indented below it.
        let lines: Vec<_> = error
            .lines()
            .filter(|line| !line.starts_with("    "))
            .collect();
        assert_eq!(lines.len(), 8, "{error}");
        assert_eq!(lines[0], "cannot build skills:");
        assert_eq!(
            lines[1],
            "  \"skill/alpha\" (ok.md): the address must have three parts: \
             `skill/<namespace>/<name>` or `reference/<namespace>/<name>`"
        );
        assert_eq!(
            lines[2],
            "  duplicate address \"skill/alpha/ok\": ok.md and named.md"
        );
        assert!(
            lines[3].starts_with("  \"skill/alpha/named\" (named.md): has invalid frontmatter: "),
            "{error}"
        );
        assert_eq!(
            lines[4],
            "  \"skill/alpha/plain\" (plain.md): has no frontmatter: \
             a skill starts with a `---` line"
        );
        assert_eq!(
            lines[5],
            "  \"reference/alpha/front\" (front.md): \
             starts with frontmatter, which a reference cannot have"
        );
        assert!(
            lines[6].starts_with(
                "  \"reference/alpha/syntax\" (syntax.md): \
                 reference/alpha/syntax:1: cannot compile template: "
            ),
            "{error}"
        );
        assert!(
            lines[7]
                .starts_with("  \"reference/alpha/missing\" (missing.md): cannot read the file: "),
            "{error}"
        );
    }
}
