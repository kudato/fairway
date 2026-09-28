//! Atomic replacement of a file through a temporary file in its directory.
//!
//! [`prepare`] resolves and locks the target, [`open`] opens the file being
//! replaced and creates the temporary file, and [`Transaction::commit`] writes
//! the encoded contents to the temporary file, copies the metadata, and
//! renames it over the target. Whatever the outcome, the temporary file is
//! then removed, and the lock is released last.

use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use crate::{Error, Operation, lock::FileLock};

/// A replacement in progress: the lock on the target, the file being replaced,
/// and the temporary file that replaces it.
///
/// It is consumed by [`commit`](Transaction::commit) or
/// [`abort`](Transaction::abort) on a blocking thread. If it is dropped
/// instead, for example because the operation was cancelled or a codec
/// panicked, the cleanup runs in the background.
pub(crate) struct Transaction {
    /// The temporary file, open for writing.
    output: Option<File>,
    /// The file being replaced, or `None` if the target does not exist yet.
    original: Option<File>,
    /// The path of the temporary file; dropping it deletes the file.
    temp: Option<tempfile::TempPath>,
    target: PathBuf,
    lock: Option<FileLock>,
    /// Lets a test pause `commit` on the blocking thread: `commit` reports
    /// through the sender that it has started and waits for the receiver.
    #[cfg(test)]
    commit_gate: Option<(
        tokio::sync::oneshot::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    )>,
}

/// Resolves the target of `path` and locks it; only `editing` waits for the
/// lock.
pub(crate) async fn prepare(path: &Path, editing: bool) -> Result<(PathBuf, FileLock), Error> {
    let target = super::lock::target(super::absolute(path)?).await?;
    let lock = super::lock::acquire(&target, editing).await?;
    Ok((target, lock))
}

/// Starts the replacement of `target`, whose `lock` is held: opens the file
/// being replaced and creates the temporary file. Runs on a blocking thread.
///
/// When `editing`, the target must exist, and a second handle to it is
/// returned for reading the current contents. Both the target and the
/// temporary file are opened before anything is encoded or a handler is
/// called, so that a missing permission is reported first. On failure, the
/// lock is released before returning.
pub(crate) fn open(
    target: PathBuf,
    lock: FileLock,
    editing: bool,
) -> Result<(Transaction, Option<File>), Error> {
    let setup = (|| {
        let original = super::platform::open_original(&target, editing)?;
        let source = if editing {
            original
                .as_ref()
                .map(File::try_clone)
                .transpose()
                .map_err(|source| Error::io(Operation::Open, Some(&target), source))?
        } else {
            None
        };
        let (file, temp) = super::temporary::adjacent(&target, original.is_none())
            .map_err(|source| Error::io(Operation::CreateTemporaryFile, Some(&target), source))?;
        Ok::<_, Error>((original, source, file, temp))
    })();
    let (original, source, file, temp) = match setup {
        Ok(parts) => parts,
        Err(error) => {
            lock.release();
            return Err(error);
        }
    };
    Ok((
        Transaction {
            output: Some(file),
            original,
            temp: Some(temp),
            target,
            lock: Some(lock),
            #[cfg(test)]
            commit_gate: None,
        },
        source,
    ))
}

impl Transaction {
    /// Replaces the target with `bytes`, then removes the temporary file and
    /// releases the lock.
    ///
    /// It runs on a blocking thread and cannot be interrupted: once started,
    /// the replacement finishes even if the operation is cancelled. If any
    /// step fails, the target is left as it was.
    pub(crate) fn commit(mut self, bytes: Vec<u8>) -> Result<(), Error> {
        #[cfg(test)]
        if let Some((started, release)) = self.commit_gate.take() {
            let _ = started.send(());
            release.recv().expect("commit gate released");
        }
        let result = self.publish(&bytes);
        self.discard();
        result
    }

    /// Writes `bytes` to the temporary file, copies the metadata of the file
    /// being replaced to it, and renames it over the target.
    fn publish(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.output
            .as_mut()
            .expect("open transaction")
            .write_all(bytes)
            .map_err(|source| Error::io(Operation::Write, Some(&self.target), source))?;
        if let Some(original) = &self.original {
            super::platform::copy_metadata(
                original,
                self.output.as_ref().expect("open transaction"),
            )
            .map_err(|cause| Error::new(Operation::CopyMetadata, Some(&self.target), cause))?;
        }
        // Check the target again right before the rename: since it was opened,
        // another program may have replaced it with a symbolic link, and the
        // rename would replace the link instead of the file.
        super::platform::check_target(&self.target, false)?;
        drop(self.output.take());
        std::fs::rename(self.temp.as_ref().expect("temporary path"), &self.target)
            .map_err(|source| Error::io(Operation::Replace, Some(&self.target), source))
    }

    /// Closes the files, deletes the temporary file if it is still there, and
    /// then releases the lock.
    fn discard(&mut self) {
        drop(self.output.take());
        drop(self.original.take());
        drop(self.temp.take());
        if let Some(lock) = self.lock.take() {
            lock.release();
        }
    }

