#![cfg_attr(not(windows), allow(missing_docs))]
#![cfg(windows)]
//! Windows path identity and filesystem API contracts.
use fairway_fs as fs;
use std::{
    ffi::OsString,
    io,
    os::windows::ffi::{OsStrExt, OsStringExt},
    time::Duration,
};

#[tokio::test]
async fn ascii_case_aliases_share_a_lock() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("MixedCase");
    fs::write(&path, "old").await?;
    let writer = fs::writer(&path).await?;
    let alias = dir.path().join("MIXEDCASE");
    assert!(matches!(fs::writer(&alias).await, Err(e) if e.kind() == io::ErrorKind::WouldBlock));
    drop(writer);
    tokio::time::timeout(Duration::from_secs(5), fs::editor(&alias))
        .await??
        .finish()
        .await?;
    Ok(())
}

#[tokio::test]
async fn non_unicode_case_aliases_share_a_lock() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(OsString::from_wide(&[0xd800, 0x61]));
    let alias = dir.path().join(OsString::from_wide(&[0xd800, 0x41]));
    std::fs::write(&path, "old")?;
    assert_eq!(std::fs::read(&alias)?, b"old", "NTFS recognizes the alias");
    let writer = fs::writer(&path).await?;
    let second = fs::writer(&alias).await;
    assert!(
        matches!(second, Err(e) if e.kind() == io::ErrorKind::WouldBlock),
        "case aliases must not acquire independent writers"
    );
    drop(writer);
    Ok(())
}

#[tokio::test]
async fn distinct_unicode_names_do_not_share_a_lock() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let first = dir.path().join("straße");
    let second = dir.path().join("STRASSE");
    std::fs::write(&first, "first")?;
    std::fs::write(&second, "second")?;
    assert_eq!(
        std::fs::read(&first)?,
        b"first",
        "NTFS keeps these names distinct"
    );
    let first_writer = fs::writer(&first).await?;
    let second_writer = fs::writer(&second).await?;
    drop(first_writer);
    drop(second_writer);
    Ok(())
}

#[tokio::test]
#[allow(
    unsafe_code,
    reason = "query a short alias of a file owned by this test"
)]
async fn short_file_aliases_share_a_lock_and_preserve_the_long_name() -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("long-document-filename.txt");
    std::fs::write(&path, "old")?;
    let input: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut buffer = vec![0_u16; 32768];
    // SAFETY: both buffers remain live, the input is terminated and output length is exact.
    let length =
        unsafe { GetShortPathNameW(input.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    assert!((length as usize) < buffer.len());
    let alias = std::path::PathBuf::from(OsString::from_wide(&buffer[..length as usize]));
    if alias.file_name() == path.file_name() {
        eprintln!("8.3 alias generation is disabled on the test volume");
        return Ok(());
    }
    let mut writer = fs::writer(&path).await?;
    assert!(
        matches!(fs::writer(&alias).await, Err(e) if e.kind() == io::ErrorKind::WouldBlock),
        "short and long names must share one lock"
    );
    writer.write("first").await?;
    writer.finish().await?;
    // Atomic replacement changes the file's generated short alias, so query it again.
    let length =
        unsafe { GetShortPathNameW(input.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
    assert!(length > 0 && (length as usize) < buffer.len());
    let alias = std::path::PathBuf::from(OsString::from_wide(&buffer[..length as usize]));
    fs::write(&alias, "second").await?;
    assert_eq!(
        std::fs::read(&path)?,
        b"second",
        "writing through 8.3 must preserve the long name"
    );
    Ok(())
}

#[tokio::test]
async fn editor_byte_cursor_flush_and_path_apis() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("данные с пробелами");
    fs::write(&path, "head猫\r\ntail").await?;
    let mut edit = fs::editor(&path).await?;
    let mut head = [0; 4];
    assert_eq!(edit.read(&mut head).await?, 4);
    assert_eq!(&head, b"head");
    assert_eq!(edit.next_line().await?, Some("猫".into()));
    edit.write("changed").await?;
    edit.flush().await?;
    assert_eq!(fs::read::<String>(&path).await?, "head猫\r\ntail");
    assert_eq!(edit.next_line().await?, Some("tail".into()));
    assert_eq!(edit.read(&mut []).await?, 0);
    edit.finish().await?;
    assert_eq!(fs::read::<String>(&path).await?, "changed");
    assert_eq!(
        fs::canonicalize(&path).await?,
        std::fs::canonicalize(&path)?
    );
    assert!(fs::read::<String>(dir.path()).await.is_err());
    Ok(())
}

#[tokio::test]
async fn long_paths_and_multiple_binary_streams_survive_edit() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut parent = dir.path().to_path_buf();
    for _ in 0..12 {
        parent.push("длинная папка с пробелами");
    }
    fs::mkdir(&parent).await?;
    let path = parent.join("document.txt");
    assert!(path.as_os_str().encode_wide().count() > 260);
    fs::write(&path, "old").await?;
    let large: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
    for (name, value) in [
        ("binary", large.as_slice()),
        ("empty", b"".as_slice()),
        ("text", b"metadata".as_slice()),
    ] {
        std::fs::write(path.with_file_name(format!("document.txt:{name}")), value)?;
    }
    fs::edit(
        &path,
        |s: String| async move { Ok::<_, io::Error>(s + "!") },
    )
    .await?;
    assert_eq!(fs::read::<String>(&path).await?, "old!");
    assert_eq!(
        std::fs::read(path.with_file_name("document.txt:binary"))?,
        large
    );
    assert!(std::fs::read(path.with_file_name("document.txt:empty"))?.is_empty());
    assert_eq!(
        std::fs::read(path.with_file_name("document.txt:text"))?,
        b"metadata"
    );
    Ok(())
}

#[tokio::test]
async fn failed_commit_releases_lock_and_removes_staging_file() -> io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("protected");
    std::fs::write(&path, "old")?;
    // Deny FILE_SHARE_DELETE: Windows must reject atomic replacement.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1 | 2)
        .open(&path)?;
    let mut writer = fs::writer(&path).await?;
    writer.write("new").await?;
    assert!(writer.finish().await.is_err());
    assert_eq!(std::fs::read(&path)?, b"old");
    assert_eq!(
        std::fs::read_dir(dir.path())?.count(),
        1,
        "failed transaction leaked staging file"
    );
    drop(held);
    fs::write(&path, "after").await?;
    assert_eq!(std::fs::read(&path)?, b"after");
    Ok(())
}

