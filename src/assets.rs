//! Shipped asset bundles: skills today, templates tomorrow. A
//! bundle is a manifest plus an opaque payload; fairway interprets
//! only the manifest and materializes the payload byte for byte.
//! The ledger records every file fairway has written; nothing
//! outside the ledger is ever touched — that is the ownership
//! boundary between fairway's files and the user's.

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The name of the manifest file that marks a directory as a bundle.
pub const MANIFEST_NAME: &str = "manifest.toml";

/// The provenance recorded for bundles shipped inside the binary.
pub const BUILTIN_SOURCE: &str = "builtin";

/// Every bundle shipped inside this binary; filled per release.
/// The twin test keeps it aligned with the `assets/` tree.
pub static REGISTRY: &[Bundle] = &[];

/// The version of this binary, stamped into the ledger it writes.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A shippable unit: a manifest plus opaque payload files.
#[derive(Debug)]
pub struct Bundle {
    /// The embedded `manifest.toml` contents.
    pub manifest: &'static str,
    /// The payload. The manifest itself is not listed here; it is
    /// written alongside.
    pub files: &'static [BundleFile],
}

/// One payload file of a bundle.
#[derive(Debug)]
pub struct BundleFile {
    /// The path relative to the bundle directory, `/`-separated.
    pub path: &'static str,
    /// The embedded contents.
    pub contents: &'static str,
}

/// The bundle metadata fairway interprets; everything else in a
/// bundle is payload.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The bundle name; also its directory name when materialized.
    pub name: String,
    /// A description for catalogs read by agents and humans.
    pub description: String,
}

/// The ledger of everything fairway has materialized: the record
/// that draws the ownership boundary and carries drift detection.
/// Owned by fairway — unlike the user's configuration, its schema
/// may change freely between versions.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    /// The fairway version that wrote the ledger.
    version: String,
    /// One entry per materialized bundle.
    bundles: Vec<Installed>,
}

/// One materialized bundle on record.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Installed {
    name: String,
    source: String,
    /// The written files, relative to the bundle directory; the
    /// manifest is listed like any other.
    files: Vec<String>,
}

/// The outcome of converging the materialized assets with this
/// binary.
#[derive(Debug, PartialEq, Eq)]
pub enum Convergence {
    /// The ledger was created and every registry bundle written.
    Created,
    /// Disk already matches this version; nothing was touched.
    Current,
    /// Disk was materialized by another fairway version; nothing
    /// was touched — updating is an explicit act, never a side
    /// effect.
    Stale {
        /// The fairway version recorded in the ledger.
        installed: String,
    },
}

/// Converge the materialized assets with this binary. On first run
/// (no ledger) every registry bundle is written and the ledger is
/// committed last; a current ledger means nothing to do; a stale
/// one is reported and left alone. Collisions are detected before
/// a single byte lands, so a refusal never leaves a half-written
/// bundle behind.
///
/// # Errors
///
/// An unreadable or invalid ledger, a bundle path already occupied
/// by a file fairway does not own, a registry entry with an
/// unusable name or path, or a filesystem failure.
pub fn converge(
    registry: &[Bundle],
    skills_dir: &Path,
    ledger_path: &Path,
) -> Result<Convergence, AssetsError> {
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| AssetsError::Io { path, source }
    };
    fs::create_dir_all(skills_dir).map_err(io(skills_dir))?;
    if let Some(text) = read_if_exists(ledger_path)? {
        let ledger: Ledger = toml::from_str(&text).map_err(|source| AssetsError::Ledger {
            path: ledger_path.to_path_buf(),
            source,
        })?;
        if ledger.version == VERSION {
            return Ok(Convergence::Current);
        }
        return Ok(Convergence::Stale {
            installed: ledger.version,
        });
    }

    // Reading follows symlinks, so a dangling link at the ledger
    // path answers "absent" while still occupying it; publishing
    // through it would replace the user's link.
    if ledger_path.symlink_metadata().is_ok() {
        return Err(AssetsError::Occupied {
            path: ledger_path.to_path_buf(),
        });
    }

    let mut writes: Vec<(PathBuf, &str)> = Vec::new();
    let mut entries = Vec::new();
    for bundle in registry {
        let manifest: Manifest =
            toml::from_str(bundle.manifest).map_err(|source| AssetsError::Manifest { source })?;
        if !is_plain_relative(&manifest.name) || manifest.name.contains('/') {
            return Err(AssetsError::Packaging {
                detail: format!(
                    "the bundle name `{}` is not a plain directory name",
                    manifest.name
                ),
            });
        }
        let dir = skills_dir.join(&manifest.name);
        let mut files = vec![MANIFEST_NAME.to_owned()];
        writes.push((dir.join(MANIFEST_NAME), bundle.manifest));
        for file in bundle.files {
            if !is_plain_relative(file.path) {
                return Err(AssetsError::Packaging {
                    detail: format!(
                        "the path `{}` in bundle `{}` is not plain and relative",
                        file.path, manifest.name
                    ),
                });
            }
            files.push(file.path.to_owned());
            writes.push((dir.join(file.path), file.contents));
        }
        entries.push(Installed {
            name: manifest.name,
            source: BUILTIN_SOURCE.to_owned(),
            files,
        });
    }
    let mut planned = HashSet::new();
    for (path, _) in &writes {
        if !planned.insert(path) {
            return Err(AssetsError::Packaging {
                detail: format!("two bundles collide at {}", path.display()),
            });
        }
        if path.symlink_metadata().is_ok() {
            return Err(AssetsError::Occupied { path: path.clone() });
        }
    }
    for (path, contents) in &writes {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io(parent))?;
        }
        create_atomically(path, contents).map_err(io(path))?;
    }
    let ledger = Ledger {
        version: VERSION.to_owned(),
        bundles: entries,
    };
    let text = toml::to_string(&ledger).map_err(|source| AssetsError::Encode { source })?;
    create_atomically(ledger_path, &text).map_err(io(ledger_path))?;
    Ok(Convergence::Created)
}

