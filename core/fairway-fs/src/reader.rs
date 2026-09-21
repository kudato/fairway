use std::{io, path::Path};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

/// An open file with a shared position for byte and line reads.
pub struct Reader {
    input: BufReader<tokio::fs::File>,
    // Kept on the reader, not on the next_line future, so cancellation cannot
    // lose bytes already consumed from the underlying file.
    line: Vec<u8>,
    replay: usize,
}

/// Opens a file for sequential reading, following symbolic links and without a write lock.
pub async fn reader(path: impl AsRef<Path>) -> io::Result<Reader> {
    let file = tokio::fs::File::open(super::absolute(path.as_ref())?).await?;
    Ok(Reader::new(file))
}

impl Reader {
    pub(crate) fn new(file: tokio::fs::File) -> Self {
        Self {
            input: BufReader::new(file),
            line: Vec::new(),
            replay: 0,
        }
    }

    /// Reads into the buffer, returning the number of bytes filled. Zero means EOF or an empty buffer.
    pub async fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.replay < self.line.len() {
            let count = buffer.len().min(self.line.len() - self.replay);
            buffer[..count].copy_from_slice(&self.line[self.replay..self.replay + count]);
            self.replay += count;
            if self.replay == self.line.len() {
                self.line.clear();
                self.replay = 0;
            }
            return Ok(count);
        }
        self.input.read(buffer).await
    }

    /// Reads UTF-8 through LF or CRLF, without the line ending. Cancellation retains pending bytes.
    pub async fn next_line(&mut self) -> io::Result<Option<String>> {
        if self.replay != 0 {
            self.line.drain(..self.replay);
            self.replay = 0;
        }
        self.input.read_until(b'\n', &mut self.line).await?;
        if self.line.is_empty() {
            return Ok(None);
        }
        let mut bytes = std::mem::take(&mut self.line);
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
        }
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| super::decode_error(error.utf8_error()))
    }

    pub(crate) async fn read_all(&mut self) -> io::Result<Vec<u8>> {
        let mut bytes = self.line.split_off(self.replay);
        self.line.clear();
        self.replay = 0;
        self.input.read_to_end(&mut bytes).await?;
        Ok(bytes)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        future::{Future, poll_fn},
        io::Write,
        os::{fd::OwnedFd, unix::net::UnixStream},
        pin::pin,
        task::Poll,
        time::Duration,
    };

    #[tokio::test]
    async fn cancelled_partial_line_can_resume_as_a_line_or_bytes() -> io::Result<()> {
        for as_bytes in [false, true] {
            let (input, mut producer) = UnixStream::pair()?;
            let descriptor: OwnedFd = input.into();
            let file = std::fs::File::from(descriptor);
            let mut reader = Reader::new(tokio::fs::File::from_std(file));
            producer.write_all(b"prefix")?;
            tokio::time::timeout(Duration::from_secs(5), async {
                while reader.line.is_empty() {
                    {
                        let mut pending = pin!(reader.next_line());
                        poll_fn(|context| {
                            assert!(pending.as_mut().poll(context).is_pending());
                            Poll::Ready(())
                        })
                        .await;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            if as_bytes {
                let mut prefix = [0; 3];
                assert_eq!(reader.read(&mut prefix).await?, 3);
                assert_eq!(&prefix, b"pre");
            }
            producer.write_all("猫\r\nlast".as_bytes())?;
            drop(producer);
            assert_eq!(
                reader.next_line().await?,
                Some(if as_bytes { "fix猫" } else { "prefix猫" }.into())
            );
            assert_eq!(reader.next_line().await?, Some("last".into()));
            assert_eq!(reader.next_line().await?, None);
        }
        Ok(())
    }
}