#[tokio::test]
async fn junction_parent_aliases_share_a_lock() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("real");
    fs::mkdir(&target).await?;
    let alias = dir.path().join("alias");
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&alias)
        .arg(&target)
        .output()?;
    assert!(output.status.success(), "{output:?}");
    let path = target.join("data");
    fs::write(&path, "old").await?;
    let writer = fs::writer(&path).await?;
    assert!(
        matches!(fs::writer(alias.join("data")).await, Err(e) if e.kind() == io::ErrorKind::WouldBlock)
    );
    drop(writer);
    tokio::time::timeout(Duration::from_secs(5), fs::editor(&path))
        .await??
        .finish()
        .await?;
    std::fs::remove_dir(&alias)?;
    Ok(())
}

#[tokio::test]
async fn readonly_target_failure_does_not_leak_readonly_staging_files() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("readonly");
    std::fs::write(&path, "old")?;
    let original_permissions = std::fs::metadata(&path)?.permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions)?;
    let result = fs::write(&path, "new").await;
    let count = std::fs::read_dir(dir.path())?.count();
    let contents = std::fs::read(&path)?;
    std::fs::set_permissions(&path, original_permissions)?;
    assert!(
        result.is_err(),
        "Windows denies replacement of a readonly target"
    );
    assert_eq!(contents, b"old");
    assert_eq!(count, 1, "failed commit leaked a readonly staging file");
    Ok(())
}

#[tokio::test]
async fn replacement_preserves_a_protected_dacl() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("security");
    std::fs::write(&path, "old")?;
    // USERDOMAIN under OpenSSH may name a workgroup, not the account authority.
    // icacls accepts a numeric SID prefixed with *, independently of locale.
    let identity = std::process::Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()?;
    assert!(identity.status.success(), "{identity:?}");
    let identity = String::from_utf8_lossy(&identity.stdout);
    let sid = identity
        .split('"')
        .find(|part| part.starts_with("S-1-"))
        .unwrap();
    let changed = std::process::Command::new("icacls")
        .arg(&path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("*{sid}:(F)"))
        .output()?;
    assert!(changed.status.success(), "{changed:?}");
    let before = std::process::Command::new("icacls").arg(&path).output()?;
    assert!(before.status.success());
    fs::write(&path, "new").await?;
    let after = std::process::Command::new("icacls").arg(&path).output()?;
    assert!(after.status.success());
    assert_eq!(
        before.stdout, after.stdout,
        "replacement changed the protected ACL"
    );
    Ok(())
}