/// A plain `/`-separated relative path: no empty, `.`, or `..`
/// segments, no backslashes. Bundle names and paths are data, and
/// data must not be able to reach outside the skills directory.
fn is_plain_relative(path: &str) -> bool {
    !path.contains('\\')
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

/// Publish `contents` at `path`: written out to a uniquely named
/// sibling temporary file, then moved into place only if `path` is
/// still free. A torn write can never be observed, and an existing
/// file — or a symlink, even a dangling one — is never replaced.
pub(crate) fn create_atomically(path: &Path, contents: &str) -> io::Result<()> {
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    io::Write::write_all(&mut tmp, contents.as_bytes())?;
    tmp.persist_noclobber(path).map_err(|e| e.error)?;
    Ok(())
}

/// Read a file that is allowed to be absent; any other failure is
/// an error.
fn read_if_exists(path: &Path) -> Result<Option<String>, AssetsError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(AssetsError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Why the assets could not be converged.
#[derive(Debug)]
pub enum AssetsError {
    /// A filesystem operation failed.
    Io {
        /// The path the operation targeted.
        path: PathBuf,
        /// The failure.
        source: io::Error,
    },
    /// The ledger did not parse; without it fairway cannot tell its
    /// own files from the user's, so nothing is safe to touch.
    Ledger {
        /// The ledger path.
        path: PathBuf,
        /// The parse failure.
        source: toml::de::Error,
    },
    /// A shipped manifest did not parse — a packaging defect, not a
    /// user problem.
    Manifest {
        /// The parse failure.
        source: toml::de::Error,
    },
    /// The ledger could not be encoded — a defect, not a user
    /// problem.
    Encode {
        /// The encoding failure.
        source: toml::ser::Error,
    },
    /// A bundle name or path the registry must never carry — a
    /// packaging defect, not a user problem.
    Packaging {
        /// What is wrong.
        detail: String,
    },
    /// A path a bundle must occupy already exists and is not on the
    /// ledger: refusing to overwrite what fairway does not own.
    Occupied {
        /// The occupied path.
        path: PathBuf,
    },
}

impl fmt::Display for AssetsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AssetsError::Io { path, source } => {
                write!(f, "could not access {}: {source}", path.display())
            }
            AssetsError::Ledger { path, source } => write!(
                f,
                "the ledger at {} is invalid; it records which files are fairway's, \
                 so nothing was touched:\n{source}",
                path.display()
            ),
            AssetsError::Manifest { source } => {
                write!(f, "a shipped bundle manifest is invalid:\n{source}")
            }
            AssetsError::Encode { source } => {
                write!(f, "the ledger could not be encoded: {source}")
            }
            AssetsError::Packaging { detail } => {
                write!(f, "a shipped bundle is invalid: {detail}")
            }
            AssetsError::Occupied { path } => write!(
                f,
                "{} already exists but is not fairway's; move it away and retry, \
                 or delete it if it is a leftover from an interrupted install",
                path.display()
            ),
        }
    }
}

impl std::error::Error for AssetsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AssetsError::Io { source, .. } => Some(source),
            AssetsError::Ledger { source, .. } | AssetsError::Manifest { source } => Some(source),
            AssetsError::Encode { source } => Some(source),
            AssetsError::Packaging { .. } | AssetsError::Occupied { .. } => None,
        }
    }
}
