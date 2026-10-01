//! Per-key FIFO queues and adjacent files that reserve a replacement.
//!
//! A process queues operations by a normalized path key. The operation at
//! the front creates `.<name>.lock` exclusively, before reading the target.
//! That file also holds the replacement contents. Renaming it over the target
//! publishes the result and releases the reservation between processes.

use std::{
    collections::HashMap,
    ffi::OsString,
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};

use caseless::Caseless;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use unicode_normalization::UnicodeNormalization;

use crate::{Error, Operation};

type Key = [u8; 32];
type Gates = Mutex<HashMap<Key, Weak<AsyncMutex<()>>>>;

/// Queues live as long as an operation holds them or waits for them.
static GATES: OnceLock<Gates> = OnceLock::new();

/// Interval between attempts to reserve a path held by another process.
const RETRY: Duration = Duration::from_millis(25);

/// Owns the adjacent file and the local queue until publication or cleanup.
pub(crate) struct FileLock(Option<Lease>);

struct Lease {
    file: Option<File>,
    path: Option<PathBuf>,
    /// A blocking attempt can outlive the future awaiting it. Keep its place
    /// in the local queue until that attempt and its cleanup have finished.
    _local: Arc<OwnedMutexGuard<()>>,
}

/// Resolves the absolute target, rejecting a final symbolic link or a path
/// that does not name a regular file. A new file uses its canonical parent.
pub(crate) async fn target(path: PathBuf) -> Result<PathBuf, Error> {
    super::blocking(Operation::Canonicalize, Some(path.clone()), move || {
        let invalid = || {
            Error::message(
                Operation::ValidateTarget,
                Some(&path),
                io::ErrorKind::InvalidInput,
                "the target must name a file",
            )
        };
        let name = path.file_name().ok_or_else(invalid)?;
        let parent = path.parent().ok_or_else(invalid)?;
        if path.as_os_str().as_encoded_bytes().ends_with(b"/")
            || path.as_os_str().as_encoded_bytes().ends_with(b"/.")
            || cfg!(windows)
                && (path.as_os_str().as_encoded_bytes().ends_with(b"\\")
                    || path.as_os_str().as_encoded_bytes().ends_with(b"\\."))
        {
            return Err(invalid());
        }
        let target = std::fs::canonicalize(parent)
            .map_err(|source| Error::io(Operation::Canonicalize, Some(&path), source))?
            .join(name);
        if super::platform::check_target(&target, false)? {
            std::fs::canonicalize(&target)
                .map_err(|source| Error::io(Operation::Canonicalize, Some(&target), source))
        } else {
            Ok(target)
        }
    })
    .await
}

/// Hashes canonical caseless Unicode text, preserving non-Unicode native
/// path bytes between text fragments. The resulting key is only used in memory.
fn key(path: &Path) -> Key {
    let mut digest = Sha256::new();
    for chunk in path.as_os_str().as_encoded_bytes().utf8_chunks() {
        let text: String = chunk.valid().nfd().default_case_fold().nfd().collect();
        digest.update(text.as_bytes());
        digest.update(chunk.invalid());
    }
    digest.finalize().into()
}

fn adjacent(target: &Path) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(target.file_name().expect("normalized target"));
    name.push(".lock");
    target.with_file_name(name)
}

/// Waits for the local queue and the adjacent file, or tries both once when
/// `wait` is false. An existing adjacent file is never opened or removed here.
pub(crate) async fn acquire(target: &Path, wait: bool) -> Result<FileLock, Error> {
    acquire_key(key(target), target, wait).await
}

