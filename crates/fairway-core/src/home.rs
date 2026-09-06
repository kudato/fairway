//! The fairway home on disk: locating the root, the file layout,
//! and the lazy convergence that materializes what is missing on
//! first use — by creation only, never by overwrite.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::assets::{self, AssetsError, Bundle, Convergence};
use crate::config::{Config, ConfigError};
use crate::policy::GitPolicy;

/// The environment variable that overrides the home root.
pub const HOME_ENV: &str = "FAIRWAY_HOME";

const CONFIG_NAME: &str = "config.toml";
const POLICY_DIR: &str = "policy";
const GIT_POLICY_NAME: &str = "git.toml";
const LEDGER_NAME: &str = "installed.toml";

/// The policy files this binary reads. The set is fixed: policy
/// lives in known files, it is never discovered by scanning.
const KNOWN_POLICY: &[&str] = &[GIT_POLICY_NAME];

/// The fairway home directory: the settings, the policy, the
/// ledger, and the default place for materialized skills.
#[derive(Debug)]
pub struct Home {
    root: PathBuf,
    os_home: Option<PathBuf>,
}

impl Home {
    /// A home rooted at `root`, with no OS home directory attached:
    /// `~` in configured paths will be refused. [`Home::resolve`]
    /// attaches one.
    #[must_use]
    pub fn new(root: PathBuf) -> Home {
        Home {
            root,
            os_home: None,
        }
    }

    /// Locate the home: `$FAIRWAY_HOME` when set and non-empty,
    /// `.fairway` under the OS home directory otherwise.
    ///
    /// # Errors
    ///
    /// Neither is available.
    pub fn resolve() -> Result<Home, HomeError> {
        // Deprecated historically; the behavior was fixed in 1.85
        // (our MSRV) and the attribute lifted in 1.87.
        #[allow(deprecated)]
        let os_home = std::env::home_dir();
        let env = std::env::var_os(HOME_ENV);
        let root = root_from(env.as_deref(), os_home.as_deref()).ok_or(HomeError::Unlocatable)?;
        Ok(Home { root, os_home })
    }

    /// The home root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The settings file path.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.root.join(CONFIG_NAME)
    }

    /// The policy directory path.
    #[must_use]
    pub fn policy_dir(&self) -> PathBuf {
        self.root.join(POLICY_DIR)
    }

    /// The git policy file path.
    #[must_use]
    pub fn git_policy_path(&self) -> PathBuf {
        self.policy_dir().join(GIT_POLICY_NAME)
    }

    /// The ledger path.
    #[must_use]
    pub fn ledger_path(&self) -> PathBuf {
        self.root.join(LEDGER_NAME)
    }

    /// Load the settings and the policy, converging the home with
    /// this binary on the way: whatever does not exist is created —
    /// the root, the settings and policy files from the shipped
    /// defaults, the skills directory, the materialized assets.
    /// Existing files are never overwritten. The returned notes are
    /// human diagnostics bound for stderr.
    ///
    /// # Errors
    ///
    /// An invalid settings, policy, or ledger file, an unknown file
    /// in the policy directory, a dangling symlink where a file
    /// should be, `~` in a configured path without an OS home
    /// directory, or a filesystem failure.
    pub fn converge(&self) -> Result<Converged, HomeError> {
        self.converge_with(assets::REGISTRY)
    }

    /// The testable core of [`Home::converge`], over any registry.
    fn converge_with(&self, registry: &[Bundle]) -> Result<Converged, HomeError> {
        let io = |path: &Path| {
            let path = path.to_path_buf();
            move |source| HomeError::Io { path, source }
        };
        fs::create_dir_all(&self.root).map_err(io(&self.root))?;
        let policy_dir = self.policy_dir();
        fs::create_dir_all(&policy_dir).map_err(io(&policy_dir))?;
        // Scanned before any file is created: a policy file the user
        // misnamed must not be shadowed by recreated defaults.
        refuse_unknown_policy(&policy_dir)?;
        let mut notes = Vec::new();
        let config_path = self.config_path();
        let text = read_or_create(&config_path, crate::config::DEFAULT, &mut notes)?;
        let config = Config::parse(&text, &config_path)?;
        let git_policy_path = self.git_policy_path();
        let text = read_or_create(&git_policy_path, crate::policy::DEFAULT, &mut notes)?;
        let policy = GitPolicy::parse(&text, &git_policy_path)?;
        let skills_dir = resolve_dir(&config.skills.dir, &self.root, self.os_home.as_deref())?;
        match assets::converge(registry, &skills_dir, &self.ledger_path())? {
            Convergence::Created if !registry.is_empty() => {
                notes.push(format!(
                    "materialized the shipped assets into {}",
                    skills_dir.display()
                ));
            }
            Convergence::Created | Convergence::Current => {}
            Convergence::Stale { installed } => {
                notes.push(format!(
                    "the assets on disk were written by fairway {installed} \
                     and are left as they are"
                ));
            }
        }
        Ok(Converged {
            config,
            policy,
            notes,
        })
    }
}

