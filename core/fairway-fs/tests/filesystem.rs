//! End-to-end filesystem behavior through the public API.

use std::{io, path::Path, sync::Arc, time::Duration};

use fairway_codec::{Decode, Encode, Json, Toml};
use fairway_fs as fs;
use tokio::sync::{Notify, oneshot};

#[tokio::test]
async fn atomic_replacement_keeps_old_readers_and_only_publishes_at_finish() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    fs::write(&path, "old\n".to_owned()).await?;
    let mut old = fs::reader(&path).await?;
    let mut output = fs::writer(&path).await?;
    output.write("new\n".to_owned()).await?;
    output.flush().await?;
    assert_eq!(fs::read::<String>(&path).await?, "old\n");
    output.write("last".to_owned()).await?;
    output.finish().await?;
    assert_eq!(fs::read::<String>(&path).await?, "new\nlast");
    assert_eq!(old.next_line().await?, Some("old".into()));
    assert_eq!(old.next_line().await?, None);
    let missing = directory.path().join("new");
    let mut output = fs::writer(&missing).await?;
    output.write(b"bytes".to_vec()).await?;
    output.flush().await?;
    assert!(!fs::exists(&missing).await?);
    output.finish().await?;
    assert_eq!(fs::read::<Vec<u8>>(&missing).await?, b"bytes");
    Ok(())
}

#[tokio::test]
async fn chunks_larger_than_the_buffer_keep_order_without_manual_flushes() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("chunks");
    let mut output = fs::writer(&path).await?;
    let mut expected = Vec::new();
    for (index, length) in [1, 65_535, 65_536, 65_537, 1_000_000, 7]
        .into_iter()
        .enumerate()
    {
        let part = vec![index as u8; length];
        output.write(part.clone()).await?;
        expected.extend(part);
    }
    output.finish().await?;
    assert_eq!(fs::read::<Vec<u8>>(&path).await?, expected);
    Ok(())
}

#[tokio::test]
async fn lines_and_bytes_share_a_position_and_support_crlf_and_empty_lines() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("lines");
    fs::write(&path, "prefix猫\r\n\nend".to_owned()).await?;
    let mut input = fs::reader(path).await?;
    let mut prefix = [0; 6];
    assert_eq!(input.read(&mut prefix).await?, 6);
    assert_eq!(&prefix, b"prefix");
    assert_eq!(input.next_line().await?, Some("猫".into()));
    assert_eq!(input.next_line().await?, Some(String::new()));
    assert_eq!(input.read(&mut []).await?, 0);
    assert_eq!(input.next_line().await?, Some("end".into()));
    assert_eq!(input.next_line().await?, None);
    Ok(())
}

#[tokio::test]
async fn reader_errors_end_reading() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("lines");
    fs::write(&path, b"\xff\nnext\n".to_vec()).await?;
    let mut input = fs::reader(path).await?;
    assert_eq!(
        input.next_line().await.unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(input.next_line().await.is_err());
    assert!(input.read(&mut [0; 4]).await.is_err());
    Ok(())
}

