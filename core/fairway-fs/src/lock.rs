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

type Gates = Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>;
static GATES: OnceLock<Gates> = OnceLock::new();
static DIRECTORY: Mutex<Option<PathBuf>> = Mutex::new(None);
const RETRY: Duration = Duration::from_millis(25);

pub(crate) struct FileLock(Option<Lease>);

struct Lease {
    file: File,
    path: PathBuf,
    directory: PathBuf,
    _local: Arc<OwnedMutexGuard<()>>,
}

pub(crate) async fn target(path: PathBuf) -> io::Result<PathBuf> {
    super::blocking(move || {
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "the target must name a file")
        })?;
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "the target has no parent")
        })?;
        if path.as_os_str().as_encoded_bytes().ends_with(b"/")
            || path.as_os_str().as_encoded_bytes().ends_with(b"/.")
            || cfg!(windows)
                && (path.as_os_str().as_encoded_bytes().ends_with(b"\\")
                    || path.as_os_str().as_encoded_bytes().ends_with(b"\\."))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the target must name a file",
            ));
        }
        Ok(super::platform::canonical_parent(parent)?.join(name))
    })
    .await
}

pub(crate) async fn acquire(target: &Path, wait: bool) -> io::Result<FileLock> {
    let path = target.to_owned();
    let key = super::blocking(move || super::platform::lock_key(&path)).await?;
    acquire_key(key, wait).await
}

async fn acquire_key(key: PathBuf, wait: bool) -> io::Result<FileLock> {
    let gate = {
        let mut gates = GATES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // Remove unused path entries instead of retaining every path ever written.
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
        gate.try_lock_owned().map_err(|_| busy())?
    });
    let directory = super::blocking(directory).await?;
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
        let attempt = super::blocking(move || {
            let _coordination = coordination(&directory)?;
            let file = super::platform::lock_file(&path)?;
            match file.try_lock() {
                Ok(()) => Ok(Some(FileLock(Some(Lease {
                    file,
                    path,
                    directory,
                    _local: local,
                })))),
                // Close before releasing .coordination.lock. A future attempt
                // must reopen the name, which may now refer to another inode.
                Err(TryLockError::WouldBlock) => {
                    drop(file);
                    Ok(None)
                }
                Err(TryLockError::Error(error)) => {
                    drop(file);
                    Err(error)
                }
            }
        })
        .await?;
        if let Some(lock) = attempt {
            return Ok(lock);
        }
        if !wait {
            return Err(busy());
        }
        tokio::time::sleep(RETRY).await;
    }
}

fn busy() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "another Fairway operation is changing this path",
    )
}

fn directory() -> io::Result<PathBuf> {
    let mut cached = DIRECTORY.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(path) = &*cached {
        return Ok(path.clone());
    }
    let path = super::home::resolved()?.join("locks");
    std::fs::create_dir_all(&path)?;
    let path = std::fs::canonicalize(path)?;
    clean_stale(&path)?;
    *cached = Some(path.clone());
    Ok(path)
}

fn coordination(directory: &Path) -> io::Result<File> {
    let file = super::platform::lock_file(&directory.join(".coordination.lock"))?;
    file.lock()?;
    Ok(file)
}

pub(crate) fn clean_stale(directory: &Path) -> io::Result<()> {
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
            Err(TryLockError::Error(error)) => return Err(error),
        }
        drop(file);
    }
    Ok(())
}

impl Lease {
    fn release(self) {
        // If housekeeping fails, leave the name for the next startup. Closing
        // the handle still releases the OS lock; never unlink without coordination.
        if let Ok(_coordination) = coordination(&self.directory) {
            let _ = std::fs::remove_file(&self.path);
            drop(self.file);
        }
    }
}

impl FileLock {
    // Called from a blocking worker before returning a completed write.
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
    async fn waiting_edits_are_fifo_and_cancelled_waiters_leave_the_queue() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("queue");
        let first = acquire_key(path.clone(), true).await?;
        let mut second = pin!(acquire_key(path.clone(), true));
        let mut cancelled = Box::pin(acquire_key(path.clone(), true));
        let mut last = pin!(acquire_key(path.clone(), true));
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
        super::super::blocking(move || {
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
        super::super::blocking(move || {
            second.release();
            Ok(())
        })
        .await?;
        let last = last.await?;
        super::super::blocking(move || {
            last.release();
            Ok(())
        })
        .await?;
        Ok(())
    }
}
