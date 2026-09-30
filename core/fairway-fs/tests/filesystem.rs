//! End-to-end filesystem behavior through the public API.

use std::{error::Error as _, io, path::Path, sync::Arc, time::Duration};

use fairway_codec::{Decode, Encode, Json, Toml};
use fairway_fs as fs;
use tokio::sync::{Notify, oneshot};

#[tokio::test]
async fn atomic_replacement_keeps_old_descriptors() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    fs::write(&path, "old\n".to_owned()).await?;
    let mut old = std::fs::File::open(&path)?;
    fs::write(&path, "new\nlast".to_owned()).await?;
    assert_eq!(fs::read::<String>(&path).await?, "new\nlast");
    let mut previous = String::new();
    std::io::Read::read_to_string(&mut old, &mut previous)?;
    assert_eq!(previous, "old\n");
    let missing = directory.path().join("new");
    fs::write(&missing, b"bytes".to_vec()).await?;
    assert_eq!(fs::read::<Vec<u8>>(&missing).await?, b"bytes");
    Ok(())
}

#[tokio::test]
async fn whole_writes_preserve_empty_and_large_buffers() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    // Sizes around 64 KiB, a common buffer size, and a short write after a
    // long one, which must not leave the old tail behind.
    for (index, length) in [0, 1, 65_535, 65_536, 65_537, 1_000_000, 7]
        .into_iter()
        .enumerate()
    {
        let expected = vec![index as u8; length];
        fs::write(&path, expected.clone()).await?;
        assert_eq!(fs::read::<Vec<u8>>(&path).await?, expected);
    }
    Ok(())
}

#[tokio::test]
async fn edits_wait_and_read_the_last_committed_version() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("counter");
    fs::write(&path, Json(0_u32)).await?;
    let mut handles = Vec::new();
    for _ in 0..12 {
        let path = path.clone();
        handles.push(tokio::spawn(async move {
            fs::edit(&path, |value: Json<u32>| async move {
                tokio::task::yield_now().await;
                Ok::<_, fs::Error>(Json(value.0 + 1))
            })
            .await
        }));
    }
    for handle in handles {
        handle.await??;
    }
    assert_eq!(fs::read::<Json<u32>>(path).await?.0, 12);
    Ok(())
}

#[tokio::test]
async fn writes_fail_when_busy_and_edits_wait_for_the_previous_edit() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    fs::write(&path, "old".to_owned()).await?;
    let (held, acquired) = oneshot::channel();
    let (release, wait_release) = oneshot::channel();
    let first_path = path.clone();
    let first = tokio::spawn(async move {
        fs::edit(first_path, |_: String| async move {
            let _ = held.send(());
            wait_release.await.map_err(io::Error::other)?;
            Ok::<_, anyhow::Error>("written".to_owned())
        })
        .await
    });
    acquired.await.unwrap();
    assert_eq!(
        fs::write(&path, "conflict".to_owned())
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    let (started, receiver) = oneshot::channel();
    let next_path = path.clone();
    let edit = tokio::spawn(async move {
        fs::edit(next_path, |text: String| async move {
            let _ = started.send(text.clone());
            Ok::<_, fs::Error>(format!("{text}!"))
        })
        .await
    });
    tokio::task::yield_now().await;
    assert!(!edit.is_finished());
    release.send(()).unwrap();
    first.await??;
    assert_eq!(receiver.await.unwrap(), "written");
    edit.await??;
    assert_eq!(fs::read::<String>(path).await?, "written!");
    Ok(())
}

