use std::{
    io,
    path::{Path, PathBuf},
};

use crate::{Error, Operation};

/// A temporary file, deleted when it is closed or dropped.
///
/// Created by [`temp_file`]. `TempFile` owns the path, not an open handle:
/// read and write the file through its [`path`](TempFile::path) with any API.
/// [`close`](TempFile::close) deletes the file, waits for the deletion, and
/// reports whether it succeeded. Dropping a `TempFile` deletes the file in
/// the background instead and ignores errors, because a destructor cannot
/// wait for I/O.
pub struct TempFile(Option<tempfile::TempPath>);

/// A temporary directory, deleted with all its contents when it is closed or
/// dropped.
///
/// Created by [`temp_dir`]. [`close`](TempDir::close) deletes the directory,
/// waits for the deletion, and reports whether it succeeded. Dropping a
/// `TempDir` deletes the directory in the background instead and ignores
/// errors, because a destructor cannot wait for I/O.
pub struct TempDir(Option<tempfile::TempDir>);

/// Creates an empty temporary file with a unique name.
///
/// The file is created in the system's temporary directory (see
/// [`std::env::temp_dir`]) with a name that starts with `fairway-`. On Unix,
/// only its owner may read and write it. It is deleted when the returned
/// [`TempFile`] is closed or dropped.
///
/// # Errors
///
/// Returns an error if the file cannot be created, for example because the
/// temporary directory does not exist or is not writable.
///
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() -> Result<(), fairway_fs::Error> {
/// let file = fairway_fs::temp_file().await?;
/// fairway_fs::write(file.path(), "scratch".to_owned()).await?;
/// assert_eq!(fairway_fs::read::<String>(file.path()).await?, "scratch");
/// file.close().await?;
/// # Ok(())
/// # }
/// ```
pub async fn temp_file() -> Result<TempFile, Error> {
    super::blocking(Operation::CreateTemporaryFile, None, || {
        Ok(TempFile(Some(
            tempfile::Builder::new()
                .prefix("fairway-")
                .tempfile()
                .map_err(|source| Error::io(Operation::CreateTemporaryFile, None, source))?
                .into_temp_path(),
        )))
    })
    .await
}

/// Creates an empty temporary directory with a unique name.
///
/// The directory is created in the system's temporary directory (see
/// [`std::env::temp_dir`]) with a name that starts with `fairway-`. On Unix,
/// only its owner has access to it. It is deleted with all its contents when
/// the returned [`TempDir`] is closed or dropped.
///
/// # Errors
///
/// Returns an error if the directory cannot be created, for example because
/// the temporary directory does not exist or is not writable.
///
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() -> Result<(), fairway_fs::Error> {
/// let directory = fairway_fs::temp_dir().await?;
/// let path = directory.path().join("draft.txt");
/// fairway_fs::write(&path, "draft".to_owned()).await?;
///
/// directory.close().await?;
/// assert!(!fairway_fs::exists(&path).await?);
/// # Ok(())
/// # }
/// ```
pub async fn temp_dir() -> Result<TempDir, Error> {
    super::blocking(Operation::CreateTemporaryDirectory, None, || {
        let mut builder = tempfile::Builder::new();
        builder.prefix("fairway-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        Ok(TempDir(Some(builder.tempdir().map_err(|source| {
            Error::io(Operation::CreateTemporaryDirectory, None, source)
        })?)))
    })
    .await
}

impl TempFile {
    /// Returns the path of the file.
    ///
    /// A copy of the path does not keep the file alive: the file is deleted
    /// when the `TempFile` is closed or dropped.
    pub fn path(&self) -> &Path {
        self.0.as_ref().expect("live temporary file").as_ref()
    }

    /// Deletes the file and waits until the deletion has finished.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be deleted, for example
    /// [`NotFound`](io::ErrorKind::NotFound) if it has already been removed.
    /// On Windows, a file that is still open may fail to be deleted, so close
    /// the handles to it first, including those of child processes.
    pub async fn close(mut self) -> Result<(), Error> {
        let path = self.0.take().expect("live temporary file");
        let target = path.to_path_buf();
        super::blocking(Operation::RemoveFile, Some(target.clone()), move || {
            path.close()
                .map_err(|source| Error::io(Operation::RemoveFile, Some(&target), source))
        })
        .await
    }
}

impl AsRef<Path> for TempFile {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            super::defer(move || drop(path));
        }
    }
}

impl TempDir {
    /// Returns the path of the directory.
    ///
    /// A copy of the path does not keep the directory alive: the directory is
    /// deleted when the `TempDir` is closed or dropped.
    pub fn path(&self) -> &Path {
        self.0.as_ref().expect("live temporary directory").path()
    }

    /// Deletes the directory with all its contents and waits until the
    /// deletion has finished.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory or something in it cannot be deleted.
    /// On Windows, files that are still open may fail to be deleted, so close
    /// the handles to them first, including those of child processes.
    pub async fn close(mut self) -> Result<(), Error> {
        let directory = self.0.take().expect("live temporary directory");
        let path = directory.path().to_owned();
        super::blocking(Operation::RemoveDirectory, Some(path.clone()), move || {
            directory
                .close()
                .map_err(|source| Error::io(Operation::RemoveDirectory, Some(&path), source))
        })
        .await
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Some(directory) = self.0.take() {
            super::defer(move || drop(directory));
        }
    }
}

/// Creates the temporary file for replacing `target`, in the same directory so
/// that it can be renamed over the target.
///
/// The name starts with `.fairway-`. On Unix, the mode is `0o666`, reduced by
/// the umask, if `new_file` is set, so that a new target gets the permissions
/// of a file created with [`File::create`](std::fs::File::create). Otherwise it
/// is `0o600`, so that the new contents stay private until the permissions of
/// the file being replaced are copied.
///
/// The handle and the path are returned separately because dropping a
/// [`NamedTempFile`](tempfile::NamedTempFile) deletes the file on the spot;
/// the transaction closes the handle and deletes the path on a blocking
/// thread, before it releases the lock.
pub(crate) fn adjacent(
    target: &Path,
    new_file: bool,
) -> io::Result<(std::fs::File, tempfile::TempPath)> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(".fairway-");
    let directory: PathBuf = target.parent().expect("absolute target with parent").into();
    Ok(builder
        .make_in(directory, |path| {
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(if new_file { 0o666 } else { 0o600 });
            }
            #[cfg(not(unix))]
            let _ = new_file;
            options.open(path)
        })?
        .into_parts())
}
