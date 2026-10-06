use std::path::Path;

use fairway_codec::{
    Decode, Encode, Toml,
    serde::Serialize,
    toml::{Table, Value},
};
use fairway_fs as fs;

use crate::{Error, load};

pub(super) const FILE: &str = "config.auto.toml";
const HEADER: &str = "# This file is managed automatically by Fairway.\n\
                     # Do not edit it manually.\n\n";

struct Automatic(Table);

impl Decode for Automatic {
    type Error = fairway_codec::Error;

    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        Toml::<Table>::decode(bytes).map(|document| Self(document.into_inner()))
    }
}

impl Encode for Automatic {
    type Error = fairway_codec::Error;

    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        let mut bytes = HEADER.as_bytes().to_vec();
        bytes.extend(Toml(self.0).encode()?);
        Ok(bytes)
    }
}

pub(super) async fn create(home: &Path) -> Result<(), Error> {
    let path = home.join(FILE);
    let exists = fs::exists(&path)
        .await
        .map_err(|source| Error::file(&path, source))?;
    if !exists {
        fs::mkdir(home).await.map_err(|source| {
            Error::caused_by(format!("could not create {}", home.display()), source)
        })?;
        fs::write(&path, Automatic(Table::new()))
            .await
            .map_err(|source| {
                Error::caused_by(format!("could not create {}", path.display()), source)
            })?;
    }
    Ok(())
}

pub(super) async fn set<V: Serialize>(
    namespace: &'static str,
    path: &[&str],
    value: V,
) -> Result<(), Error> {
    let Some((key, parents)) = path.split_last() else {
        return Err(Error::message("a setting path cannot be empty"));
    };
    let value = Value::try_from(value).map_err(|source| {
        Error::caused_by(
            format!("could not encode setting {namespace:?} at {path:?}"),
            source,
        )
    })?;
    let home = fs::home()
        .await
        .map_err(|source| Error::caused_by("could not locate configuration", source))?;
    fs::edit(home.join(FILE), |mut automatic: Automatic| async move {
        let mut table = &mut automatic.0;
        for part in std::iter::once(namespace).chain(parents.iter().copied()) {
            table = table
                .entry(part.to_owned())
                .or_insert_with(|| Value::Table(Table::new()))
                .as_table_mut()
                .ok_or_else(|| {
                    EditError(Error::message(format!(
                        "could not set {namespace:?} at {path:?}: \
                         key {part:?} in {} is not a table",
                        home.join(FILE).display(),
                    )))
                })?;
        }
        table.insert((*key).to_owned(), value);
        load::validate_candidate(&home, automatic.0.clone())
            .await
            .map_err(EditError)?;
        Ok::<_, EditError>(automatic)
    })
    .await
    .map_err(|error| error.0)
}

// Adapt fs::edit's error bound without extending the public Error API.
struct EditError(Error);

impl From<fs::Error> for EditError {
    fn from(source: fs::Error) -> Self {
        let message = match source.path() {
            Some(path) => format!("could not edit {}", path.display()),
            None => "could not edit automatic configuration".to_owned(),
        };
        Self(Error::caused_by(message, source))
    }
}