#[tokio::test]
async fn busy_edit_rejects_write_and_can_be_cancelled_without_publishing() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    fs::write(&path, "old".to_owned()).await?;
    let held = Arc::new(Notify::new());
    let held_copy = held.clone();
    let edit_path = path.clone();
    let edit = tokio::spawn(async move {
        fs::edit(edit_path, |_: String| async move {
            held_copy.notify_one();
            std::future::pending::<Result<String, fs::Error>>().await
        })
        .await
    });
    held.notified().await;
    assert_eq!(
        fs::write(&path, "conflict".to_owned())
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    edit.abort();
    let _ = edit.await;
    tokio::time::timeout(
        Duration::from_secs(5),
        fs::edit(
            &path,
            |text: String| async move { Ok::<_, fs::Error>(text) },
        ),
    )
    .await??;
    assert_eq!(fs::read::<String>(path).await?, "old");
    Ok(())
}

#[derive(Debug)]
struct EncodingFailure;
impl std::fmt::Display for EncodingFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cannot encode")
    }
}
impl std::error::Error for EncodingFailure {}
/// Decodes from any bytes and always fails to encode.
struct Invalid;
impl Decode for Invalid {
    type Error = std::convert::Infallible;
    fn decode(_: Vec<u8>) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}
impl Encode for Invalid {
    type Error = EncodingFailure;
    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        Err(EncodingFailure)
    }
}