#[tokio::test]
async fn edits_wait_and_read_the_last_committed_version() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("counter");
    fs::write(&path, Json(0_u32)).await?;
    let mut handles = Vec::new();
    for _ in 0..12 {
        let path = path.clone();
        handles.push(tokio::spawn(async move {
            fs::edit(&path, |value: Json<u32>| async move {
                tokio::task::yield_now().await;
                Ok::<_, io::Error>(Json(value.0 + 1))
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
async fn writes_fail_when_busy_and_edits_wait_for_a_streaming_writer() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    fs::write(&path, "old".to_owned()).await?;
    let mut output = fs::writer(&path).await?;
    assert_eq!(
        fs::write(&path, "conflict".to_owned())
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    assert!(
        matches!(fs::writer(&path).await, Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );
    let (started, receiver) = oneshot::channel();
    let next_path = path.clone();
    let edit = tokio::spawn(async move {
        fs::edit(next_path, |text: String| async move {
            let _ = started.send(text.clone());
            Ok::<_, io::Error>(format!("{text}!"))
        })
        .await
    });
    tokio::task::yield_now().await;
    assert!(!edit.is_finished());
    output.write("written".to_owned()).await?;
    output.finish().await?;
    assert_eq!(receiver.await.unwrap(), "written");
    edit.await??;
    assert_eq!(fs::read::<String>(path).await?, "written!");
    Ok(())
}

#[tokio::test]
async fn busy_edit_rejects_write_and_can_be_cancelled_without_publishing() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    fs::write(&path, "old".to_owned()).await?;
    let held = Arc::new(Notify::new());
    let held_copy = held.clone();
    let edit_path = path.clone();
    let edit = tokio::spawn(async move {
        fs::edit(edit_path, |_: String| async move {
            held_copy.notify_one();
            std::future::pending::<io::Result<String>>().await
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
            |text: String| async move { Ok::<_, io::Error>(text) },
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
struct Invalid;
impl Encode for Invalid {
    type Error = EncodingFailure;
    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        Err(EncodingFailure)
    }
}

#[tokio::test]
async fn readonly_files_reject_writes_before_encoding_or_editing() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("readonly");
    std::fs::write(&path, "old")?;
    let permissions = std::fs::metadata(&path)?.permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&path, readonly)?;

    // Privileged processes may still have write access; the OS decides.
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
        async move { Ok::<_, io::Error>(text) }
    })
    .await;
    let writer = fs::writer(&path).await.err();
    let editor = fs::editor(&path).await.err();
    let contents = fs::read::<String>(&path).await;
    let entries = std::fs::read_dir(directory.path())?.count();
    // Restore permissions before assertions so Windows can remove the fixture.
    std::fs::set_permissions(&path, permissions)?;

    assert_eq!(native.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(write.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(edit.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(writer.unwrap().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(editor.unwrap().kind(), io::ErrorKind::PermissionDenied);
    assert!(!handler_ran);
    assert_eq!(contents?, "old");
    assert_eq!(entries, 1);

    fs::write(&path, "after".to_owned()).await?;
    assert_eq!(fs::read::<String>(&path).await?, "after");
    Ok(())
}

#[tokio::test]
async fn conversion_failures_preserve_original_and_poison_streaming_writes() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    fs::write(&path, "old".to_owned()).await?;
    let error = fs::write(&path, Invalid).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(error.get_ref().unwrap().is::<EncodingFailure>());
    let mut output = fs::editor(&path).await?;
    output.write("first".to_owned()).await?;
    assert_eq!(
        output.write(Invalid).await.unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert!(output.next_line().await.is_err());
    assert!(output.finish().await.is_err());
    assert_eq!(fs::read::<String>(&path).await?, "old");
    let error = fs::edit(&path, |_: String| async {
        Err::<String, _>(io::Error::other("handler failed"))
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
    Ok(())
}

#[tokio::test]
async fn editor_saves_only_explicit_output_and_requires_an_existing_file() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    assert!(
        matches!(fs::editor(&path).await, Err(error) if error.kind() == io::ErrorKind::NotFound)
    );
    fs::write(&path, "first\nsecond\nthird".to_owned()).await?;
    let mut editor = fs::editor(&path).await?;
    let line = editor.next_line().await?.unwrap_or_default();
    editor.write(line).await?;
    editor.finish().await?;
    assert_eq!(fs::read::<String>(&path).await?, "first");
    fs::editor(&path).await?.finish().await?;
    assert_eq!(fs::read::<String>(&path).await?, "");
    Ok(())
}

#[tokio::test]
async fn editor_read_errors_prevent_replacement() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("bytes");
    fs::write(&path, [0xff, b'\n']).await?;
    let mut editor = fs::editor(&path).await?;
    editor.write("replacement".to_owned()).await?;
    assert_eq!(
        editor.next_line().await.unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(editor.read(&mut [0; 1]).await.is_err());
    assert!(editor.finish().await.is_err());
    assert_eq!(fs::read::<Vec<u8>>(&path).await?, [0xff, b'\n']);
    Ok(())
}

#[tokio::test]
async fn directories_metadata_and_temporary_resources() -> io::Result<()> {
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
        matches!(fs::read::<Custom>(&path).await, Err(error) if error.kind() == io::ErrorKind::InvalidData && error.to_string() == "custom decoder failed")
    );
    Ok(())
}

struct ConversionGate {
    started: oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
    finished: oneshot::Sender<()>,
}

impl ConversionGate {
    fn new() -> (
        Self,
        oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
        oneshot::Receiver<()>,
    ) {
        let (started, wait_started) = oneshot::channel();
        let (release, wait_release) = std::sync::mpsc::channel();
        let (finished, wait_finished) = oneshot::channel();
        (
            Self {
                started,
                release: wait_release,
                finished,
            },
            wait_started,
            release,
            wait_finished,
        )
    }

    fn wait(self) -> oneshot::Sender<()> {
        let _ = self.started.send(());
        // Dropping the sender also releases the worker if a test fails.
        let _ = self.release.recv();
        self.finished
    }
}

impl Encode for ConversionGate {
    type Error = std::convert::Infallible;

    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        let finished = self.wait();
        let bytes = b"cancelled".to_vec();
        let _ = finished.send(());
        Ok(bytes)
    }
}

#[tokio::test]
async fn cancelled_encoding_preserves_the_file_and_allows_later_writes() -> io::Result<()> {
    for whole_file in [false, true] {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("data");
        std::fs::write(&path, "old")?;
        let (gated, started, release, finished) = ConversionGate::new();
        let task_path = path.clone();
        let task = tokio::spawn(async move {
            if whole_file {
                fs::write(task_path, gated).await
            } else {
                let mut output = fs::writer(task_path).await?;
                output.write(gated).await?;
                output.finish().await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), started)
            .await?
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), finished)
            .await?
            .unwrap();
        let mut following =
            tokio::time::timeout(Duration::from_secs(5), fs::editor(&path)).await??;
        assert_eq!(std::fs::read(&path)?, b"old");
        following.write(b"following".to_vec()).await?;
        following.finish().await?;
        assert_eq!(std::fs::read(&path)?, b"following");
    }
    Ok(())
}

#[tokio::test]
async fn cancelling_encoding_prevents_publication_of_earlier_chunks() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    std::fs::write(&path, "old")?;
    let mut output = fs::writer(&path).await?;
    output.write(b"partial".to_vec()).await?;
    let (gated, started, release, finished) = ConversionGate::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            result = output.write(gated) => panic!("encoding finished before release: {result:?}"),
            result = started => result.unwrap(),
        }
    })
    .await?;
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), finished)
        .await?
        .unwrap();
    assert!(output.write(b"more".to_vec()).await.is_err());
    assert!(output.finish().await.is_err());
    assert_eq!(std::fs::read(&path)?, b"old");
    fs::write(&path, b"following".to_vec()).await?;
    assert_eq!(std::fs::read(&path)?, b"following");
    Ok(())
}

#[tokio::test]
async fn cancelled_decoding_preserves_the_file_and_never_calls_the_handler() -> io::Result<()> {
    static GATE: std::sync::Mutex<Option<ConversionGate>> = std::sync::Mutex::new(None);
    struct Document(Option<oneshot::Sender<()>>);
    impl Decode for Document {
        type Error = std::convert::Infallible;
        fn decode(_: Vec<u8>) -> Result<Self, Self::Error> {
            let gate = GATE.lock().unwrap().take().unwrap();
            Ok(Self(Some(gate.wait())))
        }
    }
    impl Encode for Document {
        type Error = std::convert::Infallible;
        fn encode(self) -> Result<Vec<u8>, Self::Error> {
            Ok(b"cancelled".to_vec())
        }
    }
    impl Drop for Document {
        fn drop(&mut self) {
            // Wait for the decoded value to be discarded before checking the handler.
            let _ = self.0.take().unwrap().send(());
        }
    }
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    std::fs::write(&path, "old")?;
    let (gated, started, release, finished) = ConversionGate::new();
    *GATE.lock().unwrap() = Some(gated);
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let handler_ran = ran.clone();
    let task_path = path.clone();
    let task = tokio::spawn(async move {
        fs::edit(task_path, |value: Document| async move {
            handler_ran.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, io::Error>(value)
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), started)
        .await?
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), finished)
        .await?
        .unwrap();
    let mut following = tokio::time::timeout(Duration::from_secs(5), fs::editor(&path)).await??;
    assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(std::fs::read(&path)?, b"old");
    following.write(b"following".to_vec()).await?;
    following.finish().await?;
    assert_eq!(std::fs::read(&path)?, b"following");
    Ok(())
}

#[test]
fn resource_handles_and_standard_operation_futures_are_send() {
    fn send<T: Send>() {}
    fn future_send(_: impl std::future::Future + Send) {}
    send::<fs::Reader>();
    send::<fs::Writer>();
    send::<fs::Editor>();
    send::<fs::DirEntries>();
    send::<fs::TempFile>();
    send::<fs::TempDir>();
    future_send(fs::write(Path::new("data"), "value".to_owned()));
    future_send(fs::edit(Path::new("data"), |text: String| async move {
        Ok::<_, io::Error>(text)
    }));
}

#[cfg(windows)]
#[tokio::test]
async fn windows_alternate_streams_survive_file_replacement() -> io::Result<()> {
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
    async fn links_are_rejected_for_writes_and_parent_aliases_share_a_lock() -> io::Result<()> {
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
        let alias = directory.path().join("alias");
        symlink(directory.path(), &alias)?;
        let output = fs::writer(&path).await?;
        assert_eq!(
            fs::write(alias.join("original"), "bad".to_owned())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        drop(output);
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
    async fn replacement_breaks_hardlinks_and_preserves_mode_owner_and_xattrs() -> io::Result<()> {
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
    async fn new_file_permissions_follow_the_normal_umask() -> io::Result<()> {
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
    async fn macos_case_and_unicode_aliases_share_locks_even_before_creation() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("Café");
        let alias = directory.path().join("CAFE\u{301}");
        let mut first = fs::writer(&path).await?;
        assert_eq!(
            fs::write(&alias, "conflict".to_owned())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        first.write("content".to_owned()).await?;
        first.finish().await?;
        assert_eq!(fs::read::<String>(&alias).await?, "content");
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_acl_entries_survive_replacement() -> io::Result<()> {
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
    async fn macos_writes_and_edits_preserve_creation_time_and_flags() -> io::Result<()> {
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
                    Ok::<_, io::Error>("new".to_owned())
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
    async fn macos_protected_files_fail_without_leaking_temporaries() -> io::Result<()> {
        use std::process::Command;
        for flag in ["uchg", "uappnd"] {
            let directory = tempfile::tempdir()?;
            let path = directory.path().join("protected");
            std::fs::write(&path, "old")?;
            let result = Command::new("chflags").arg(flag).arg(&path).output()?;
            assert!(result.status.success(), "{result:?}");
            let result = fs::write(&path, "new".to_owned()).await;
            let entries = std::fs::read_dir(directory.path())?.count();
            // Clear protection even if a regression left a protected temporary file.
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