/// What [`Home::converge`] produced.
#[derive(Debug)]
pub struct Converged {
    /// The validated settings.
    pub config: Config,
    /// The validated git policy.
    pub policy: GitPolicy,
    /// Human diagnostics about what convergence did or found.
    pub notes: Vec<String>,
}

/// Read a user-owned file, creating it from `default` when absent;
/// the creation is noted for stderr.
fn read_or_create(
    path: &Path,
    default: &str,
    notes: &mut Vec<String>,
) -> Result<String, HomeError> {
    let io = |source| HomeError::Io {
        path: path.to_path_buf(),
        source,
    };
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // Reading follows symlinks, so a dangling link answers
            // "absent" while still occupying the path; creating the
            // defaults there would replace the user's link.
            if path.symlink_metadata().is_ok() {
                return Err(HomeError::Dangling {
                    path: path.to_path_buf(),
                });
            }
            match assets::create_atomically(path, default) {
                Ok(()) => {
                    notes.push(format!(
                        "created {} with the shipped defaults",
                        path.display()
                    ));
                    Ok(default.to_owned())
                }
                // Another fairway created the file first; it is in
                // force exactly as if it had existed all along.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    fs::read_to_string(path).map_err(io)
                }
                Err(source) => Err(io(source)),
            }
        }
        Err(source) => Err(io(source)),
    }
}

/// Refuse unknown `.toml` files in the policy directory, in the
/// spirit of the strict schemas: a typo in a file name must not
/// silently leave policy inert. The extension is matched without
/// regard to case — `GIT.TOML` is a typo, not a different file.
/// Anything without the extension is none of fairway's business.
fn refuse_unknown_policy(dir: &Path) -> Result<(), HomeError> {
    let io = |source| HomeError::Io {
        path: dir.to_path_buf(),
        source,
    };
    for entry in fs::read_dir(dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        let name = entry.file_name();
        if Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
            && !KNOWN_POLICY.iter().any(|known| name == OsStr::new(known))
        {
            return Err(HomeError::UnknownPolicy { path: entry.path() });
        }
    }
    Ok(())
}

/// The root the home lives at: the environment override when set
/// and non-empty, `.fairway` under the OS home otherwise.
fn root_from(env: Option<&OsStr>, os_home: Option<&Path>) -> Option<PathBuf> {
    match env {
        Some(root) if !root.is_empty() => Some(PathBuf::from(root)),
        _ => os_home.map(|home| home.join(".fairway")),
    }
}

/// Resolve a configured directory: an absolute path stays, `~` and
/// `~/` expand to the OS home, anything else lives under `root`.
fn resolve_dir(dir: &str, root: &Path, os_home: Option<&Path>) -> Result<PathBuf, HomeError> {
    let expand = |rest: Option<&str>| {
        let home = os_home.ok_or_else(|| HomeError::NoOsHome {
            dir: dir.to_owned(),
        })?;
        Ok(match rest {
            Some(rest) => home.join(rest),
            None => home.to_path_buf(),
        })
    };
    if dir == "~" {
        return expand(None);
    }
    if let Some(rest) = dir.strip_prefix("~/") {
        return expand(Some(rest));
    }
    let path = Path::new(dir);
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    })
}