#[tokio::test]
async fn readonly_files_reject_writes_before_encoding_or_editing() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("readonly");
    std::fs::write(&path, "old")?;
    let permissions = std::fs::metadata(&path)?.permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&path, readonly)?;

    // A privileged process, such as one running as root, may write to the file
    // anyway, and then there is nothing to test.
    let native = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path);
    if native.is_ok() {
        std::fs::set_permissions(&path, permissions)?;
        return Ok(());
    }

    let write = fs::write(&path, Invalid).await;
    let mut handler_ran = false;
    let edit = fs::edit(&path, |text: String| {
        handler_ran = true;
        async move { Ok::<_, fs::Error>(text) }
    })
    .await;
    let contents = fs::read::<String>(&path).await;
    let entries = std::fs::read_dir(directory.path())?.count();
    // Restore the permissions before asserting: on Windows, the temporary
    // directory cannot be removed while it contains a read-only file.
    std::fs::set_permissions(&path, permissions)?;

    assert_eq!(native.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(write.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(edit.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert!(!handler_ran);
    assert_eq!(contents?, "old");
    assert_eq!(entries, 1);

    fs::write(&path, "after".to_owned()).await?;
    assert_eq!(fs::read::<String>(&path).await?, "after");
    Ok(())
}

#[tokio::test]
async fn conversion_failures_preserve_original() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    fs::write(&path, "old".to_owned()).await?;
    let error = fs::write(&path, Invalid).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(error.operation(), fs::Operation::Encode);
    assert_eq!(error.path(), Some(std::fs::canonicalize(&path)?.as_path()));
    assert!(error.source().unwrap().is::<EncodingFailure>());
    assert_eq!(fs::read::<String>(&path).await?, "old");
    let error = fs::edit(
        &path,
        |value: Invalid| async move { Ok::<_, fs::Error>(value) },
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(error.source().unwrap().is::<EncodingFailure>());
    assert_eq!(fs::read::<String>(&path).await?, "old");
    let error = fs::edit(&path, |_: String| async {
        Err::<String, _>(anyhow::Error::msg("handler failed"))
    })
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "handler failed");
    assert_eq!(fs::read::<String>(&path).await?, "old");
    assert_eq!(
        fs::read::<Json<u32>>(&path).await.unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        fs::read::<String>(directory.path().join("absent"))
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
    fs::write(&path, "after".to_owned()).await?;
    Ok(())
}

/// A panic in a codec reaches the task that awaits the operation, and the file
/// is left unchanged and unlocked.
///
/// Each operation runs in a spawned task, so that its panic ends up in the
/// task's `JoinError` instead of failing the test.
mod codec_panics {
    use super::*;
    use std::{convert::Infallible, panic::panic_any};

    #[derive(Debug, PartialEq)]
    enum CodecPanic {
        Decode,
        Encode,
    }

    #[derive(Debug)]
    struct PanickingDecode;

    impl Decode for PanickingDecode {
        type Error = Infallible;

        fn decode(_: Vec<u8>) -> Result<Self, Self::Error> {
            panic_any(CodecPanic::Decode);
        }
    }

    impl Encode for PanickingDecode {
        type Error = Infallible;

        fn encode(self) -> Result<Vec<u8>, Self::Error> {
            Ok(b"unexpected".to_vec())
        }
    }

    struct PanickingEncode;

    impl Decode for PanickingEncode {
        type Error = Infallible;

        fn decode(_: Vec<u8>) -> Result<Self, Self::Error> {
            Ok(Self)
        }
    }

    impl Encode for PanickingEncode {
        type Error = Infallible;

        fn encode(self) -> Result<Vec<u8>, Self::Error> {
            panic_any(CodecPanic::Encode);
        }
    }

    fn assert_panic(error: tokio::task::JoinError, expected: CodecPanic) {
        assert!(error.is_panic());
        assert_eq!(
            *error.into_panic().downcast::<CodecPanic>().unwrap(),
            expected
        );
    }

    /// Checks that `path` has its old contents, that its lock is free, and
    /// that no temporary file is left next to it.
    async fn assert_original_and_cleanup(path: &Path) -> anyhow::Result<()> {
        assert_eq!(std::fs::read(path)?, b"old");
        tokio::time::timeout(
            Duration::from_secs(5),
            fs::edit(path, |text: String| async move {
                assert_eq!(text, "old");
                Ok::<_, fs::Error>("following".to_owned())
            }),
        )
        .await??;
        assert_eq!(std::fs::read(path)?, b"following");
        assert_eq!(std::fs::read_dir(path.parent().unwrap())?.count(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn read_propagates_decode_panic() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("data");
        std::fs::write(&path, b"old")?;

        let error = tokio::spawn(fs::read::<PanickingDecode>(path.clone()))
            .await
            .unwrap_err();
        assert_panic(error, CodecPanic::Decode);
        assert_eq!(std::fs::read(&path)?, b"old");
        Ok(())
    }

    #[tokio::test]
    async fn write_propagates_encode_panic_and_preserves_original() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("data");
        std::fs::write(&path, b"old")?;

        let error = tokio::spawn(fs::write(path.clone(), PanickingEncode))
            .await
            .unwrap_err();
        assert_panic(error, CodecPanic::Encode);
        assert_original_and_cleanup(&path).await
    }

    #[tokio::test]
    async fn edit_propagates_decode_panic_and_skips_handler() -> anyhow::Result<()> {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

        let directory = tempfile::tempdir()?;
        let path = directory.path().join("data");
        std::fs::write(&path, b"old")?;
        let ran = Arc::new(AtomicBool::new(false));
        let handler_ran = ran.clone();

        let error = tokio::spawn(fs::edit(path.clone(), move |value: PanickingDecode| {
            handler_ran.store(true, SeqCst);
            async move { Ok::<_, fs::Error>(value) }
        }))
        .await
        .unwrap_err();
        assert_panic(error, CodecPanic::Decode);
        assert!(!ran.load(SeqCst));
        assert_original_and_cleanup(&path).await
    }

    #[tokio::test]
    async fn edit_propagates_encode_panic_and_preserves_original() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("data");
        std::fs::write(&path, b"old")?;

        let error = tokio::spawn(fs::edit(
            path.clone(),
            |value: PanickingEncode| async move { Ok::<_, fs::Error>(value) },
        ))
        .await
        .unwrap_err();
        assert_panic(error, CodecPanic::Encode);
        assert_original_and_cleanup(&path).await
    }
}

#[tokio::test]
async fn edit_handler_can_borrow_context_and_return_a_non_send_error() -> anyhow::Result<()> {
    use std::{cell::Cell, rc::Rc};

    #[derive(Debug)]
    struct HandlerError(Rc<io::Error>);

    impl From<fs::Error> for HandlerError {
        fn from(error: fs::Error) -> Self {
            Self(Rc::new(io::Error::other(error)))
        }
    }

    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    fs::write(&path, "old".to_owned()).await?;
    let calls = Rc::new(Cell::new(0));
    let error = fs::edit(&path, |_: String| {
        calls.set(calls.get() + 1);
        async {
            tokio::task::yield_now().await;
            Err::<String, _>(HandlerError(Rc::new(io::Error::other("handler failed"))))
        }
    })
    .await
    .unwrap_err();
    assert_eq!(calls.get(), 1);
    assert_eq!(error.0.to_string(), "handler failed");
    assert_eq!(fs::read::<String>(&path).await?, "old");
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
    fs::write(&path, "after".to_owned()).await?;
    Ok(())
}

#[tokio::test]
async fn edit_saves_handler_output_and_requires_an_existing_file() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    assert_eq!(
        fs::edit(
            &path,
            |text: String| async move { Ok::<_, fs::Error>(text) }
        )
        .await
        .unwrap_err()
        .kind(),
        io::ErrorKind::NotFound
    );
    fs::write(&path, "1\n2\n3".to_owned()).await?;
    fs::edit(&path, |text: String| async move {
        Ok::<_, fs::Error>(format!("{}\n", text.lines().next().unwrap()))
    })
    .await?;
    assert_eq!(fs::read::<String>(&path).await?, "1\n");
    fs::edit(&path, |_: String| async {
        Ok::<_, fs::Error>(String::new())
    })
    .await?;
    assert_eq!(fs::read::<String>(&path).await?, "");
    Ok(())
}

#[tokio::test]
async fn edit_decode_errors_preserve_the_file_and_skip_the_handler() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("bytes");
    fs::write(&path, [0xff, b'\n']).await?;
    let mut handler_ran = false;
    let result = fs::edit(&path, |_: String| {
        handler_ran = true;
        async { Ok::<_, fs::Error>("replacement".to_owned()) }
    })
    .await;
    let error = result.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    let codec_error = error
        .source()
        .unwrap()
        .downcast_ref::<fairway_codec::Error>()
        .unwrap();
    assert!(matches!(
        codec_error,
        fairway_codec::Error::Decode {
            format: "text",
            line: Some(1),
            column: Some(1),
            source,
        } if source.is::<std::str::Utf8Error>()
    ));
    assert!(!handler_ran);
    assert_eq!(fs::read::<Vec<u8>>(&path).await?, [0xff, b'\n']);
    Ok(())
}