async fn acquire_key(key: Key, target: &Path, wait: bool) -> Result<FileLock, Error> {
    let gate = {
        let mut gates = GATES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        gates.retain(|_, value| value.strong_count() != 0);
        match gates.get(&key).and_then(Weak::upgrade) {
            Some(gate) => gate,
            None => {
                let gate = Arc::new(AsyncMutex::new(()));
                gates.insert(key, Arc::downgrade(&gate));
                gate
            }
        }
    };
    let local = Arc::new(if wait {
        gate.lock_owned().await
    } else {
        gate.try_lock_owned().map_err(|_| busy(target))?
    });
    loop {
        let path = adjacent(target);
        let local = local.clone();
        let attempt_target = target.to_owned();
        let attempt = super::blocking(Operation::Lock, Some(attempt_target.clone()), move || {
            match super::platform::create_lock(&path) {
                Ok(file) => Ok(Some(FileLock(Some(Lease {
                    file: Some(file),
                    path: Some(path),
                    _local: local,
                })))),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(None),
                Err(source) => Err(Error::io(Operation::Lock, Some(&attempt_target), source)),
            }
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

fn busy(path: &Path) -> Error {
    Error::message(
        Operation::Lock,
        Some(path),
        io::ErrorKind::WouldBlock,
        "the path's queue or adjacent lock file is occupied",
    )
}

impl Lease {
    fn release(mut self) {
        drop(self.file.take());
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl FileLock {
    /// Transfers the open file to the transaction; this lock retains ownership
    /// of its name and must outlive that file handle.
    pub(crate) fn take_file(&mut self) -> File {
        self.0
            .as_mut()
            .expect("held lock")
            .file
            .take()
            .expect("lock file")
    }

    pub(crate) fn path(&self) -> &Path {
        self.0
            .as_ref()
            .expect("held lock")
            .path
            .as_deref()
            .expect("unpublished lock")
    }

    /// The rename consumed our name. A later operation may already own a new
    /// file there, so cleanup must no longer remove it.
    pub(crate) fn published(&mut self) {
        self.0.as_mut().expect("held lock").path = None;
    }

    /// Cleans up on the calling blocking thread, then releases the queue.
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

    #[test]
    fn queue_keys_use_canonical_caseless_matching_for_the_whole_path() {
        for (first, second) in [
            ("Projects/Report", "projects/report"),
            ("dir/straße", "DIR/STRAẞE"),
            ("dir/straße", "dir/STRASSE"),
            ("dir/xΣ", "dir/xς"),
            ("dir/xσ", "dir/xς"),
            ("CAFÉ/data", "cafe\u{301}/data"),
        ] {
            assert_eq!(
                key(Path::new(first)),
                key(Path::new(second)),
                "{first} / {second}"
            );
        }
        assert_ne!(key(Path::new("one/data")), key(Path::new("two/data")));
        assert_ne!(key(Path::new("dir/cafe")), key(Path::new("dir/café")));
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_names_keep_their_bytes_and_fold_the_text_around_them() {
        use std::os::unix::ffi::OsStrExt;
        let first = Path::new(std::ffi::OsStr::from_bytes(b"dir/\xffReport"));
        let alias = Path::new(std::ffi::OsStr::from_bytes(b"DIR/\xffREPORT"));
        let other = Path::new(std::ffi::OsStr::from_bytes(b"dir/\xfeReport"));
        assert_eq!(key(first), key(alias));
        assert_ne!(key(first), key(other));
    }

    #[tokio::test]
    async fn waiting_for_an_adjacent_file_does_not_block_other_targets() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let first = directory.path().join("first");
        let other = directory.path().join("other");
        let reservation = adjacent(&first);
        std::fs::write(&reservation, "held by another process")?;
        let mut waiting = Box::pin(acquire(&first, true));
        assert!(
            tokio::time::timeout(Duration::from_millis(80), &mut waiting)
                .await
                .is_err()
        );
        let independent =
            tokio::time::timeout(Duration::from_secs(5), acquire(&other, false)).await??;
        super::super::blocking(Operation::Lock, Some(other), move || {
            independent.release();
            std::fs::remove_file(reservation).unwrap();
            Ok(())
        })
        .await?;
        let lock = tokio::time::timeout(Duration::from_secs(5), waiting).await??;
        super::super::blocking(Operation::Lock, Some(first), move || {
            lock.release();
            Ok(())
        })
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn waiting_edits_are_fifo_and_cancelled_waiters_leave_the_queue() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("queue");
        let first = acquire_key(key(&path), &path, true).await?;
        let mut second = pin!(acquire_key(key(&path), &path, true));
        let mut cancelled = Box::pin(acquire_key(key(&path), &path, true));
        let mut last = pin!(acquire_key(key(&path), &path, true));
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
