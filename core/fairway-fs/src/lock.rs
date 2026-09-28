//! Locks that make replacements of a path take turns, within a process and
//! between processes.
//!
//! A lock has two levels. Within a process, each path has a gate, a Tokio
//! mutex: [`edit`](crate::edit) waits for it, and because Tokio's mutex is
//! fair, waiting edits proceed in the order in which they started waiting;
//! [`write`](crate::write) only tries it. Between processes, the holder of
//! the gate locks a file in the `locks` subdirectory of the Fairway directory
//! with [`File::try_lock`]. While another process holds that file, a waiting
//! operation tries again every [`RETRY`], so waiters from different processes
//! are not ordered.
//!
//! A lock file is named after the digest of the path's key and is deleted
//! when the lock is released, so that the directory does not keep a file for
//! every path ever locked. Deleting races with opening: a process could open
//! the file just before another one deletes it, then lock the deleted file,
//! while a third process creates and locks a new file under the same name,
//! and both would hold the lock. Therefore a lock file is opened and locked,
//! and deleted, only under the coordination lock, `.coordination.lock` in the
//! same directory. Lock files left behind by a process that crashed are
//! removed by [`clean_stale`].

use std::{
    collections::HashMap,
    fs::{File, TryLockError},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};

use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::{Error, Operation, error::Cause};

type Gates = Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>;

/// The gates of this process by lock key. They are held weakly, so a gate
/// exists only while an operation holds it or waits for it.
static GATES: OnceLock<Gates> = OnceLock::new();

/// The canonical path of the lock directory, set once the directory has been
/// created and cleaned. A failure is not stored, so the next lock tries again.
static DIRECTORY: Mutex<Option<PathBuf>> = Mutex::new(None);

/// How often a waiting operation tries again to lock a file that another
/// process holds.
const RETRY: Duration = Duration::from_millis(25);

/// A lock on a path, held at both levels.
///
/// [`release`](FileLock::release) releases it on the calling thread; dropping
/// it releases it in the background.
pub(crate) struct FileLock(Option<Lease>);

/// The resources of a held lock.
struct Lease {
    /// The locked file.
    file: File,
    path: PathBuf,
    directory: PathBuf,
    /// The guard of the gate. It is shared with the attempt that may still be
    /// running on a blocking thread, so that no other operation of this
    /// process passes the gate while a cancelled wait can still take the lock
    /// file.
    _local: Arc<OwnedMutexGuard<()>>,
}

/// Returns the path that [`write`](crate::write) and [`edit`](crate::edit)
/// lock and replace when given the absolute `path`.
///
/// The path must name a file, so an ending of `/` or `/.` is rejected,
/// although [`Path::file_name`] ignores it; on Windows, `\` and `\.` as well.
/// Symbolic links in the parent directories are resolved, so different paths
/// to one file give the same target and share its lock. The last component is
/// kept as it is: the file may not exist yet, and a symbolic link must not be
/// followed but rejected by the replacement. On Windows, an 8.3 short name of
/// the last component is expanded.
///
/// # Errors
///
/// Returns an error of [`Operation::ValidateTarget`] if `path` does not name
/// a file, and of [`Operation::Canonicalize`] if the parent directory cannot
/// be resolved, for example because it does not exist.
pub(crate) async fn target(path: PathBuf) -> Result<PathBuf, Error> {
    super::blocking(Operation::Canonicalize, Some(path.clone()), move || {
        let name = path.file_name().ok_or_else(|| {
            Error::message(
                Operation::ValidateTarget,
                Some(&path),
                io::ErrorKind::InvalidInput,
                "the target must name a file",
            )
        })?;
        let parent = path.parent().ok_or_else(|| {
            Error::message(
                Operation::ValidateTarget,
                Some(&path),
                io::ErrorKind::InvalidInput,
                "the target has no parent",
            )
        })?;
        if path.as_os_str().as_encoded_bytes().ends_with(b"/")
            || path.as_os_str().as_encoded_bytes().ends_with(b"/.")
            || cfg!(windows)
                && (path.as_os_str().as_encoded_bytes().ends_with(b"\\")
                    || path.as_os_str().as_encoded_bytes().ends_with(b"\\."))
        {
            return Err(Error::message(
                Operation::ValidateTarget,
                Some(&path),
                io::ErrorKind::InvalidInput,
                "the target must name a file",
            ));
        }
        let target = super::platform::canonical_parent(parent)
            .map_err(|cause| Error::new(Operation::Canonicalize, Some(&path), cause))?
            .join(name);
        #[cfg(windows)]
        let target = super::platform::canonical_target(target)
            .map_err(|cause| Error::new(Operation::Canonicalize, Some(&path), cause))?;
        Ok(target)
    })
    .await
}