#[tokio::test]
async fn cancelled_decoding_preserves_the_file_and_never_calls_the_handler() -> anyhow::Result<()> {
    use std::{
        convert::Infallible,
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering::SeqCst},
            mpsc,
        },
    };

    struct DecodeGate {
        started: oneshot::Sender<()>,
        release: mpsc::Receiver<()>,
        discarded: oneshot::Sender<()>,
    }
    // `decode` receives only the bytes, so the channels reach it through a
    // static.
    static GATE: Mutex<Option<DecodeGate>> = Mutex::new(None);

    struct Document(Option<oneshot::Sender<()>>);
    impl Decode for Document {
        type Error = Infallible;

        fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
            assert_eq!(bytes, b"old");
            let gate = GATE.lock().unwrap().take().unwrap();
            let _ = gate.started.send(());
            gate.release
                .recv_timeout(Duration::from_secs(10))
                .expect("the test releases the decoder");
            Ok(Self(Some(gate.discarded)))
        }
    }
    impl Encode for Document {
        type Error = Infallible;

        fn encode(self) -> Result<Vec<u8>, Self::Error> {
            Ok(b"cancelled".to_vec())
        }
    }
    // Dropping the decoded document reports that the cancelled edit has
    // discarded it.
    impl Drop for Document {
        fn drop(&mut self) {
            if let Some(discarded) = self.0.take() {
                let _ = discarded.send(());
            }
        }
    }

    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    std::fs::write(&path, b"old")?;
    let (started, wait_started) = oneshot::channel();
    let (release, wait_release) = mpsc::channel();
    let (discarded, wait_discarded) = oneshot::channel();
    *GATE.lock().unwrap() = Some(DecodeGate {
        started,
        release: wait_release,
        discarded,
    });
    let ran = Arc::new(AtomicBool::new(false));
    let handler_ran = ran.clone();
    let edit_path = path.clone();
    let task = tokio::spawn(async move {
        fs::edit(edit_path, |value: Document| {
            handler_ran.store(true, SeqCst);
            async move { Ok::<_, fs::Error>(value) }
        })
        .await
    });

    tokio::time::timeout(Duration::from_secs(5), wait_started)
        .await?
        .unwrap();
    task.abort();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await?
            .unwrap_err()
            .is_cancelled()
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), wait_discarded)
        .await?
        .unwrap();
    assert!(!ran.load(SeqCst));
    assert_eq!(std::fs::read(&path)?, b"old");

    tokio::time::timeout(
        Duration::from_secs(5),
        fs::edit(&path, |text: String| async move {
            assert_eq!(text, "old");
            Ok::<_, fs::Error>("following".to_owned())
        }),
    )
    .await??;
    assert!(!ran.load(SeqCst));
    assert_eq!(std::fs::read(&path)?, b"following");
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
    Ok(())
}