/// Why the home could not be located or converged.
#[derive(Debug)]
pub enum HomeError {
    /// Neither `$FAIRWAY_HOME` nor an OS home directory exists.
    Unlocatable,
    /// A configured path starts with `~`, but there is no OS home
    /// directory to expand it with.
    NoOsHome {
        /// The configured value.
        dir: String,
    },
    /// A filesystem operation under the home failed.
    Io {
        /// The path the operation targeted.
        path: PathBuf,
        /// The failure.
        source: io::Error,
    },
    /// A settings or policy file was refused.
    Config(ConfigError),
    /// A `.toml` file in the policy directory that this binary does
    /// not read.
    UnknownPolicy {
        /// The unknown file.
        path: PathBuf,
    },
    /// A symbolic link that reads as absent because its target is
    /// missing; replacing it with a defaults file would destroy the
    /// user's link.
    Dangling {
        /// The symlink path.
        path: PathBuf,
    },
    /// The assets could not be converged.
    Assets(AssetsError),
}

impl fmt::Display for HomeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HomeError::Unlocatable => write!(
                f,
                "could not locate the fairway home: there is no OS home directory; \
                 set {HOME_ENV}"
            ),
            HomeError::NoOsHome { dir } => {
                write!(f, "cannot expand `{dir}`: there is no OS home directory")
            }
            HomeError::Io { path, source } => {
                write!(f, "could not access {}: {source}", path.display())
            }
            HomeError::Config(e) => write!(f, "{e}"),
            HomeError::UnknownPolicy { path } => write!(
                f,
                "{} is not a policy file fairway knows; check the name for a typo, \
                 or move the file away",
                path.display()
            ),
            HomeError::Dangling { path } => write!(
                f,
                "{} is a symbolic link whose target is missing; restore the target \
                 or remove the link",
                path.display()
            ),
            HomeError::Assets(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for HomeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HomeError::Io { source, .. } => Some(source),
            HomeError::Config(e) => Some(e),
            HomeError::Assets(e) => Some(e),
            HomeError::Unlocatable
            | HomeError::NoOsHome { .. }
            | HomeError::UnknownPolicy { .. }
            | HomeError::Dangling { .. } => None,
        }
    }
}

impl From<ConfigError> for HomeError {
    fn from(e: ConfigError) -> HomeError {
        HomeError::Config(e)
    }
}

impl From<AssetsError> for HomeError {
    fn from(e: AssetsError) -> HomeError {
        HomeError::Assets(e)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::policy::Action;

    #[test]
    fn the_root_prefers_the_environment_override() {
        let root = root_from(Some(OsStr::new("/custom")), Some(Path::new("/home/u")));
        assert_eq!(root, Some(PathBuf::from("/custom")));
    }

    #[test]
    fn an_empty_override_falls_back_to_the_os_home() {
        let root = root_from(Some(OsStr::new("")), Some(Path::new("/home/u")));
        assert_eq!(root, Some(PathBuf::from("/home/u/.fairway")));
    }

    #[test]
    fn no_override_derives_from_the_os_home() {
        let root = root_from(None, Some(Path::new("/home/u")));
        assert_eq!(root, Some(PathBuf::from("/home/u/.fairway")));
    }

    #[test]
    fn no_home_anywhere_is_no_root() {
        assert_eq!(root_from(None, None), None);
        assert_eq!(root_from(Some(OsStr::new("")), None), None);
    }

    #[test]
    fn a_relative_dir_lives_under_the_root() {
        let dir = resolve_dir("skills", Path::new("/r"), None).unwrap();
        assert_eq!(dir, PathBuf::from("/r/skills"));
    }

    #[test]
    fn an_absolute_dir_stays() {
        let dir = resolve_dir("/elsewhere", Path::new("/r"), None).unwrap();
        assert_eq!(dir, PathBuf::from("/elsewhere"));
    }

    #[test]
    fn a_tilde_dir_expands_to_the_os_home() {
        let home = Some(Path::new("/home/u"));
        let dir = resolve_dir("~/sk", Path::new("/r"), home).unwrap();
        assert_eq!(dir, PathBuf::from("/home/u/sk"));
        let bare = resolve_dir("~", Path::new("/r"), home).unwrap();
        assert_eq!(bare, PathBuf::from("/home/u"));
    }

    #[test]
    fn a_tilde_without_an_os_home_is_refused() {
        let error = resolve_dir("~/sk", Path::new("/r"), None).unwrap_err();
        assert!(matches!(error, HomeError::NoOsHome { .. }), "{error:?}");
    }

    /// `~` expands only as a whole first component; `~backup` is a
    /// literal directory name here, not another user's home.
    #[test]
    fn a_tilde_glued_to_a_name_is_literal() {
        let dir = resolve_dir("~backup", Path::new("/r"), None).unwrap();
        assert_eq!(dir, PathBuf::from("/r/~backup"));
    }

    /// Every policy file shipped in `assets/` must be one the
    /// binary reads, and the other way around.
    #[test]
    fn the_known_policy_mirrors_the_assets_tree() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/policy");
        let mut on_disk: Vec<String> = fs::read_dir(assets)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        on_disk.sort();
        let mut known: Vec<String> = KNOWN_POLICY.iter().map(|name| (*name).to_owned()).collect();
        known.sort();
        assert_eq!(on_disk, known);
    }

