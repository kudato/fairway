//! End-to-end filesystem behavior through the public API.

use std::{borrow::Cow, io, path::Path, sync::Arc, time::Duration};

use fairway_codec::{Decode, Encode, Json, Toml};
use fairway_fs as fs;
use tokio::sync::{Notify, oneshot};

#[tokio::test]
async fn atomic_replacement_keeps_old_readers_and_only_publishes_at_finish() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    fs::write(&path, "old\n").await?;
    let mut old = fs::reader(&path).await?;
    let mut output = fs::writer(&path).await?;
    output.write("new\n").await?;
    output.flush().await?;
    assert_eq!(fs::read::<String>(&path).await?, "old\n");
    output.write("last").await?;
    output.finish().await?;
    assert_eq!(fs::read::<String>(&path).await?, "new\nlast");
    assert_eq!(old.next_line().await?, Some("old".into()));
    assert_eq!(old.next_line().await?, None);
    let missing = directory.path().join("new");
    let mut output = fs::writer(&missing).await?;
    output.write(b"bytes").await?;
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
    fs::write(&path, "prefix猫\r\n\nend").await?;
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
    fs::write(&path, "old").await?;
    let mut output = fs::writer(&path).await?;
    assert_eq!(
        fs::write(&path, "conflict").await.unwrap_err().kind(),
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
    output.write("written").await?;
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
    fs::write(&path, "old").await?;
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
        fs::write(&path, "conflict").await.unwrap_err().kind(),
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
    fn encode(&self) -> Result<Cow<'_, [u8]>, Self::Error> {
        Err(EncodingFailure)
    }
}

#[tokio::test]
async fn conversion_failures_preserve_original_and_poison_streaming_writes() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    fs::write(&path, "old").await?;
    let error = fs::write(&path, Invalid).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(error.get_ref().unwrap().is::<EncodingFailure>());
    let mut output = fs::editor(&path).await?;
    output.write("first").await?;
    assert_eq!(
        output.write(Invalid).await.unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
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
    fs::write(&path, "first\nsecond\nthird").await?;
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
    editor.write("replacement").await?;
    assert_eq!(
        editor.next_line().await.unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(editor.finish().await.is_err());
    assert_eq!(fs::read::<Vec<u8>>(&path).await?, [0xff, b'\n']);
    Ok(())
}

#[tokio::test]
async fn directories_metadata_and_temporary_resources() -> io::Result<()> {
    let directory = fs::temp_dir().await?;
    fs::mkdir(directory.path().join("a/b")).await?;
    fs::mkdir(directory.path().join("a/b")).await?;
    fs::write(directory.path().join(".hidden"), "abc").await?;
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
    fs::write(&file, "replaced temporary inode").await?;
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
        fn decode(_: &[u8]) -> anyhow::Result<Self> {
            anyhow::bail!("custom decoder failed")
        }
    }
    assert!(
        matches!(fs::read::<Custom>(&path).await, Err(error) if error.kind() == io::ErrorKind::InvalidData && error.to_string() == "custom decoder failed")
    );
    Ok(())
}

struct EncodeGate {
    started: std::sync::Mutex<Option<oneshot::Sender<()>>>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl Encode for EncodeGate {
    type Error = std::convert::Infallible;
    fn encode(&self) -> Result<Cow<'_, [u8]>, Self::Error> {
        self.started
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Ok(Cow::Borrowed(b"new"))
    }
}

#[tokio::test]
async fn cancelled_encoding_keeps_the_file_locked_until_conversion_finishes() -> io::Result<()> {
    for whole_file in [false, true] {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("data");
        std::fs::write(&path, "old")?;
        let (started, receiver) = oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let gated = EncodeGate {
            started: std::sync::Mutex::new(Some(started)),
            release: std::sync::Mutex::new(wait),
        };
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
        receiver.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let busy = fs::writer(&path).await;
        release.send(()).unwrap();
        assert!(matches!(busy, Err(error) if error.kind() == io::ErrorKind::WouldBlock));
        let following = tokio::time::timeout(Duration::from_secs(5), fs::editor(&path)).await??;
        assert_eq!(std::fs::read_to_string(&path)?, "old");
        drop(following);
    }
    Ok(())
}

#[tokio::test]
async fn cancelling_edit_during_decode_retains_lock_and_never_calls_the_handler() -> io::Result<()>
{
    type Gate = (oneshot::Sender<()>, std::sync::mpsc::Receiver<()>);
    static GATE: std::sync::Mutex<Option<Gate>> = std::sync::Mutex::new(None);
    struct Document;
    impl Decode for Document {
        type Error = std::convert::Infallible;
        fn decode(_: &[u8]) -> Result<Self, Self::Error> {
            let (started, release) = GATE.lock().unwrap().take().unwrap();
            started.send(()).unwrap();
            release.recv().unwrap();
            Ok(Self)
        }
    }
    impl Encode for Document {
        type Error = std::convert::Infallible;
        fn encode(&self) -> Result<Cow<'_, [u8]>, Self::Error> {
            Ok(Cow::Borrowed(b"new"))
        }
    }
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    std::fs::write(&path, "old")?;
    let (started, receiver) = oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    *GATE.lock().unwrap() = Some((started, wait));
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
    receiver.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let busy = fs::writer(&path).await;
    release.send(()).unwrap();
    assert!(matches!(busy, Err(error) if error.kind() == io::ErrorKind::WouldBlock));
    let following = tokio::time::timeout(Duration::from_secs(5), fs::editor(&path)).await??;
    assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(std::fs::read_to_string(&path)?, "old");
    drop(following);
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
    future_send(fs::write(Path::new("data"), "value"));
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
    fs::write(&path, "new").await?;
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
        fs::write(&path, "old").await?;
        symlink(&path, &link)?;
        assert_eq!(fs::read::<String>(&link).await?, "old");
        assert_eq!(
            fs::write(&link, "bad").await.unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        let broken = directory.path().join("broken");
        symlink("absent", &broken)?;
        assert!(!fs::exists(&broken).await?);
        assert_eq!(
            fs::write(&broken, "bad").await.unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        let alias = directory.path().join("alias");
        symlink(directory.path(), &alias)?;
        let output = fs::writer(&path).await?;
        assert_eq!(
            fs::write(alias.join("original"), "bad")
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
        fs::write(&path, "new").await?;
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
        fs::write(&fairway, "").await?;
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
            fs::write(&alias, "conflict").await.unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        first.write("content").await?;
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
        fs::write(&path, "new").await?;
        assert_eq!(acl()?, before);
        Ok(())
    }
}
