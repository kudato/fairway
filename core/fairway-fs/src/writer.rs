use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

use fairway_codec::Encode;
use tokio::task::JoinHandle;

use crate::{BoxError, Reader, lock::FileLock};

const CAPACITY: usize = 64 * 1024;

// State only lives on the caller between operations. Every operation that can
// block moves it (including the lock) into one worker until that operation ends.
struct State {
    output: Option<BufWriter<File>>,
    original: Option<File>,
    temp: Option<tempfile::TempPath>,
    target: PathBuf,
    lock: Option<FileLock>,
    #[cfg(test)]
    commit_gate: Option<(
        tokio::sync::oneshot::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    )>,
}

impl State {
    fn output(&mut self) -> &mut BufWriter<File> {
        self.output.as_mut().expect("open writer")
    }

    fn commit(mut self) -> io::Result<()> {
        #[cfg(test)]
        if let Some((started, release)) = self.commit_gate.take() {
            let _ = started.send(());
            release.recv().expect("commit gate released");
        }
        let result = self.publish();
        self.discard();
        result
    }

    fn publish(&mut self) -> io::Result<()> {
        self.output().flush()?;
        if let Some(original) = &self.original {
            super::platform::copy_metadata(
                original,
                self.output.as_ref().expect("open writer").get_ref(),
            )?;
        }
        // Revalidate the leaf. We never replace a symlink, even if another
        // program created one while the temporary file was being written.
        super::platform::check_target(&self.target, false)?;
        let (file, _) = self.output.take().expect("open writer").into_parts();
        drop(file);
        std::fs::rename(self.temp.as_ref().expect("temporary path"), &self.target)?;
        drop(self.temp.take());
        if let Some(lock) = self.lock.take() {
            lock.release();
        }
        Ok(())
    }

    fn discard(&mut self) {
        if let Some(output) = self.output.take() {
            drop(output.into_parts());
        }
        drop(self.original.take());
        drop(self.temp.take());
        if let Some(lock) = self.lock.take() {
            lock.release();
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let output = self.output.take();
        let original = self.original.take();
        let temp = self.temp.take();
        let lock = self.lock.take();
        if output.is_none() && original.is_none() && temp.is_none() && lock.is_none() {
            return;
        }
        // BufWriter normally flushes on drop. Discard its buffer explicitly;
        // unsuccessful transactions must only remove their temporary output.
        super::defer(move || {
            if let Some(output) = output {
                drop(output.into_parts());
            }
            drop(original);
            drop(temp);
            if let Some(lock) = lock {
                lock.release();
            }
        });
    }
}

/// Buffered, atomic file replacement. Call [`Self::finish`] to publish it.
pub struct Writer {
    state: Option<State>,
    pending: Option<JoinHandle<(State, io::Result<()>)>>,
    poisoned: bool,
}

impl Drop for Writer {
    fn drop(&mut self) {
        // Cancel queued work. A running I/O job still owns State
        // and its lock until it ends, then removes the temporary output.
        if let Some(pending) = &self.pending {
            pending.abort();
        }
    }
}

/// Opens an atomic writer. Returns `WouldBlock` if another Fairway operation owns the path.
pub async fn writer(path: impl AsRef<Path>) -> io::Result<Writer> {
    let (writer, _) = open(path.as_ref(), false).await?;
    Ok(writer)
}

async fn open(path: &Path, editing: bool) -> io::Result<(Writer, Option<Reader>)> {
    let target = super::lock::target(super::absolute(path)?).await?;
    let lock = super::lock::acquire(&target, editing).await?;
    let (state, source) = super::blocking(move || {
        let setup = (|| {
            let original = super::platform::open_original(&target, editing)?;
            let source = if editing {
                original.as_ref().map(File::try_clone).transpose()?
            } else {
                None
            };
            let (file, temp) = super::temporary::adjacent(&target, original.is_none())?;
            Ok::<_, io::Error>((original, source, file, temp))
        })();
        let (original, source, file, temp) = match setup {
            Ok(parts) => parts,
            Err(error) => {
                lock.release();
                return Err(error);
            }
        };
        Ok((
            State {
                output: Some(BufWriter::with_capacity(CAPACITY, file)),
                original,
                temp: Some(temp),
                target,
                lock: Some(lock),
                #[cfg(test)]
                commit_gate: None,
            },
            source,
        ))
    })
    .await?;
    Ok((
        Writer {
            state: Some(state),
            pending: None,
            poisoned: false,
        },
        source.map(|file| Reader::new(tokio::fs::File::from_std(file))),
    ))
}

impl Writer {
    fn check(&self) -> io::Result<()> {
        if self.poisoned {
            Err(io::Error::other(
                "an earlier operation failed or was cancelled; this result cannot be saved",
            ))
        } else {
            Ok(())
        }
    }