    #[test]
    fn the_first_run_materializes_the_home() {
        let dir = TempDir::new().unwrap();
        let home = Home::new(dir.path().join("deep").join("home"));
        let converged = home.converge().unwrap();
        assert_eq!(converged.policy.git_available.action, Action::Stop);
        assert_eq!(converged.config.skills.dir, "skills");
        assert_eq!(converged.notes.len(), 2, "{:?}", converged.notes);
        assert_eq!(
            fs::read_to_string(home.config_path()).unwrap(),
            crate::config::DEFAULT
        );
        assert_eq!(
            fs::read_to_string(home.git_policy_path()).unwrap(),
            crate::policy::DEFAULT
        );
        assert!(home.root().join("skills").is_dir());
        assert!(home.ledger_path().is_file());
    }

    #[test]
    fn the_second_run_finds_everything_in_place() {
        let dir = TempDir::new().unwrap();
        let home = Home::new(dir.path().to_path_buf());
        home.converge().unwrap();
        let converged = home.converge().unwrap();
        assert_eq!(converged.notes, Vec::<String>::new());
    }

    #[test]
    fn a_hand_edit_survives_and_is_honored() {
        let dir = TempDir::new().unwrap();
        let home = Home::new(dir.path().to_path_buf());
        home.converge().unwrap();
        let edited = crate::policy::DEFAULT.replacen("value = false", "value = true", 1);
        fs::write(home.git_policy_path(), &edited).unwrap();
        let converged = home.converge().unwrap();
        assert!(converged.policy.git_available.value);
        assert_eq!(fs::read_to_string(home.git_policy_path()).unwrap(), edited);
    }

    /// A broken settings file is the user's to fix or delete; it
    /// must reach them intact, not be repaired into silence.
    #[test]
    fn a_broken_settings_file_is_refused_and_left_alone() {
        let dir = TempDir::new().unwrap();
        let home = Home::new(dir.path().to_path_buf());
        fs::create_dir_all(home.root()).unwrap();
        fs::write(home.config_path(), "not = [toml").unwrap();
        let error = home.converge().unwrap_err();
        assert!(matches!(error, HomeError::Config(_)), "{error:?}");
        assert_eq!(
            fs::read_to_string(home.config_path()).unwrap(),
            "not = [toml"
        );
    }

    #[test]
    fn a_broken_policy_file_is_refused_and_left_alone() {
        let dir = TempDir::new().unwrap();
        let home = Home::new(dir.path().to_path_buf());
        home.converge().unwrap();
        fs::write(home.git_policy_path(), "not = [toml").unwrap();
        let error = home.converge().unwrap_err();
        assert!(matches!(error, HomeError::Config(_)), "{error:?}");
        assert_eq!(
            fs::read_to_string(home.git_policy_path()).unwrap(),
            "not = [toml"
        );
    }

    /// A misnamed policy file must halt convergence before the real
    /// name is recreated from the defaults and shadows it.
    #[test]
    fn an_unknown_policy_file_is_refused_before_anything_is_written() {
        let dir = TempDir::new().unwrap();
        let home = Home::new(dir.path().to_path_buf());
        fs::create_dir_all(home.policy_dir()).unwrap();
        fs::write(home.policy_dir().join("gti.toml"), "").unwrap();
        let error = home.converge().unwrap_err();
        assert!(
            matches!(error, HomeError::UnknownPolicy { .. }),
            "{error:?}"
        );
        assert!(!home.git_policy_path().exists());
        assert!(!home.config_path().exists());
    }

