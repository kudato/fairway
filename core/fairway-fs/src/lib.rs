//! Asynchronous file operations for Fairway plugins.
//!
//! The central functions work with whole files through the codecs of
//! [`fairway_codec`]: [`read`] decodes a file into a value, [`write`] encodes
//! a value and replaces the file with the result, and [`edit`] reads, changes,
//! and replaces a file under a lock, passing the decoded value through an
//! asynchronous handler. The crate also creates and lists directories
//! ([`mkdir`], [`ls`]), inspects paths ([`exists`], [`metadata`],
//! [`canonicalize`]), creates temporary files and directories that are
//! deleted automatically ([`temp_file`], [`temp_dir`]), and reports the
//! Fairway directory ([`home`]).
//!
//! Every operation is `async` and must run inside a [Tokio](tokio) runtime.
//! Filesystem calls, decoding, and encoding run on Tokio's blocking thread
//! pool, so they never block the threads that run asynchronous tasks. A
//! relative path is resolved against the current working directory when the
//! operation starts.
//!
//! # Examples
//!
//! ```
//! # #[tokio::main]
//! # async fn main() -> Result<(), fairway_fs::Error> {
//! use fairway_codec::Json;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Deserialize, Serialize)]
//! struct Stats {
//!     runs: u32,
//! }
//!
//! let directory = fairway_fs::temp_dir().await?;
//! let path = directory.path().join("stats.json");
//!
//! fairway_fs::write(&path, Json(Stats { runs: 0 })).await?;
//! fairway_fs::edit(&path, |Json(mut stats): Json<Stats>| async move {
//!     stats.runs += 1;
//!     Ok::<_, fairway_fs::Error>(Json(stats))
//! })
//! .await?;
//!
//! let Json(stats): Json<Stats> = fairway_fs::read(&path).await?;
//! assert_eq!(stats.runs, 1);
//! # Ok(())
//! # }
//! ```
//!
//! # Replacing files
//!
//! [`write`] and [`edit`] never change a file in place. They write the new
//! contents to a temporary file in the same directory, copy the metadata of
//! the existing file to it, and rename it over the target. The rename is
//! atomic, so [`read`] and other programs see either the complete old
//! contents or the complete new contents, never a mix of both. If any step
//! fails, the temporary file is removed and the target is left as it was.
//!
//! The new file keeps the permissions, owner and group, and extended
//! attributes of the old one; on Linux, extended attributes include POSIX
//! ACLs. On macOS, the ACL, creation time, and file flags are kept as well,
//! and on Windows, the DACL, alternate data streams, and file attributes,
//! such as hidden. The modification time is updated. Because the target
//! becomes a different file, handles opened before the replacement keep
//! reading the old contents, and so do other hard links to the old file.
//!
//! A replacement therefore needs write access to both the file and its
//! directory. A file that the process may not write to is not replaced, even
//! if its directory would allow the rename, and the replacement fails if the
//! metadata cannot be copied, for example because the file belongs to
//! another user. The last component of the path must not be a symbolic link,
//! because renaming onto a link would replace the link itself rather than
//! the file it points to; symbolic links in the parent directories are
//! resolved.
//!
//! The contents are not flushed to disk before the rename, so the
//! replacement is atomic but not durable: after a power failure or an
//! operating system crash, the new contents may be lost. If the process is
//! killed in the middle of a replacement, its adjacent lock file may remain.
//! After checking that no operation is using it, remove it to allow further
//! writes and edits.
//!
//! # Locking
//!
//! [`write`] and [`edit`] reserve the target by exclusively creating an
//! adjacent file: `report.txt` uses `.report.txt.lock`. The reservation is
//! taken before reading or encoding, and the replacement contents are written
//! to this same file. Renaming it over the target publishes the result and
//! releases the reservation. Processes coordinate independently of
//! `FAIRWAY_HOME`. Other programs can still change the target at any time.
//!
//! Within a process, operations also share a queue keyed by the canonical
//! path's Unicode normalization and full case folding. Waiting [`edit`] calls
//! in one queue proceed in the order in which they started waiting. Different
//! queues can run concurrently; the adjacent file provides mutual exclusion
//! if more than one queue addresses the same target. Queues in different
//! processes have no shared ordering.
//!
//! [`edit`] waits without a time limit, polling an occupied adjacent name.
//! [`write`] tries once and returns [`WouldBlock`](io::ErrorKind::WouldBlock)
//! if the queue or the adjacent name is occupied. Different files with names
//! equivalent under normalization and case folding can share a queue, so a
//! write can also be busy while another file in that queue is being changed.
//! [`read`] uses neither the queue nor the reservation and sees one complete
//! version of the file thanks to the atomic replacement.
//!
//! The adjacent name is reserved for Fairway. An existing entry is left
//! untouched, and the extra prefix and suffix must fit the filesystem's name
//! length limit. A reservation left after a crash is not removed at startup.
//! On Windows, the staging file is hidden; the published file receives the
//! target's attributes, or normal attributes when the target is new.
//!
//! A successful return means the replacement is published and the lock is
//! released. On error, Fairway attempts to remove its reservation before
//! returning. After cancellation or a panic, cleanup runs in the background
//! and holds the local queue until the remaining work has finished.
//!
//! # Cancellation
//!
//! Dropping the future of an operation cancels only the work that has not
//! been handed to a blocking thread yet. Work on a blocking thread cannot be
//! interrupted and runs to the end in the background: a started [`read`]
//! finishes, and its result is discarded; a started replacement finishes
//! too, so the file ends up either unchanged or with the complete new
//! contents. [`write`](fn@write#cancellation) and [`edit`](edit#cancellation)
//! describe when their replacement starts.
//!
//! The lock is held until that background work has finished, so a [`write`]
//! of the same path issued right after a cancellation may fail with
//! [`WouldBlock`](io::ErrorKind::WouldBlock), and an [`edit`] waits.
//!
//! # Errors
//!
//! Every operation returns [`Error`]. It records the [`Operation`] that
//! failed and the path it concerned, and [`Error::kind`] classifies the
//! failure with an [`io::ErrorKind`]:
//!
//! - If the operating system reported the failure, the kind is the one it
//!   reported, and the original [`io::Error`] is the error's
//!   [`source`](std::error::Error::source).
//! - If a codec failed, the kind is [`InvalidData`](io::ErrorKind::InvalidData)
//!   for decoding and [`InvalidInput`](io::ErrorKind::InvalidInput) for
//!   encoding, and the codec's own error is the source.
//! - If the crate itself rejects the operation, for example because the
//!   target is not a regular file, the message says why, and there is no
//!   source.
//!
//! The message names the operation and the path but not the cause, which
//! stays in the source. To show both, print the whole chain of sources, as
//! the alternate format `{:#}` of `anyhow::Error` does.
//!
//! ```
//! # #[tokio::main]
//! # async fn main() -> Result<(), fairway_fs::Error> {
//! use std::{error::Error as _, io::ErrorKind};
//!
//! use fairway_codec::Json;
//! use fairway_fs::Operation;
//!
//! let directory = fairway_fs::temp_dir().await?;
//! let path = directory.path().join("numbers.json");
//! fairway_fs::write(&path, "[1, 2".to_owned()).await?;
//!
//! let error = fairway_fs::read::<Json<Vec<u32>>>(&path).await.unwrap_err();
//! assert_eq!(error.operation(), Operation::Decode);
//! assert_eq!(error.kind(), ErrorKind::InvalidData);
//! assert!(matches!(
//!     error.source().unwrap().downcast_ref::<fairway_codec::Error>(),
//!     Some(fairway_codec::Error::Decode { format: "json", .. })
//! ));
//! # Ok(())
//! # }
//! ```
//!
//! A panic in a codec is not turned into an error: it resumes in the task
//! that awaits the operation, with the original panic payload.
//!
//! [`write`]: fn@write
//! [`home`]: fn@home