#[tokio::test]
async fn directories_metadata_and_temporary_resources() -> anyhow::Result<()> {
    let directory = fs::temp_dir().await?;
    fs::mkdir(directory.path().join("a/b")).await?;
    fs::mkdir(directory.path().join("a/b")).await?;
    fs::write(directory.path().join(".hidden"), "abc".to_owned()).await?;
    let mut entries = fs::ls(&directory).await?;
    let mut names = Vec::new();
    while let Some(entry) = entries.next().await? {
        if entry.file_type().await?.is_file() {
            assert_eq!(fs::metadata(entry.path()).await?.len(), 3);
        }
        names.push(entry.file_name());
    }
    names.sort();
    assert_eq!(names, [".hidden", "a"]);
    drop(entries);
    let file = fs::temp_file().await?;
    // `write` puts a new file at the path; `close` removes it anyway, because
    // a `TempFile` owns the path rather than a particular file.
    fs::write(&file, "replaced temporary inode".to_owned()).await?;
    let file_path = file.path().to_owned();
    file.close().await?;
    assert!(!fs::exists(file_path).await?);
    let directory_path = directory.path().to_owned();
    directory.close().await?;
    assert!(!fs::exists(directory_path).await?);
    Ok(())
}

#[tokio::test]
async fn codecs_and_custom_anyhow_errors_work_through_fs() -> anyhow::Result<()> {
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Dataset {
        name: String,
    }
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data.toml");
    fs::write(
        &path,
        Toml(Dataset {
            name: "corpus".into(),
        }),
    )
    .await?;
    let data: Toml<Dataset> = fs::read(&path).await?;
    assert_eq!(data.0.name, "corpus");
    struct Custom;
    impl Decode for Custom {
        type Error = anyhow::Error;
        fn decode(_: Vec<u8>) -> anyhow::Result<Self> {
            anyhow::bail!("custom decoder failed")
        }
    }
    assert!(
        matches!(fs::read::<Custom>(&path).await, Err(error) if error.kind() == io::ErrorKind::InvalidData && error.source().unwrap().to_string() == "custom decoder failed")
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn file_conversions_run_in_workers_and_handler_runs_in_the_calling_task() -> anyhow::Result<()>
{
    use std::{sync::Mutex, thread::ThreadId};

    // Set only in the calling task: the codecs must not see it, and the
    // handler must.
    tokio::task_local! {
        static CALLER: ();
    }
    static CALLS: Mutex<Vec<(&str, ThreadId)>> = Mutex::new(Vec::new());
    struct Document(String);

    impl Decode for Document {
        type Error = fairway_codec::Error;

        fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
            assert!(CALLER.try_with(|_| ()).is_err());
            CALLS
                .lock()
                .unwrap()
                .push(("decode", std::thread::current().id()));
            String::decode(bytes).map(Self)
        }
    }
    impl Encode for Document {
        type Error = std::convert::Infallible;

        fn encode(self) -> Result<Vec<u8>, Self::Error> {
            assert!(CALLER.try_with(|_| ()).is_err());
            CALLS
                .lock()
                .unwrap()
                .push(("encode", std::thread::current().id()));
            self.0.encode()
        }
    }

    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    let caller_thread = std::thread::current().id();
    CALLER
        .scope((), async {
            fs::write(&path, Document("old".to_owned())).await?;
            assert_eq!(fs::read::<Document>(&path).await?.0, "old");
            let suffix = " and new".to_owned();
            let suffix = &suffix;
            fs::edit(&path, |mut value: Document| async move {
                CALLER.with(|_| ());
                assert_eq!(std::thread::current().id(), caller_thread);
                tokio::task::yield_now().await;
                CALLER.with(|_| ());
                value.0.push_str(suffix);
                Ok::<_, fs::Error>(value)
            })
            .await?;
            assert_eq!(fs::read::<Document>(&path).await?.0, "old and new");
            Ok::<_, fs::Error>(())
        })
        .await?;
    let calls = CALLS.lock().unwrap();
    // `write`, `read`, `edit` (decode and encode), and `read` again.
    assert_eq!(
        calls
            .iter()
            .map(|&(operation, _)| operation)
            .collect::<Vec<_>>(),
        ["encode", "decode", "decode", "encode", "decode"]
    );
    assert!(calls.iter().all(|&(_, thread)| thread != caller_thread));
    Ok(())
}

#[test]
fn resource_handles_and_standard_operation_futures_are_send() {
    // The check happens at compile time: the futures are created but never
    // polled.
    fn send<T: Send>() {}
    fn future_send(_: impl std::future::Future + Send) {}
    send::<fs::DirEntries>();
    send::<fs::TempFile>();
    send::<fs::TempDir>();
    future_send(fs::ls(Path::new("directory")));
    future_send(fs::write(Path::new("data"), "value".to_owned()));
    future_send(fs::edit(Path::new("data"), |text: String| async move {
        Ok::<_, fs::Error>(text)
    }));
}

#[cfg(windows)]
#[tokio::test]
async fn windows_alternate_streams_survive_file_replacement() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    std::fs::write(&path, "old")?;
    let stream = path.with_file_name("data:fairway-test");
    std::fs::write(&stream, "stream metadata")?;
    fs::write(&path, "new".to_owned()).await?;
    assert_eq!(std::fs::read_to_string(&stream)?, "stream metadata");
    assert_eq!(fs::read::<String>(&path).await?, "new");
    Ok(())
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    #[tokio::test]
    async fn links_are_rejected_for_writes_and_parent_aliases_share_a_lock() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("original");
        let link = directory.path().join("link");
        fs::write(&path, "old".to_owned()).await?;
        symlink(&path, &link)?;
        assert_eq!(fs::read::<String>(&link).await?, "old");
        assert_eq!(
            fs::write(&link, "bad".to_owned()).await.unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        let broken = directory.path().join("broken");
        symlink("absent", &broken)?;
        assert!(!fs::exists(&broken).await?);
        assert_eq!(
            fs::write(&broken, "bad".to_owned())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        // `alias/original` resolves to the same target as `path`, so a write
        // through it must find the lock of the running edit.
        let alias = directory.path().join("alias");
        symlink(directory.path(), &alias)?;
        fs::edit(&path, |text: String| async move {
            assert_eq!(
                fs::write(alias.join("original"), "bad".to_owned())
                    .await
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::WouldBlock
            );
            Ok::<_, fs::Error>(text)
        })
        .await?;
        let mut entries = fs::ls(&directory).await?;
        let mut found = false;
        while let Some(entry) = entries.next().await? {
            if entry.file_name() == "link" {
                assert!(entry.file_type().await?.is_symlink());
                found = true;
            }
        }
        assert!(found);
        Ok(())
    }

    #[tokio::test]
    async fn replacement_breaks_hardlinks_and_preserves_mode_owner_and_xattrs() -> anyhow::Result<()>
    {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("original");
        let link = directory.path().join("hardlink");
        std::fs::write(&path, "old")?;
        std::fs::hard_link(&path, &link)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))?;
        #[cfg(target_os = "macos")]
        let attribute = "com.fairway.test";
        #[cfg(not(target_os = "macos"))]
        let attribute = "user.fairway-test";
        xattr::set(&path, attribute, b"value")?;
        let before = std::fs::metadata(&path)?;
        fs::write(&path, "new".to_owned()).await?;
        let after = std::fs::metadata(&path)?;
        assert_ne!(before.ino(), after.ino());
        assert_eq!(
            (before.uid(), before.gid(), before.mode() & 0o7777),
            (after.uid(), after.gid(), after.mode() & 0o7777)
        );
        assert_eq!(xattr::get(&path, attribute)?, Some(b"value".to_vec()));
        assert_eq!(std::fs::read_to_string(&link)?, "old");
        assert_eq!(fs::read::<String>(&path).await?, "new");
        Ok(())
    }

    #[tokio::test]
    async fn new_file_permissions_follow_the_normal_umask() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let normal = directory.path().join("normal");
        let fairway = directory.path().join("fairway");
        std::fs::write(&normal, "")?;
        fs::write(&fairway, "".to_owned()).await?;
        assert_eq!(
            std::fs::metadata(normal)?.mode() & 0o777,
            std::fs::metadata(fairway)?.mode() & 0o777
        );
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_case_and_unicode_aliases_share_locks_even_before_creation() -> anyhow::Result<()>
    {
        struct GatedEncode {
            started: oneshot::Sender<()>,
            release: std::sync::mpsc::Receiver<()>,
        }

        impl Encode for GatedEncode {
            type Error = std::convert::Infallible;

            fn encode(self) -> Result<Vec<u8>, Self::Error> {
                let _ = self.started.send(());
                // Encoding runs under the lock, so pausing it keeps the lock
                // held while the test tries the alias.
                let _ = self.release.recv();
                Ok(b"new".to_vec())
            }
        }

        let directory = tempfile::tempdir()?;
        let path = directory.path().join("Café");
        let alias = directory.path().join("CAFE\u{301}");
        let (started, wait_started) = oneshot::channel();
        let (release, wait_release) = std::sync::mpsc::channel();
        let first = tokio::spawn(fs::write(
            path.clone(),
            GatedEncode {
                started,
                release: wait_release,
            },
        ));
        tokio::time::timeout(Duration::from_secs(5), wait_started)
            .await?
            .unwrap();
        assert!(!path.exists());
        assert_eq!(
            fs::write(&alias, "conflict".to_owned())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        release.send(()).unwrap();
        first.await??;
        assert_eq!(fs::read::<String>(&alias).await?, "new");
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_acl_entries_survive_replacement() -> anyhow::Result<()> {
        use std::process::Command;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("acl");
        std::fs::write(&path, "old")?;
        let result = Command::new("chmod")
            .args(["+a", "everyone allow read"])
            .arg(&path)
            .output()?;
        assert!(result.status.success(), "{result:?}");
        let acl = || -> io::Result<Vec<String>> {
            let output = Command::new("ls").arg("-le").arg(&path).output()?;
            assert!(output.status.success());
            // The ACL entries follow the line that describes the file.
            Ok(String::from_utf8_lossy(&output.stdout)
                .lines()
                .skip(1)
                .map(str::to_owned)
                .collect())
        };
        let before = acl()?;
        assert!(!before.is_empty());
        fs::write(&path, "new".to_owned()).await?;
        assert_eq!(acl()?, before);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_writes_and_edits_preserve_creation_time_and_flags() -> anyhow::Result<()> {
        use std::{
            fs::{File, FileTimes},
            os::macos::fs::{FileTimesExt, MetadataExt as _},
            process::Command,
            time::UNIX_EPOCH,
        };
        let directory = tempfile::tempdir()?;
        let created = UNIX_EPOCH + Duration::new(1_500_000_000, 123_456_789);
        let modified = UNIX_EPOCH + Duration::new(1_600_000_000, 987_654_321);
        for editing in [false, true] {
            let path = directory
                .path()
                .join(if editing { "edit" } else { "write" });
            std::fs::write(&path, "old")?;
            File::options()
                .write(true)
                .open(&path)?
                .set_times(FileTimes::new().set_created(created).set_modified(modified))?;
            let result = Command::new("chflags")
                .arg("hidden,nodump")
                .arg(&path)
                .output()?;
            assert!(result.status.success(), "{result:?}");
            let before = std::fs::metadata(&path)?;
            if editing {
                fs::edit(&path, |text: String| async move {
                    assert_eq!(text, "old");
                    Ok::<_, fs::Error>("new".to_owned())
                })
                .await?;
            } else {
                fs::write(&path, "new".to_owned()).await?;
            }
            let after = std::fs::metadata(&path)?;
            assert_eq!(after.created()?, before.created()?);
            assert_eq!(after.st_flags(), before.st_flags());
            assert!(after.modified()? > before.modified()?);
            assert_eq!(std::fs::read_to_string(&path)?, "new");
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_writes_and_edits_accept_creation_time_rounding() -> anyhow::Result<()> {
        use std::{
            fs::{File, FileTimes},
            os::macos::fs::FileTimesExt,
        };

        let directory = tempfile::tempdir()?;
        let reference = File::create(directory.path().join("reference"))?;
        for editing in [false, true] {
            let path = directory
                .path()
                .join(if editing { "edit" } else { "write" });
            // On exFAT, a newly created file can have a more precise creation
            // time than the filesystem can preserve when setting it later.
            std::fs::write(&path, "old")?;
            let created = std::fs::metadata(&path)?.created()?;
            reference.set_times(FileTimes::new().set_created(created))?;
            let expected = reference.metadata()?.created()?;

            if editing {
                fs::edit(&path, |text: String| async move {
                    assert_eq!(text, "old");
                    Ok::<_, fs::Error>("new".to_owned())
                })
                .await?;
            } else {
                fs::write(&path, "new".to_owned()).await?;
            }
            assert_eq!(std::fs::metadata(&path)?.created()?, expected);
            assert_eq!(std::fs::read_to_string(&path)?, "new");
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_protected_files_fail_without_leaking_temporaries() -> anyhow::Result<()> {
        use std::process::Command;
        for flag in ["uchg", "uappnd"] {
            let directory = tempfile::tempdir()?;
            let path = directory.path().join("protected");
            std::fs::write(&path, "old")?;
            let result = Command::new("chflags").arg(flag).arg(&path).output()?;
            assert!(result.status.success(), "{result:?}");
            let result = fs::write(&path, "new".to_owned()).await;
            let entries = std::fs::read_dir(directory.path())?.count();
            // Clear the flags of the whole directory before asserting: a
            // protected temporary file left by a bug would keep the directory
            // from being removed.
            let cleanup = Command::new("chflags")
                .args(["-R", "0"])
                .arg(directory.path())
                .output()?;
            assert!(cleanup.status.success(), "{cleanup:?}");
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(std::fs::read_to_string(&path)?, "old");
            assert_eq!(entries, 1);
        }
        Ok(())
    }
}