/// Locks `target`, a path returned by [`target`].
///
/// If `wait` is set, the call waits until the lock is free; otherwise it fails
/// if the lock is held.
///
/// # Errors
///
/// Returns an error of kind [`WouldBlock`](io::ErrorKind::WouldBlock) if the
/// lock is held and `wait` is not set. Also returns an error if the Fairway
/// directory cannot be determined, or if the lock directory or a lock file
/// cannot be used.
pub(crate) async fn acquire(target: &Path, wait: bool) -> Result<FileLock, Error> {
    let path = target.to_owned();
    let key = super::blocking(Operation::Lock, Some(path.clone()), move || {
        super::platform::lock_key(&path)
            .map_err(|cause| Error::new(Operation::Lock, Some(&path), cause))
    })
    .await?;
    acquire_key(key, target, wait).await
}

/// Implements [`acquire`] for the lock `key` of `target`, which is used only
/// in errors.
async fn acquire_key(key: PathBuf, target: &Path, wait: bool) -> Result<FileLock, Error> {
    let gate = {
        let mut gates = GATES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // Remove the entries of gates that no longer exist, so that the map does
        // not grow with every path ever locked.
        gates.retain(|_, value| value.strong_count() != 0);
        match gates.get(&key).and_then(Weak::upgrade) {
            Some(gate) => gate,
            None => {
                let gate = Arc::new(AsyncMutex::new(()));
                gates.insert(key.clone(), Arc::downgrade(&gate));
                gate
            }
        }
    };
    let local = Arc::new(if wait {
        gate.lock_owned().await
    } else {
        gate.try_lock_owned().map_err(|_| busy(target))?
    });
    let directory_target = target.to_owned();
    let directory = super::blocking(Operation::Lock, Some(directory_target.clone()), move || {
        directory(&directory_target)
    })
    .await?;
    // A key can be long and contain any character, so the file is named after
    // its digest instead.
    let digest = Sha256::digest(key.as_os_str().as_encoded_bytes());
    let mut name = String::with_capacity(69);
    for byte in digest {
        use std::fmt::Write;
        write!(&mut name, "{byte:02x}").expect("writing to a String");
    }
    name.push_str(".lock");
    loop {
        let directory = directory.clone();
        let path = directory.join(&name);
        let local = local.clone();
        let attempt_target = target.to_owned();
        let attempt = super::blocking(Operation::Lock, Some(attempt_target.clone()), move || {
            let result = (|| {
                let _coordination = coordination(&directory)?;
                let file = super::platform::lock_file(&path)?;
                match file.try_lock() {
                    Ok(()) => Ok(Some(FileLock(Some(Lease {
                        file,
                        path,
                        directory,
                        _local: local,
                    })))),
                    // Close the file while the coordination lock is still held.
                    // Once it is released, the holder may delete the file, so the
                    // next attempt has to open the name again, which may then
                    // refer to a new file.
                    Err(TryLockError::WouldBlock) => {
                        drop(file);
                        Ok(None)
                    }
                    Err(TryLockError::Error(error)) => {
                        drop(file);
                        Err(Cause::Io(error))
                    }
                }
            })();
            result.map_err(|cause| Error::new(Operation::Lock, Some(&attempt_target), cause))
        })
        .await?;
        if let Some(lock) = attempt {
            return Ok(lock);
        }
        if !wait {
            return Err(busy(target));
        }
        tokio::time::sleep(RETRY).await;
    }
}