    /// The extension is matched case-insensitively: on a
    /// case-insensitive filesystem `GIT.TOML` shadows `git.toml`,
    /// and on any other it would sit there silently ignored.
    #[test]
    fn an_uppercase_extension_is_still_judged() {
        let dir = TempDir::new().unwrap();
        let home = Home::new(dir.path().to_path_buf());
        fs::create_dir_all(home.policy_dir()).unwrap();
        fs::write(home.policy_dir().join("GIT.TOML"), "").unwrap();
        let error = home.converge().unwrap_err();
        assert!(
            matches!(error, HomeError::UnknownPolicy { .. }),
            "{error:?}"
        );
    }

    /// Only `.toml` files are fairway's to judge; editors and
    /// operating systems drop other files everywhere.
    #[test]
    fn a_non_toml_file_in_the_policy_dir_is_ignored() {
        let dir = TempDir::new().unwrap();
        let home = Home::new(dir.path().to_path_buf());
        home.converge().unwrap();
        fs::write(home.policy_dir().join(".DS_Store"), "junk").unwrap();
        home.converge().unwrap();
    }

    /// The dotfiles way: a symlinked settings file with a live
    /// target is read through, like any other file.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_settings_file_is_followed() {
        let dir = TempDir::new().unwrap();
        let home = Home::new(dir.path().join("home"));
        fs::create_dir_all(home.root()).unwrap();
        let target = dir.path().join("dotfiles-config.toml");
        fs::write(&target, crate::config::DEFAULT).unwrap();
        std::os::unix::fs::symlink(&target, home.config_path()).unwrap();
        let converged = home.converge().unwrap();
        assert_eq!(converged.config.skills.dir, "skills");
    }

    /// A dangling symlink reads as absent but still occupies the
    /// path: writing the defaults there would destroy the user's
    /// link (config in dotfiles, target not yet restored).
    #[cfg(unix)]
    #[test]
    fn a_dangling_settings_symlink_is_refused_and_left_alone() {
        let dir = TempDir::new().unwrap();
        let home = Home::new(dir.path().join("home"));
        fs::create_dir_all(home.root()).unwrap();
        std::os::unix::fs::symlink(dir.path().join("missing"), home.config_path()).unwrap();
        let error = home.converge().unwrap_err();
        assert!(matches!(error, HomeError::Dangling { .. }), "{error:?}");
        let kind = home.config_path().symlink_metadata().unwrap().file_type();
        assert!(kind.is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_policy_symlink_is_refused_and_left_alone() {
        let dir = TempDir::new().unwrap();
        let home = Home::new(dir.path().join("home"));
        fs::create_dir_all(home.policy_dir()).unwrap();
        std::os::unix::fs::symlink(dir.path().join("missing"), home.git_policy_path()).unwrap();
        let error = home.converge().unwrap_err();
        assert!(matches!(error, HomeError::Dangling { .. }), "{error:?}");
        let kind = home
            .git_policy_path()
            .symlink_metadata()
            .unwrap()
            .file_type();
        assert!(kind.is_symlink());
    }

    /// The materialization note names the directory the skills went
    /// into; an empty registry earns no note.
    #[test]
    fn a_materialized_registry_is_noted() {
        static FIXTURE: &[Bundle] = &[Bundle {
            manifest: "name = \"fixture\"\ndescription = \"A bundle for tests.\"\n",
            files: &[],
        }];
        let dir = TempDir::new().unwrap();
        let home = Home::new(dir.path().to_path_buf());
        let converged = home.converge_with(FIXTURE).unwrap();
        assert!(
            converged.notes.iter().any(|n| n.contains("materialized")),
            "{:?}",
            converged.notes
        );
        assert!(home.root().join("skills/fixture/manifest.toml").is_file());
    }

    #[test]
    fn a_stale_ledger_is_reported_untouched() {
        let dir = TempDir::new().unwrap();
        let home = Home::new(dir.path().to_path_buf());
        home.converge().unwrap();
        let old = "version = \"0.0.0\"\nbundles = []\n";
        fs::write(home.ledger_path(), old).unwrap();
        let converged = home.converge().unwrap();
        assert_eq!(converged.notes.len(), 1, "{:?}", converged.notes);
        assert!(
            converged.notes[0].contains("0.0.0"),
            "{:?}",
            converged.notes
        );
        assert_eq!(fs::read_to_string(home.ledger_path()).unwrap(), old);
    }
}