mod directories;
mod error;
mod home;
mod lock;
mod platform;
mod temporary;
mod transaction;

use std::{
    future::Future,
    io::{self, Read},
    path::{Path, PathBuf},
};

use fairway_codec::{Decode, Encode};

pub use directories::{DirEntries, DirEntry, canonicalize, exists, ls, metadata, mkdir};
pub use error::{Error, Operation};
pub use home::home;
pub use temporary::{TempDir, TempFile, temp_dir, temp_file};

/// An error of a codec or of the runtime, kept as the
/// [`source`](std::error::Error::source) of an [`Error`].
type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Reads a whole file and decodes it as `T`.
///
/// The file is read into memory in full, and [`T::decode`](Decode::decode)
/// turns the bytes into a value; both steps run on a blocking thread.
/// Symbolic links are followed. `read` does not take the
/// [lock](crate#locking) of [`write`] and [`edit`]: because they replace files
/// atomically, a concurrent read returns either the old or the new contents.
///
/// # Errors
///
/// Returns an error if the file cannot be opened or read; its
/// [kind](Error::kind) is the one reported by the operating system, for
/// example [`NotFound`](io::ErrorKind::NotFound) or
/// [`PermissionDenied`](io::ErrorKind::PermissionDenied). In addition, the
/// kind is:
///
/// - [`NotFound`](io::ErrorKind::NotFound) if `path` is empty;
/// - [`InvalidInput`](io::ErrorKind::InvalidInput) if `path` is not a regular
///   file, such as a directory, a FIFO, or a device; on Windows, a directory
///   cannot even be opened this way, and the kind is
///   [`PermissionDenied`](io::ErrorKind::PermissionDenied);
/// - [`InvalidData`](io::ErrorKind::InvalidData) if decoding fails; the
///   decoder's error is the [`source`](std::error::Error::source) of the
///   returned error.
///
/// # Panics
///
/// Panics if polled outside a Tokio runtime. If `T::decode` panics, the
/// panic resumes in the task that awaits `read`.
///
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() -> Result<(), fairway_fs::Error> {
/// use fairway_codec::Json;
///
/// let directory = fairway_fs::temp_dir().await?;
/// let path = directory.path().join("tags.json");
/// fairway_fs::write(&path, r#"["draft", "notes"]"#.to_owned()).await?;
///
/// let Json(tags): Json<Vec<String>> = fairway_fs::read(&path).await?;
/// assert_eq!(tags, ["draft", "notes"]);
/// # Ok(())
/// # }
/// ```
///
/// [`write`]: fn@write
pub async fn read<T>(path: impl AsRef<Path>) -> Result<T, Error>
where
    T: Decode + Send + 'static,
    T::Error: Into<BoxError> + Send,
{
    let path = absolute(path.as_ref())?;
    blocking(Operation::Read, Some(path.clone()), move || {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Without O_NONBLOCK, opening a FIFO blocks until a writer
            // appears, before the file type can be checked. The flag has no
            // effect on reading a regular file.
            options.custom_flags(libc::O_NONBLOCK);
        }
        let mut file = options
            .open(&path)
            .map_err(|source| Error::io(Operation::Open, Some(&path), source))?;
        if !file
            .metadata()
            .map_err(|source| Error::io(Operation::Metadata, Some(&path), source))?
            .is_file()
        {
            return Err(Error::message(
                Operation::ValidateTarget,
                Some(&path),
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|source| Error::io(Operation::Read, Some(&path), source))?;
        T::decode(bytes).map_err(|source| decode_error(&path, source))
    })
    .await
}

/// Encodes `contents` and atomically replaces the file at `path` with the
/// result.
///
/// The file is created if it does not exist; its parent directory must
/// already exist. A new file gets the same permissions as one created with
/// [`File::create`](std::fs::File::create). An existing file is replaced as
/// described in [Replacing files](crate#replacing-files): the new contents
/// appear all at once, and the file keeps its metadata.
///
/// `write` holds the path's [lock](crate#locking) for the whole operation,
/// but it does not wait for it: if another Fairway operation is changing the
/// same path, `write` fails immediately with
/// [`WouldBlock`](io::ErrorKind::WouldBlock). Use [`edit`] to wait for the
/// lock or to base the new contents on the current ones.
///
/// # Cancellation
///
/// Once the lock is acquired, dropping the returned future does not stop the
/// operation: encoding and replacement continue on a blocking thread, and the
/// file ends up either unchanged, if one of them fails, or with the new
/// contents. The lock is released when this work is finished.
///
/// # Errors
///
/// Returns an error and leaves the file unchanged if:
///
/// - the path's queue or adjacent lock name is occupied
///   ([`WouldBlock`](io::ErrorKind::WouldBlock));
/// - `path` does not name a file, for example because it ends with a
///   separator, or it names a symbolic link, a directory, or another file
///   that is not a regular file ([`InvalidInput`](io::ErrorKind::InvalidInput));
/// - `path` is empty, or its parent directory does not exist
///   ([`NotFound`](io::ErrorKind::NotFound));
/// - the process may not write to the existing file or to its directory, or
///   may not give the new file the owner or permissions of the old one
///   ([`PermissionDenied`](io::ErrorKind::PermissionDenied));
/// - encoding fails ([`InvalidInput`](io::ErrorKind::InvalidInput)); the
///   encoder's error is the [`source`](std::error::Error::source) of the
///   returned error;
/// - the adjacent lock file cannot be created, for example because its
///   name exceeds the filesystem's limit.
///
/// Other failures reported by the operating system, such as a full disk,
/// keep the kind that it reported.
///
/// # Panics
///
/// Panics if polled outside a Tokio runtime. If `V::encode` panics, the
/// panic resumes in the task that awaits `write`, and the file is left
/// unchanged.
///
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() -> Result<(), fairway_fs::Error> {
/// use fairway_codec::{Toml, toml};
///
/// let directory = fairway_fs::temp_dir().await?;
/// let path = directory.path().join("settings.toml");
///
/// fairway_fs::write(&path, Toml(toml! { retries = 3 })).await?;
/// assert_eq!(fairway_fs::read::<String>(&path).await?, "retries = 3\n");
/// # Ok(())
/// # }
/// ```
///
/// [`home`]: fn@home
pub async fn write<V>(path: impl AsRef<Path>, contents: V) -> Result<(), Error>
where
    V: Encode + Send + 'static,
    V::Error: Into<BoxError> + Send,
{
    let (target, lock) = transaction::prepare(path.as_ref(), false).await?;
    blocking(Operation::Write, Some(target.clone()), move || {
        let (output, _) = transaction::open(target.clone(), lock, false)?;
        let bytes = match contents
            .encode()
            .map_err(|source| encode_error(&target, source))
        {
            Ok(bytes) => bytes,
            Err(error) => {
                output.abort();
                return Err(error);
            }
        };
        output.commit(bytes)
    })
    .await
}

/// Reads a file, passes its decoded contents to `handler`, and atomically
/// replaces the file with the value that the handler returns.
///
/// `edit` holds the path's [lock](crate#locking) for the whole operation, so
/// no other Fairway operation changes the file between reading and
/// replacing it. If the path is locked, `edit` waits without a time limit;
/// wrap it in [`tokio::time::timeout`] to bound the wait. The file must exist
/// and be writable, and both are checked before `handler` is called. The
/// replacement works as described in [Replacing files](crate#replacing-files).
///
/// Decoding and encoding run on a blocking thread, but `handler` is called in
/// the task that awaits `edit`. It may therefore borrow from the caller and
/// await other asynchronous operations, and its error type `E` does not have
/// to be [`Send`], although the future of `edit` is then not `Send` either.
/// The handler is called at most once, and not at all if the operation fails
/// before it, for example because the file cannot be read or decoded.
///
/// If the handler returns an error, the file is left unchanged and `edit`
/// returns that error as it is. If it returns a value, the file is rewritten
/// even if the value has not changed; return an error to leave the file
/// untouched. Calling `edit` for the same path from inside the handler never
/// completes, because it waits for the lock held by the outer call, and
/// [`write`] fails with [`WouldBlock`](io::ErrorKind::WouldBlock).
///
/// # Cancellation
///
/// Dropping the returned future before the handler's future has completed
/// leaves the file unchanged; this covers waiting for the lock, reading and
/// decoding, and the handler itself. Once the handler's future has completed
/// with a value, encoding and replacement continue on a blocking thread even
/// if the future of `edit` is dropped, and the file ends up either
/// unchanged, if one of them fails, or with the new contents.
///
/// # Errors
///
/// Returns the handler's error unchanged. Every other error is an [`Error`]
/// converted into `E` with [`From`], and in every case the file is left
/// unchanged. The [kind](Error::kind) of such an error is:
///
/// - [`NotFound`](io::ErrorKind::NotFound) if the file or its parent
///   directory does not exist, or if `path` is empty;
/// - [`PermissionDenied`](io::ErrorKind::PermissionDenied) if the process may
///   not write to the file or to its directory;
/// - [`InvalidInput`](io::ErrorKind::InvalidInput) if `path` does not name a
///   regular file, or if encoding the handler's value fails;
/// - [`InvalidData`](io::ErrorKind::InvalidData) if decoding the file fails.
///
/// The codec's error is the [`source`](std::error::Error::source) of the
/// [`Error`]. Other errors are the same as for [`write`].
///
/// # Panics
///
/// Panics if polled outside a Tokio runtime, or if it has to wait for a lock
/// held by another process while the runtime's time driver is disabled.
/// Panics in `handler`, `T::decode`, and `T::encode` resume in the task that
/// awaits `edit`, and the file is left unchanged.
///
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() -> Result<(), fairway_fs::Error> {
/// let directory = fairway_fs::temp_dir().await?;
/// let path = directory.path().join("todo.txt");
/// fairway_fs::write(&path, "- write docs\n".to_owned()).await?;
///
/// let item = "- run tests\n";
/// fairway_fs::edit(&path, |mut text: String| async move {
///     text.push_str(item);
///     Ok::<_, fairway_fs::Error>(text)
/// })
/// .await?;
///
/// let text: String = fairway_fs::read(&path).await?;
/// assert_eq!(text, "- write docs\n- run tests\n");
/// # Ok(())
/// # }
/// ```
///
/// [`write`]: fn@write
pub async fn edit<T, E, F, Fut>(path: impl AsRef<Path>, handler: F) -> Result<(), E>
where
    T: Decode + Encode + Send + 'static,
    <T as Decode>::Error: Into<BoxError> + Send,
    <T as Encode>::Error: Into<BoxError> + Send,
    E: From<Error>,
    F: FnOnce(T) -> Fut,
    Fut: Future<Output = Result<T, E>> + Send,
{
    let (target, lock) = transaction::prepare(path.as_ref(), true).await?;
    let read_path = target.clone();
    let (output, value) = blocking(Operation::Read, Some(target.clone()), move || {
        let (output, input) = transaction::open(read_path.clone(), lock, true)?;
        let result = (|| {
            let mut input = input.expect("editing opens the source");
            let mut bytes = Vec::new();
            input
                .read_to_end(&mut bytes)
                .map_err(|source| Error::io(Operation::Read, Some(&read_path), source))?;
            T::decode(bytes).map_err(|source| decode_error(&read_path, source))
        })();
        match result {
            Ok(value) => Ok((output, value)),
            Err(error) => {
                output.abort();
                Err(error)
            }
        }
    })
    .await?;
    let value = match handler(value).await {
        Ok(value) => value,
        Err(error) => {
            // Cleanup performs blocking I/O, so it runs on the blocking pool.
            // Waiting for it, instead of leaving it to `Drop`, releases the
            // lock before the handler's error reaches the caller.
            let _ = blocking(Operation::RemoveFile, Some(target), move || {
                output.abort();
                Ok(())
            })
            .await;
            return Err(error);
        }
    };
    blocking(Operation::Write, Some(target.clone()), move || {
        let bytes = match value
            .encode()
            .map_err(|source| encode_error(&target, source))
        {
            Ok(bytes) => bytes,
            Err(error) => {
                output.abort();
                return Err(error);
            }
        };
        output.commit(bytes)
    })
    .await?;
    Ok(())
}

/// Wraps a decoder's error: the contents of the file are invalid.
fn decode_error(path: &Path, error: impl Into<BoxError>) -> Error {
    Error::external(
        Operation::Decode,
        Some(path),
        io::ErrorKind::InvalidData,
        error,
    )
}

/// Wraps an encoder's error: the value passed in cannot be written.
fn encode_error(path: &Path, error: impl Into<BoxError>) -> Error {
    Error::external(
        Operation::Encode,
        Some(path),
        io::ErrorKind::InvalidInput,
        error,
    )
}

/// Returns `path` as an absolute path, without accessing the filesystem.
///
/// A relative path is resolved against the current working directory once,
/// when the operation starts, so a later change of the working directory
/// does not affect the operation. On Windows this is [`std::path::absolute`].
/// On other platforms the path is joined to the working directory as it is,
/// keeping `.` components and a trailing separator, which
/// [`lock::target`] checks for. Symbolic links and `..` are not resolved.
///
/// # Errors
///
/// Returns [`NotFound`](io::ErrorKind::NotFound) for an empty path, as the
/// operating system does, instead of resolving it to the working directory;
/// and the error of [`std::env::current_dir`] if the working directory is
/// unavailable.
fn absolute(path: &Path) -> Result<PathBuf, Error> {
    if path.as_os_str().is_empty() {
        return Err(Error::message(
            Operation::ResolvePath,
            Some(path),
            io::ErrorKind::NotFound,
            "empty filesystem path",
        ));
    }
    #[cfg(windows)]
    return std::path::absolute(path)
        .map_err(|source| Error::io(Operation::ResolvePath, Some(path), source));
    #[cfg(not(windows))]
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|source| Error::io(Operation::ResolvePath, Some(path), source))
    }
}

/// Runs `work` on Tokio's blocking thread pool and waits for its result.
///
/// The work cannot be interrupted: if the returned future is dropped, the
/// work still runs to the end, and its result, including any transaction or
/// lock it returns, is dropped on the pool thread. If the runtime shuts down
/// before the work starts, the result is an error of kind
/// [`Other`](io::ErrorKind::Other) that reports `operation` and `path`.
///
/// # Panics
///
/// Panics if called outside a Tokio runtime. A panic in `work` resumes in
/// the awaiting task with its original payload, so a panicking codec behaves
/// as if it had been called directly.
async fn blocking<T: Send + 'static>(
    operation: Operation,
    path: Option<PathBuf>,
    work: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(error) => Err(Error::external(
            operation,
            path.as_deref(),
            io::ErrorKind::Other,
            error,
        )),
    }
}