    async fn complete(&mut self) -> io::Result<()> {
        if let Some(pending) = &mut self.pending {
            // Await by reference: cancelling flush keeps this job in the writer.
            let result = pending.await;
            self.pending = None;
            match result {
                Ok((state, result)) => {
                    self.state = Some(state);
                    if result.is_err() {
                        self.poisoned = true;
                    }
                    result
                }
                Err(error) => {
                    self.poisoned = true;
                    Err(io::Error::other(error))
                }
            }
        } else {
            Ok(())
        }
    }

    fn start(
        &mut self,
        operation: impl FnOnce(&mut BufWriter<File>) -> io::Result<()> + Send + 'static,
    ) {
        let mut state = self.state.take().expect("idle writer");
        self.pending = Some(tokio::task::spawn_blocking(move || {
            let result = operation(state.output());
            (state, result)
        }));
    }

    /// Accepts the entire encoded chunk. Failure or cancellation prevents publication.
    pub async fn write<V>(&mut self, contents: V) -> io::Result<()>
    where
        V: Encode + Send + 'static,
        V::Error: Into<BoxError> + Send,
    {
        self.check()?;
        // Set before the first await. A dropped, started future leaves this set.
        self.poisoned = true;
        self.complete().await?;
        let bytes = fairway_codec::encode(contents)
            .await
            .map_err(super::encode_error)?;
        self.start(move |output| output.write_all(&bytes));
        self.complete().await?;
        self.poisoned = false;
        Ok(())
    }

    /// Waits for accepted data to reach the OS. Cancellation is safe to resume.
    pub async fn flush(&mut self) -> io::Result<()> {
        self.check()?;
        self.complete().await?;
        self.start(Write::flush);
        self.complete().await
    }

    /// Flushes accepted data, preserves metadata, and atomically replaces the target.
    /// Once the commit worker starts, cancellation does not stop it.
    pub async fn finish(mut self) -> io::Result<()> {
        if let Err(error) = self.check() {
            self.abort().await;
            return Err(error);
        }
        if let Err(error) = self.complete().await {
            self.abort().await;
            return Err(error);
        }
        let state = self.state.take().expect("idle writer");
        super::blocking(move || state.commit()).await
    }

    pub(crate) async fn abort(mut self) {
        let _ = self.complete().await;
        if let Some(mut state) = self.state.take() {
            let _ = super::blocking(move || {
                state.discard();
                Ok(())
            })
            .await;
        }
    }
}

/// A locked original file and a separate writer for its replacement. An error ends editing.
pub struct Editor {
    input: Reader,
    output: Writer,
}

/// Waits for exclusive editing access and opens an existing regular file.
pub async fn editor(path: impl AsRef<Path>) -> io::Result<Editor> {
    let (output, input) = open(path.as_ref(), true).await?;
    Ok(Editor {
        input: input.expect("editing opens the source"),
        output,
    })
}

impl Editor {
    pub(crate) async fn abort(self) {
        self.output.abort().await;
    }
    /// Reads bytes from the original file. Read and write positions are independent.
    pub async fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.output.check()?;
        let result = self.input.read(buffer).await;
        if result.is_err() {
            self.output.poisoned = true;
        }
        result
    }

    /// Reads an original UTF-8 line without LF or CRLF, retaining partial input on cancellation.
    pub async fn next_line(&mut self) -> io::Result<Option<String>> {
        self.output.check()?;
        let result = self.input.next_line().await;
        if result.is_err() {
            self.output.poisoned = true;
        }
        result
    }

    /// Appends a whole encoded chunk to the replacement.
    pub async fn write<V>(&mut self, contents: V) -> io::Result<()>
    where
        V: Encode + Send + 'static,
        V::Error: Into<BoxError> + Send,
    {
        self.output.write(contents).await
    }

    /// Waits for accepted output to reach the OS. Cancellation is safe to resume.
    pub async fn flush(&mut self) -> io::Result<()> {
        self.output.flush().await
    }

    /// Replaces the original with only the explicitly written data, including an empty result.
    pub async fn finish(self) -> io::Result<()> {
        self.output.finish().await
    }