/// Returns the error for a lock that another operation holds.
fn busy(path: &Path) -> Error {
    Error::message(
        Operation::Lock,
        Some(path),
        io::ErrorKind::WouldBlock,
        "another Fairway operation is changing this path",
    )
}

/// Returns the lock directory, which is created and cleaned of stale lock
/// files on first use in the process; `target` is used only in errors.
fn directory(target: &Path) -> Result<PathBuf, Error> {
    let mut cached = DIRECTORY.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(path) = &*cached {
        return Ok(path.clone());
    }
    let path = super::home::resolved()?.join("locks");
    std::fs::create_dir_all(&path)
        .map_err(|source| Error::io(Operation::Lock, Some(target), source))?;
    let path = std::fs::canonicalize(path)
        .map_err(|source| Error::io(Operation::Lock, Some(target), source))?;
    clean_stale(&path).map_err(|cause| Error::new(Operation::Lock, Some(target), cause))?;
    *cached = Some(path.clone());
    Ok(path)
}

/// Takes the coordination lock of `directory`, which is held until the
/// returned file is closed.
///
/// If another process holds it, the call blocks until it is free. The lock is
/// only held for a few filesystem calls, so the wait is short.
fn coordination(directory: &Path) -> Result<File, Cause> {
    let file = super::platform::lock_file(&directory.join(".coordination.lock"))?;
    file.lock()?;
    Ok(file)
}

/// Deletes the lock files in `directory` that no process holds, such as those
/// of a process that crashed.
///
/// Only names of the form of a lock file are considered. A file is deleted
/// while this process holds both its lock and the coordination lock, so a
/// lock file in use is never deleted. Called when the Fairway application
/// starts and when a process first uses the directory.
pub(crate) fn clean_stale(directory: &Path) -> Result<(), Cause> {
    let _coordination = coordination(directory)?;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(key) = name.strip_suffix(".lock") else {
            continue;
        };
        if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let file = super::platform::lock_file(&entry.path())?;
        match file.try_lock() {
            Ok(()) => std::fs::remove_file(entry.path())?,
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        drop(file);
    }
    Ok(())
}

impl Lease {
    /// Deletes the lock file and releases the lock, and the gate after it.
    fn release(self) {
        // The file may be deleted only under the coordination lock. If that
        // lock cannot be taken, the file is left for `clean_stale`; dropping
        // `self` still closes it, which releases the lock.
        if let Ok(_coordination) = coordination(&self.directory) {
            let _ = std::fs::remove_file(&self.path);
            drop(self.file);
        }
    }
}

impl FileLock {
    /// Releases the lock on the calling thread.
    ///
    /// It performs blocking I/O, so it must run on a blocking thread. The
    /// replacement calls it there, so that the lock is free by the time
    /// [`write`](crate::write) or [`edit`](crate::edit) returns.
    pub(crate) fn release(mut self) {
        if let Some(lease) = self.0.take() {
            lease.release();
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if let Some(lease) = self.0.take() {
            super::defer(move || lease.release());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::{Future, poll_fn},
        pin::pin,
        task::Poll,
    };

    #[tokio::test]
    async fn waiting_edits_are_fifo_and_cancelled_waiters_leave_the_queue() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("queue");
        let first = acquire_key(path.clone(), &path, true).await?;
        let mut second = pin!(acquire_key(path.clone(), &path, true));
        let mut cancelled = Box::pin(acquire_key(path.clone(), &path, true));
        let mut last = pin!(acquire_key(path.clone(), &path, true));
        // Poll each waiter once, so that they join the gate's queue in this order.
        poll_fn(|context| {
            assert!(second.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        poll_fn(|context| {
            assert!(cancelled.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        poll_fn(|context| {
            assert!(last.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(cancelled);
        super::super::blocking(Operation::Lock, Some(path.clone()), move || {
            first.release();
            Ok(())
        })
        .await?;
        let second = second.await?;
        poll_fn(|context| {
            assert!(last.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        super::super::blocking(Operation::Lock, Some(path.clone()), move || {
            second.release();
            Ok(())
        })
        .await?;
        let last = last.await?;
        super::super::blocking(Operation::Lock, Some(path.clone()), move || {
            last.release();
            Ok(())
        })
        .await?;
        Ok(())
    }
}
