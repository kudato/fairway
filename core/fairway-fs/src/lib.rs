//! Asynchronous filesystem operations for Fairway plugins.
//!
//! Whole-file and streaming writes replace the target atomically. Editing
//! holds a per-path, interprocess lock from opening through replacement.

mod directories;
mod home;
mod lock;
mod platform;
mod reader;
mod temporary;
mod writer;

use std::{
    error::Error,
    future::Future,
    io,
    path::{Path, PathBuf},
};

use fairway_codec::{Decode, Encode};

pub use directories::{DirEntries, DirEntry, canonicalize, exists, ls, metadata, mkdir};
pub use home::home;
pub use reader::{Reader, reader};
pub use temporary::{TempDir, TempFile, temp_dir, temp_file};
pub use writer::{Editor, Writer, editor, writer};

type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// Reads a whole regular file and decodes it as `T`, following symbolic links.
/// Opened non-regular files are rejected with `InvalidInput`; decode errors become `InvalidData`.
pub async fn read<T>(path: impl AsRef<Path>) -> io::Result<T>
where
    T: Decode + Send + 'static,
    T::Error: Into<BoxError>,
{
    let path = absolute(path.as_ref())?;
    let bytes = blocking(move || {
        use std::io::Read;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Opening a FIFO must not wait for a writer before the type check.
            options.custom_flags(libc::O_NONBLOCK);
        }
        let mut file = options.open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
    .await?;
    fairway_compute::run(move || T::decode(&bytes).map_err(decode_error)).await
}

/// Atomically creates or replaces a regular file. A busy path returns `WouldBlock`.
/// Encoding errors become `InvalidInput`; the original file remains unchanged.
pub async fn write<V>(path: impl AsRef<Path>, contents: V) -> io::Result<()>
where
    V: Encode + Send + 'static,
    V::Error: Into<BoxError>,
{
    let mut output = writer(path).await?;
    if let Err(error) = output.write(contents).await {
        output.abort().await;
        return Err(error);
    }
    output.finish().await
}

/// Reads, transforms, and atomically replaces an existing regular file under one lock.
/// Waits asynchronously for earlier edits. The handler is called exactly once.
pub async fn edit<T, E, F, Fut>(path: impl AsRef<Path>, handler: F) -> Result<(), E>
where
    T: Decode + Encode + Send + 'static,
    <T as Decode>::Error: Into<BoxError>,
    <T as Encode>::Error: Into<BoxError>,
    E: From<io::Error>,
    F: FnOnce(T) -> Fut,
    Fut: Future<Output = Result<T, E>> + Send,
{
    let mut output = editor(path).await?;
    let bytes = match output.read_all().await {
        Ok(bytes) => bytes,
        Err(error) => {
            output.abort().await;
            return Err(error.into());
        }
    };
    // The worker owns the editor, including its lock, until decoding ends.
    let (mut output, value) = fairway_compute::run(move || {
        let value = T::decode(&bytes).map_err(decode_error);
        (output, value)
    })
    .await;
    let result = async {
        let value = handler(value?).await?;
        output.write(value).await?;
        Ok::<_, E>(())
    }
    .await;
    if let Err(error) = result {
        output.abort().await;
        return Err(error);
    }
    output.finish().await?;
    Ok(())
}

fn decode_error(error: impl Into<BoxError>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.into())
}

fn encode_error(error: impl Into<BoxError>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, error.into())
}

fn absolute(path: &Path) -> io::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "empty filesystem path",
        ));
    }
    #[cfg(windows)]
    return std::path::absolute(path);
    #[cfg(not(windows))]
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(io::Error::other)?
}

// Drop cannot wait for I/O. Moving ownership into the worker also keeps file
// locks alive until outstanding operations and cleanup have completed.
fn defer(work: impl FnOnce() + Send + 'static) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn_blocking(work);
    } else {
        let _ = std::thread::Builder::new()
            .name("fairway-fs-cleanup".into())
            .spawn(work);
    }
}

/// Application startup hooks, not part of the plugin API.
#[doc(hidden)]
pub mod __private {
    /// Freezes `FAIRWAY_HOME` and removes unused lock files before configuration loads.
    pub fn initialize() -> std::io::Result<()> {
        crate::home::initialize()
    }
}

#[cfg(doctest)]
#[doc = include_str!("../../../docs/ru/plugin-development/fs.md")]
mod guide_ru {}

#[cfg(doctest)]
#[doc = include_str!("../../../docs/en/plugin-development/fs.md")]
mod guide_en {}
