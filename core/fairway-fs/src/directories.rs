use std::{
    ffi::OsString,
    fs::{FileType, Metadata},
    io,
    path::{Path, PathBuf},
};

/// Creates a directory and its missing parents. An existing directory is accepted.
pub async fn mkdir(path: impl AsRef<Path>) -> io::Result<()> {
    tokio::fs::create_dir_all(super::absolute(path.as_ref())?).await
}

/// Whether a path resolves to an existing file or directory. Other I/O errors propagate.
pub async fn exists(path: impl AsRef<Path>) -> io::Result<bool> {
    match metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Returns metadata, following symbolic links.
pub async fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    tokio::fs::metadata(super::absolute(path.as_ref())?).await
}

/// Opens an unsorted, nonrecursive directory traversal, including hidden entries.
pub async fn ls(path: impl AsRef<Path>) -> io::Result<DirEntries> {
    tokio::fs::read_dir(super::absolute(path.as_ref())?)
        .await
        .map(DirEntries)
}

/// An open directory traversal.
pub struct DirEntries(tokio::fs::ReadDir);

impl DirEntries {
    /// Returns the next entry or `None` at the end.
    pub async fn next(&mut self) -> io::Result<Option<DirEntry>> {
        self.0.next_entry().await.map(|entry| entry.map(DirEntry))
    }
}

/// The name, path, and type of one directory entry.
pub struct DirEntry(tokio::fs::DirEntry);

impl DirEntry {
    /// Returns the entry's absolute path, fixed when the traversal opened.
    pub fn path(&self) -> PathBuf {
        self.0.path()
    }

    /// Returns the entry's name without its parent path.
    pub fn file_name(&self) -> OsString {
        self.0.file_name()
    }

    /// Returns the type of the entry itself, without following a symbolic link.
    pub async fn file_type(&self) -> io::Result<FileType> {
        self.0.file_type().await
    }
}
