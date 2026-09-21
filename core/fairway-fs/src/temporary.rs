use std::{
    io,
    path::{Path, PathBuf},
};

/// A temporary file deleted on close, or in the background on drop.
pub struct TempFile(Option<tempfile::TempPath>);

/// A temporary directory recursively deleted on close, or in the background on drop.
pub struct TempDir(Option<tempfile::TempDir>);

/// Creates an empty, uniquely named file in the system temporary directory.
pub async fn temp_file() -> io::Result<TempFile> {
    super::blocking(|| {
        Ok(TempFile(Some(
            tempfile::Builder::new()
                .prefix("fairway-")
                .tempfile()?
                .into_temp_path(),
        )))
    })
    .await
}

/// Creates an empty, uniquely named directory in the system temporary directory.
pub async fn temp_dir() -> io::Result<TempDir> {
    super::blocking(|| {
        Ok(TempDir(Some(
            tempfile::Builder::new().prefix("fairway-").tempdir()?,
        )))
    })
    .await
}

impl TempFile {
    /// Returns the file path. Copying it does not extend the resource's lifetime.
    pub fn path(&self) -> &Path {
        self.0.as_ref().expect("live temporary file").as_ref()
    }

    /// Deletes the file and waits for completion. Close its readers and writers first.
    pub async fn close(mut self) -> io::Result<()> {
        let path = self.0.take().expect("live temporary file");
        super::blocking(move || path.close()).await
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
    /// Returns the directory path. Copying it does not extend the resource's lifetime.
    pub fn path(&self) -> &Path {
        self.0.as_ref().expect("live temporary directory").path()
    }

    /// Recursively deletes the directory and waits for completion.
    pub async fn close(mut self) -> io::Result<()> {
        let directory = self.0.take().expect("live temporary directory");
        super::blocking(move || directory.close()).await
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

// NamedTempFile's destructor performs synchronous I/O. Keep the path separate
// from an active writer and dispose of it in the same worker as the file lock.
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
