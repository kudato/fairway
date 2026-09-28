use std::{
    ffi::OsString,
    fs::{FileType, Metadata},
    io,
    path::{Path, PathBuf},
};

use crate::{Error, Operation};

/// Creates a directory together with its missing parent directories.
///
/// A directory that already exists is not an error, so `mkdir` can be called
/// every time before the directory is used.
///
/// # Errors
///
/// Returns an error if a directory cannot be created, for example
/// [`PermissionDenied`](io::ErrorKind::PermissionDenied) if the process may not
/// create it, or if `path` or one of its parents exists and is not a
/// directory. An empty `path` is [`NotFound`](io::ErrorKind::NotFound).
///
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() -> Result<(), fairway_fs::Error> {
/// let directory = fairway_fs::temp_dir().await?;
/// let path = directory.path().join("cache").join("images");
///
/// fairway_fs::mkdir(&path).await?;
/// fairway_fs::mkdir(&path).await?;
/// assert!(fairway_fs::metadata(&path).await?.is_dir());
/// # Ok(())
/// # }
/// ```
pub async fn mkdir(path: impl AsRef<Path>) -> Result<(), Error> {
    let path = super::absolute(path.as_ref())?;
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(|source| Error::io(Operation::CreateDirectory, Some(&path), source))
}

/// Returns whether `path` refers to an existing file or directory.
///
/// Symbolic links are followed, so a link whose target does not exist gives
/// `false`. Like [`std::fs::exists`] and unlike [`Path::exists`], `exists`
/// does not turn every error into `false`: a path that cannot be checked is
/// not reported as missing.
///
/// # Errors
///
/// Returns the errors of [`metadata`], except that
/// [`NotFound`](io::ErrorKind::NotFound) gives `Ok(false)`. For example, the
/// error is [`PermissionDenied`](io::ErrorKind::PermissionDenied) if a parent
/// directory may not be searched.
///
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() -> Result<(), fairway_fs::Error> {
/// let directory = fairway_fs::temp_dir().await?;
/// let path = directory.path().join("notes.txt");
/// assert!(!fairway_fs::exists(&path).await?);
///
/// fairway_fs::write(&path, String::new()).await?;
/// assert!(fairway_fs::exists(&path).await?);
/// # Ok(())
/// # }
/// ```
pub async fn exists(path: impl AsRef<Path>) -> Result<bool, Error> {
    match metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Returns the metadata of the file or directory at `path`.
///
/// Symbolic links are followed, so the metadata describes the file that a
/// link points to.
///
/// # Errors
///
/// Returns an error if the metadata cannot be read, for example
/// [`NotFound`](io::ErrorKind::NotFound) if `path` does not exist or is empty,
/// or [`PermissionDenied`](io::ErrorKind::PermissionDenied) if a parent
/// directory may not be searched.
///
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() -> Result<(), fairway_fs::Error> {
/// let directory = fairway_fs::temp_dir().await?;
/// let path = directory.path().join("notes.txt");
/// fairway_fs::write(&path, "hello".to_owned()).await?;
///
/// let metadata = fairway_fs::metadata(&path).await?;
/// assert!(metadata.is_file());
/// assert_eq!(metadata.len(), 5);
/// # Ok(())
/// # }
/// ```
pub async fn metadata(path: impl AsRef<Path>) -> Result<Metadata, Error> {
    let path = super::absolute(path.as_ref())?;
    tokio::fs::metadata(&path)
        .await
        .map_err(|source| Error::io(Operation::Metadata, Some(&path), source))
}

/// Returns the canonical form of `path`: an absolute path in which all
/// symbolic links and all `.` and `..` components are resolved.
///
/// Every component of `path` must exist. On Windows, the result uses the
/// extended-length `\\?\` syntax, as with [`std::fs::canonicalize`].
///
/// # Errors
///
/// Returns an error if `path` cannot be resolved, for example
/// [`NotFound`](io::ErrorKind::NotFound) if one of its components does not
/// exist.
///
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() -> Result<(), fairway_fs::Error> {
/// let directory = fairway_fs::temp_dir().await?;
/// let nested = directory.path().join("a").join("b");
/// fairway_fs::mkdir(&nested).await?;
///
/// let canonical = fairway_fs::canonicalize(nested.join("..")).await?;
/// assert!(canonical.is_absolute());
/// assert_eq!(canonical.file_name(), Some("a".as_ref()));
/// # Ok(())
/// # }
/// ```
pub async fn canonicalize(path: impl AsRef<Path>) -> Result<PathBuf, Error> {
    let path = super::absolute(path.as_ref())?;
    super::blocking(Operation::Canonicalize, Some(path.clone()), move || {
        std::fs::canonicalize(&path)
            .map_err(|source| Error::io(Operation::Canonicalize, Some(&path), source))
    })
    .await
}

/// Starts listing the entries of the directory at `path`.
///
/// The returned [`DirEntries`] yields every entry of the directory, including
/// hidden ones, but not `.` and `..`. Subdirectories are not entered, and the
/// order of the entries is whatever the operating system returns, so sort
/// them if the order matters. Symbolic links in `path` are followed.
///
/// # Errors
///
/// Returns an error if the directory cannot be opened, for example
/// [`NotFound`](io::ErrorKind::NotFound) if it does not exist, or if `path` is
/// not a directory.
///
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() -> Result<(), fairway_fs::Error> {
/// let directory = fairway_fs::temp_dir().await?;
/// fairway_fs::write(directory.path().join("a.txt"), String::new()).await?;
/// fairway_fs::mkdir(directory.path().join("b")).await?;
///
/// let mut names = Vec::new();
/// let mut entries = fairway_fs::ls(directory.path()).await?;
/// while let Some(entry) = entries.next().await? {
///     names.push(entry.file_name());
/// }
/// names.sort();
/// assert_eq!(names, ["a.txt", "b"]);
/// # Ok(())
/// # }
/// ```
pub async fn ls(path: impl AsRef<Path>) -> Result<DirEntries, Error> {
    let path = super::absolute(path.as_ref())?;
    let inner = tokio::fs::read_dir(&path)
        .await
        .map_err(|source| Error::io(Operation::ReadDirectory, Some(&path), source))?;
    Ok(DirEntries { inner, path })
}

/// The entries of a directory, returned by [`ls`].
///
/// Call [`next`](DirEntries::next) until it returns `None`.
pub struct DirEntries {
    inner: tokio::fs::ReadDir,
    path: PathBuf,
}

impl DirEntries {
    /// Returns the next entry, or `None` once all entries have been returned.
    ///
    /// # Errors
    ///
    /// Returns an error if the operating system fails to read the directory.
    pub async fn next(&mut self) -> Result<Option<DirEntry>, Error> {
        self.inner
            .next_entry()
            .await
            .map(|entry| entry.map(DirEntry))
            .map_err(|source| Error::io(Operation::ReadDirectory, Some(&self.path), source))
    }
}

/// An entry of a directory, yielded by [`DirEntries`].
pub struct DirEntry(tokio::fs::DirEntry);

impl DirEntry {
    /// Returns the path of the entry: the directory passed to [`ls`], made
    /// absolute, joined with the entry's [name](DirEntry::file_name).
    pub fn path(&self) -> PathBuf {
        self.0.path()
    }

    /// Returns the name of the entry, without the path of its directory.
    pub fn file_name(&self) -> OsString {
        self.0.file_name()
    }

    /// Returns the type of the entry.
    ///
    /// A symbolic link is reported as a link and is not followed; pass the
    /// entry's [`path`](DirEntry::path) to [`metadata`] to get the type of the
    /// file it points to. On most platforms the type is already known from
    /// the listing, so this usually does not access the filesystem again.
    ///
    /// # Errors
    ///
    /// Returns an error if the type has to be queried separately and the query
    /// fails, for example because the entry has been removed since it was
    /// listed.
    pub async fn file_type(&self) -> Result<FileType, Error> {
        self.0
            .file_type()
            .await
            .map_err(|source| Error::io(Operation::Metadata, Some(&self.0.path()), source))
    }
}
