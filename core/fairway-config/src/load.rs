use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

use fairway_codec::Toml;
use fairway_compute as compute;
use fairway_fs as fs;

use crate::{
    Error, Values,
    registry::{self, Registration},
};

pub(crate) async fn registered() -> Result<Values, Error> {
    let namespaces = registry::checked()?;
    let home = fs::home()
        .await
        .map_err(|source| Error::caused_by("could not locate configuration", source))?;
    load(&home, namespaces).await
}

async fn load(home: &Path, namespaces: Vec<&'static Registration>) -> Result<Values, Error> {
    let mut document = Document::default();
    let primary = home.join("config.toml");
    if let Some(table) = read(&primary, true).await? {
        document = merge(document, table, primary, namespaces.clone()).await?;
    }
    for path in additional(&home.join("conf.d")).await? {
        let table = read(&path, false).await?.expect("required file");
        document = merge(document, table, path, namespaces.clone()).await?;
    }
    compute::run(move || document.prepare(&namespaces)).await
}

async fn read(path: &Path, optional: bool) -> Result<Option<toml::Table>, Error> {
    match fs::read::<Toml<toml::Table>>(path).await {
        Ok(table) => Ok(Some(table.into_inner())),
        Err(error) if optional && error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::file(path, error)),
    }
}

struct Directory {
    path: PathBuf,
    identity: PathBuf,
    entries: fs::DirEntries,
}

async fn directory(path: &Path, optional: bool) -> Result<Option<Directory>, Error> {
    let entries = match fs::ls(path).await {
        Ok(entries) => entries,
        Err(error) if optional && error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::file(path, error)),
    };
    let identity = fs::canonicalize(path)
        .await
        .map_err(|error| Error::file(path, error))?;
    Ok(Some(Directory {
        path: path.to_owned(),
        identity,
        entries,
    }))
}

async fn additional(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let Some(root) = directory(root, true).await? else {
        return Ok(Vec::new());
    };
    let mut stack = vec![root];
    let mut paths = Vec::new();
    while let Some(current) = stack.last_mut() {
        let Some(entry) = current
            .entries
            .next()
            .await
            .map_err(|error| Error::file(&current.path, error))?
        else {
            stack.pop();
            continue;
        };
        let path = entry.path();
        let kind = entry
            .file_type()
            .await
            .map_err(|error| Error::file(&path, error))?;
        let kind = if kind.is_symlink() {
            fs::metadata(&path)
                .await
                .map_err(|error| Error::file(&path, error))?
                .file_type()
        } else {
            kind
        };
        if kind.is_dir() {
            let child = directory(&path, false).await?.expect("required directory");
            // Only ancestors form cycles. Separate aliases of one directory
            // deliberately contribute their files under each logical path.
            if stack
                .iter()
                .any(|ancestor| ancestor.identity == child.identity)
            {
                return Err(Error::message(format!(
                    "directory link cycle at {}",
                    path.display()
                )));
            }
            stack.push(child);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            if !kind.is_file() {
                return Err(Error::message(format!(
                    "{} is not a regular file",
                    path.display()
                )));
            }
            paths.push(path);
        }
    }
    // Path ordering compares components. All paths share the same conf.d prefix.
    Ok(compute::run(move || {
        paths.sort();
        paths
    })
    .await)
}

#[derive(Default)]
struct Document {
    tables: toml::Table,
    sources: BTreeMap<&'static str, Vec<PathBuf>>,
}

async fn merge(
    mut document: Document,
    mut table: toml::Table,
    path: PathBuf,
    namespaces: Vec<&'static Registration>,
) -> Result<Document, Error> {
    compute::run(move || {
        for namespace in namespaces {
            if let Some(value) = table.remove(namespace.name) {
                if !value.is_table() {
                    return Err(Error::message(format!(
                        "{}: configuration namespace {:?} must be a table",
                        path.display(),
                        namespace.name,
                    )));
                }
                document
                    .sources
                    .entry(namespace.name)
                    .or_default()
                    .push(path.clone());
                match document.tables.get_mut(namespace.name) {
                    Some(previous) => overlay(previous, value),
                    None => {
                        document.tables.insert(namespace.name.to_owned(), value);
                    }
                }
            }
        }
        Ok(document)
    })
    .await
}

fn overlay(previous: &mut toml::Value, later: toml::Value) {
    match (previous, later) {
        (toml::Value::Table(previous), toml::Value::Table(later)) => {
            for (key, value) in later {
                match previous.get_mut(&key) {
                    Some(previous) => overlay(previous, value),
                    None => {
                        previous.insert(key, value);
                    }
                }
            }
        }
        (previous, later) => *previous = later,
    }
}

impl Document {
    fn prepare(mut self, namespaces: &[&Registration]) -> Result<Values, Error> {
        let mut values = Values::new();
        for namespace in namespaces {
            let value = match self.tables.remove(namespace.name) {
                Some(table) => (namespace.deserialize)(table).map_err(|source| {
                    let paths = self.sources[namespace.name]
                        .iter()
                        .map(|path| format!("\n  {}", path.display()))
                        .collect::<String>();
                    Error::caused_by(
                        format!(
                            "invalid settings in namespace {:?}, assembled from:{paths}",
                            namespace.name,
                        ),
                        source,
                    )
                })?,
                None => (namespace.default)(),
            };
            values.insert(namespace.name, value);
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests;
