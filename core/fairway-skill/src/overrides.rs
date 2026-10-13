//! Overrides from the `skill` configuration namespace.

use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use fairway_codec::serde::Deserialize;

use crate::{
    Error, ErrorKind, address, document,
    error::{chain, report},
    store::{self, Stored},
};

fairway_config::namespace!(SETTINGS: Settings, "skill");

/// The `[skill]` table: `skills_dir` and files by address.
#[derive(Debug, Default, Deserialize)]
#[serde(crate = "fairway_codec::serde", try_from = "BTreeMap<String, String>")]
pub(crate) struct Settings {
    skills_dir: Option<PathBuf>,
    overrides: Vec<(String, PathBuf)>,
}

impl TryFrom<BTreeMap<String, String>> for Settings {
    type Error = String;

    /// Takes a key with `/` for an address and any other key for a parameter.
    fn try_from(table: BTreeMap<String, String>) -> Result<Self, Self::Error> {
        let mut settings = Self::default();
        let mut problems = Vec::new();
        for (key, value) in table {
            if key.contains('/') {
                if let Err(reason) = address::parse(&key) {
                    problems.push(format!("address {key:?} {reason}"));
                } else if value.is_empty() {
                    problems.push(format!("address {key:?} has an empty path"));
                } else {
                    settings.overrides.push((key, value.into()));
                }
            } else if key == "skills_dir" {
                if value.is_empty() {
                    problems.push("`skills_dir` is empty".to_owned());
                } else {
                    settings.skills_dir = Some(value.into());
                }
            } else {
                problems.push(format!(
                    "unknown key {key:?}: a key is an address, like \
                     \"skill/<namespace>/<name>\", or `skills_dir`"
                ));
            }
        }
        if problems.is_empty() {
            Ok(settings)
        } else {
            Err(problems.join("; "))
        }
    }
}

fn failed(source: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::caused_by(
        ErrorKind::Initialize,
        "cannot load skills".to_owned(),
        source,
    )
}

/// Reads and checks every override, then stores all of them or none.
pub(crate) async fn load() -> Result<(), Error> {
    fairway_catalog::initialize().map_err(failed)?;
    fairway_config::initialize().await.map_err(failed)?;
    let settings = SETTINGS.get();
    if settings.overrides.is_empty() {
        return Ok(());
    }
    let home = fairway_fs::home().await.map_err(failed)?;
    // An absolute path replaces the one it is joined to.
    let directory = home.join(
        settings
            .skills_dir
            .as_deref()
            .unwrap_or(Path::new("skills")),
    );

    let mut problems = Vec::new();
    let mut values: Vec<(address::Address<'static>, Stored)> = Vec::new();
    for (key, path) in &settings.overrides {
        let address = address::parse(key).expect("settings keep only valid addresses");
        let path = directory.join(path);
        let text = match fairway_fs::read::<String>(&path).await {
            Ok(text) => text,
            Err(error) => {
                problems.push(format!("{key:?}: {}", chain(&error)));
                continue;
            }
        };
        let found = document::check(key, address.kind, &text);
        if !found.is_empty() {
            for reason in found {
                problems.push(format!("{key:?} ({}): {reason}", path.display()));
            }
            continue;
        }
        match store::encode(text) {
            Ok(bytes) => values.push((address, Cow::Owned(bytes))),
            Err(error) => problems.push(format!("{key:?} ({}): {}", path.display(), chain(&error))),
        }
    }
    if !problems.is_empty() {
        return Err(Error::new(
            ErrorKind::Initialize,
            report("invalid skill overrides", &problems),
        ));
    }

    for (address, value) in values {
        store::namespace(address.kind)
            .put(address.entry, value)
            .expect("a valid address is a valid catalog address");
    }
    Ok(())
}