    pub(crate) async fn read_all(&mut self) -> io::Result<Vec<u8>> {
        let result = self.input.read_all().await;
        if result.is_err() {
            self.output.poisoned = true;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::{Future, poll_fn},
        pin::pin,
        sync::mpsc,
        task::Poll,
    };
    use tokio::sync::oneshot;

    async fn poll_pending(future: impl Future) {
        let mut future = pin!(future);
        poll_fn(|context| {
            assert!(future.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
    }

    async fn pending_flush(output: &mut Writer, fail: bool) -> mpsc::Sender<()> {
        let (started, receiver) = oneshot::channel();
        let (release, wait) = mpsc::channel();
        output.start(move |buffer| {
            let _ = started.send(());
            wait.recv().expect("flush gate released");
            if fail {
                return Err(io::Error::other("injected flush failure"));
            }
            buffer.flush()
        });
        receiver.await.unwrap();
        release
    }

    #[tokio::test]
    async fn cancelling_flush_keeps_data_and_allows_write_flush_or_finish() -> io::Result<()> {
        for resume in 0..3 {
            let directory = tempfile::tempdir()?;
            let path = directory.path().join("data");
            std::fs::write(&path, "old")?;
            let mut output = writer(&path).await?;
            output.write("one".to_owned()).await?;
            let release = pending_flush(&mut output, false).await;
            poll_pending(output.flush()).await;
            assert!(!output.poisoned);
            assert_eq!(std::fs::read_to_string(&path)?, "old");
            release.send(()).unwrap();
            if resume == 0 {
                output.write("two".to_owned()).await?;
            }
            if resume == 1 {
                output.flush().await?;
            }
            output.finish().await?;
            assert_eq!(
                std::fs::read_to_string(path)?,
                if resume == 0 { "onetwo" } else { "one" }
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_started_write_poisoning_keeps_old_target() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("data");
        std::fs::write(&path, "old")?;
        let mut output = writer(&path).await?;
        output.write("one".to_owned()).await?;
        let release = pending_flush(&mut output, false).await;
        poll_pending(output.write("two".to_owned())).await;
        assert!(output.poisoned);
        release.send(()).unwrap();
        assert!(output.write("three".to_owned()).await.is_err());
        assert!(output.finish().await.is_err());
        assert_eq!(std::fs::read_to_string(path)?, "old");
        Ok(())
    }

    #[tokio::test]
    async fn unpolled_write_does_not_poison() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("data");
        let mut output = writer(&path).await?;
        drop(output.write("not started".to_owned()));
        output.write("saved".to_owned()).await?;
        output.finish().await?;
        assert_eq!(std::fs::read_to_string(path)?, "saved");
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_flush_cannot_hide_a_later_io_failure() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("data");
        std::fs::write(&path, "old")?;
        let mut output = writer(&path).await?;
        output.write("one".to_owned()).await?;
        let release = pending_flush(&mut output, true).await;
        poll_pending(output.flush()).await;
        release.send(()).unwrap();
        assert!(output.finish().await.is_err());
        assert_eq!(std::fs::read_to_string(path)?, "old");
        Ok(())
    }

    #[tokio::test]
    async fn real_flush_error_does_not_publish_partial_output() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("data");
        std::fs::write(&path, "old")?;
        let mut output = writer(&path).await?;
        let state = output.state.as_mut().unwrap();
        let readonly = File::open(state.temp.as_ref().unwrap())?;
        state.output = Some(BufWriter::new(readonly));
        output.write("cannot flush this".to_owned()).await?;
        assert!(output.flush().await.is_err());
        assert!(output.finish().await.is_err());
        assert_eq!(std::fs::read_to_string(path)?, "old");
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_commit_keeps_the_lock_until_background_replacement_completes()
    -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("data");
        std::fs::write(&path, "old")?;
        let mut output = writer(&path).await?;
        output.write("new".to_owned()).await?;
        let (started, receiver) = oneshot::channel();
        let (release, wait) = mpsc::channel();
        output.state.as_mut().unwrap().commit_gate = Some((started, wait));
        let commit = tokio::spawn(output.finish());
        receiver.await.unwrap();
        commit.abort();
        let _ = commit.await;
        assert!(
            matches!(writer(&path).await, Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
        assert_eq!(std::fs::read_to_string(&path)?, "old");
        release.send(()).unwrap();
        let following =
            tokio::time::timeout(std::time::Duration::from_secs(5), editor(&path)).await??;
        assert_eq!(std::fs::read_to_string(&path)?, "new");
        drop(following);
        Ok(())
    }
}
