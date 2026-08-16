//! The user's settings: everything about fairway itself that is
//! not policy. Materialized from the shipped defaults on first use
//! and owned by the user from then on.

#[cfg(test)]
mod tests;

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The settings shipped with the binary. Kept as data, not code:
/// this file is the source of truth for every default.
pub const DEFAULT: &str = include_str!("../assets/config.toml");

/// The whole settings file. The schema is strict on purpose: every
/// key is required and unknown keys are refused, so a typo or a
/// file left behind by another version fails loudly instead of
/// silently meaning something else.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Skill locations.
    pub skills: Skills,
}

impl Config {
    /// Parse and validate a settings file's text; `path` names the
    /// file in the error.
    ///
    /// # Errors
    ///
    /// The text is not TOML, does not match the schema, or sets
    /// `skills.dir` to the empty string.
    pub fn parse(text: &str, path: &Path) -> Result<Config, ConfigError> {
        let config: Config =
            toml::from_str(text).map_err(|source| ConfigError::new(path, source))?;
        if config.skills.dir.is_empty() {
            // Empty would resolve to the fairway home itself, mixing
            // skills with the settings and the ledger.
            let source =
                <toml::de::Error as serde::de::Error>::custom("`skills.dir` must not be empty");
            return Err(ConfigError::new(path, source));
        }
        Ok(config)
    }
}

/// Skill locations.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skills {
    /// The skills directory: an absolute path, `~/` for the OS home
    /// directory, or a path relative to the fairway home.
    pub dir: String,
}

/// A user-owned configuration file — settings or policy — that
/// fairway refuses to run with. Never fixed automatically: the file
/// is the user's.
#[derive(Debug)]
pub struct ConfigError {
    path: PathBuf,
    source: toml::de::Error,
}

impl ConfigError {
    pub(crate) fn new(path: &Path, source: toml::de::Error) -> ConfigError {
        ConfigError {
            path: path.to_path_buf(),
            source,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the file at {} is invalid — fix it, or delete it and fairway will restore \
             the defaults on the next run:\n{}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