    /// Abandons the replacement and leaves the target unchanged. Runs on a
    /// blocking thread, like [`commit`](Transaction::commit).
    pub(crate) fn abort(mut self) {
        self.discard();
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        let output = self.output.take();
        let original = self.original.take();
        let temp = self.temp.take();
        let lock = self.lock.take();
        if output.is_none() && original.is_none() && temp.is_none() && lock.is_none() {
            return;
        }
        // Closing the files and deleting the temporary file block, so they run
        // in the background. The lock is released last, so that the next
        // operation on the path starts only after the cleanup is complete.
        super::defer(move || {
            drop(output);
            drop(original);
            drop(temp);
            if let Some(lock) = lock {
                lock.release();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocking;
    use std::{io, sync::mpsc};
    use tokio::sync::oneshot;

    async fn open_transaction(path: &Path, editing: bool) -> Result<Transaction, Error> {
        let (target, lock) = prepare(path, editing).await?;
        blocking(Operation::Open, Some(target.clone()), move || {
            open(target, lock, editing).map(|(output, _)| output)
        })
        .await
    }

    #[tokio::test]
    async fn failed_write_preserves_original_and_cleans_up() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("data");
        std::fs::write(&path, "old")?;
        let mut output = open_transaction(&path, false).await?;
        // A handle opened only for reading makes writing the new contents fail.
        let readonly = File::open(output.temp.as_ref().unwrap())?;
        output.output = Some(readonly);
        let error = blocking(Operation::Write, Some(path.clone()), move || {
            output.commit(b"cannot write this".to_vec())
        })
        .await
        .unwrap_err();
        assert_eq!(error.operation(), Operation::Write);
        assert_eq!(error.path(), Some(std::fs::canonicalize(&path)?.as_path()));
        assert!(std::error::Error::source(&error).unwrap().is::<io::Error>());
        assert_eq!(std::fs::read_to_string(&path)?, "old");
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        let following = open_transaction(&path, false).await?;
        blocking(Operation::Write, Some(path.clone()), move || {
            following.commit(b"following".to_vec())
        })
        .await?;
        assert_eq!(std::fs::read_to_string(path)?, "following");
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_commit_keeps_the_lock_until_background_replacement_completes()
    -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("data");
        std::fs::write(&path, "old")?;
        let mut output = open_transaction(&path, false).await?;
        // Pause the commit once it has started on the blocking thread, and
        // cancel the task that awaits it.
        let (started, receiver) = oneshot::channel();
        let (release, wait) = mpsc::channel();
        output.commit_gate = Some((started, wait));
        let commit = tokio::spawn(blocking(Operation::Write, Some(path.clone()), move || {
            output.commit(b"new".to_vec())
        }));
        receiver.await.unwrap();
        commit.abort();
        let _ = commit.await;
        assert!(
            matches!(open_transaction(&path, false).await, Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
        assert_eq!(std::fs::read_to_string(&path)?, "old");
        release.send(()).unwrap();
        let following = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            open_transaction(&path, true),
        )
        .await??;
        assert_eq!(std::fs::read_to_string(&path)?, "new");
        blocking(Operation::Write, Some(path.clone()), move || {
            following.abort();
            Ok(())
        })
        .await?;
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn replacement_failure_reports_the_target_and_original_os_error() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = std::fs::canonicalize(directory.path())?.join("new");
        let output = open_transaction(&path, false).await?;
        // Without the temporary file, the rename fails.
        std::fs::remove_file(output.temp.as_ref().unwrap())?;
        let error = blocking(Operation::Write, Some(path.clone()), move || {
            output.commit(b"new".to_vec())
        })
        .await
        .unwrap_err();
        assert_eq!(error.operation(), Operation::Replace);
        assert_eq!(error.path(), Some(path.as_path()));
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(std::error::Error::source(&error).unwrap().is::<io::Error>());
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn protected_metadata_reports_its_own_error_without_an_os_cause() -> anyhow::Result<()> {
        use std::process::Command;

        for flag in ["uchg", "uappnd"] {
            let directory = tempfile::tempdir()?;
            let path = std::fs::canonicalize(directory.path())?.join("data");
            std::fs::write(&path, b"old")?;
            let output = open_transaction(&path, false).await?;
            let protected = Command::new("chflags").arg(flag).arg(&path).output()?;
            assert!(protected.status.success(), "{protected:?}");
            let result = blocking(Operation::Write, Some(path.clone()), move || {
                output.commit(b"new".to_vec())
            })
            .await;
            let cleanup = Command::new("chflags")
                .args(["-R", "0"])
                .arg(directory.path())
                .output()?;
            assert!(cleanup.status.success(), "{cleanup:?}");

            let error = result.unwrap_err();
            assert_eq!(error.operation(), Operation::CopyMetadata);
            assert_eq!(error.path(), Some(path.as_path()));
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(error.raw_os_error(), None);
            assert!(std::error::Error::source(&error).is_none());
            assert!(error.to_string().contains("immutable or append-only"));
            assert_eq!(std::fs::read(&path)?, b"old");
            assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        }
        Ok(())
    }
}