/// Runs blocking cleanup in the background; used by `Drop` implementations,
/// which cannot wait for I/O.
///
/// The work runs on Tokio's blocking pool when a runtime is available and on
/// a new thread otherwise. Values moved into `work`, such as a file lock,
/// stay alive until it has finished, so a lock outlives the cleanup that
/// comes before its release. If the work cannot be started, it is dropped on
/// the calling thread without running.
fn defer(work: impl FnOnce() + Send + 'static) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn_blocking(work);
    } else {
        let _ = std::thread::Builder::new()
            .name("fairway-fs-cleanup".into())
            .spawn(work);
    }
}

/// Hooks for the Fairway application itself; not part of the plugin API.
#[doc(hidden)]
pub mod __private {
    /// Fixes the Fairway directory for the rest of the process.
    ///
    /// The application calls this once at startup, before it loads the
    /// configuration, so that an unusable Fairway directory is reported
    /// before any command runs. It performs blocking I/O and does not need a
    /// Tokio runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if the Fairway directory cannot be determined (see
    /// [`home`](fn@crate::home)).
    pub fn initialize() -> Result<(), crate::Error> {
        crate::home::initialize()
    }
}

// Compile the Rust examples of the plugin guides as doctests, so that the
// guides cannot drift away from the API.
#[cfg(doctest)]
#[doc = include_str!("../../../docs/ru/plugin-development/fs.md")]
mod guide_ru {}

#[cfg(doctest)]
#[doc = include_str!("../../../docs/en/plugin-development/fs.md")]
mod guide_en {}
