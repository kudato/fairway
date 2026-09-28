// On other platforms, `cfg(windows)` removes everything, including the crate
// documentation, which `missing_docs` would then report.
#![cfg_attr(not(windows), allow(missing_docs))]
#![cfg(windows)]

//! Windows-specific behavior: which spellings of a path share a lock, and how
//! replacement handles long paths, alternate data streams, sharing modes, the
//! read-only attribute, and DACLs.

use fairway_fs as fs;
use std::{
    ffi::OsString,
    io,
    os::windows::ffi::{OsStrExt, OsStringExt},
};

#[tokio::test]
async fn ascii_case_aliases_share_a_lock() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("MixedCase");
    fs::write(&path, "old".to_owned()).await?;
    let alias = dir.path().join("MIXEDCASE");
    fs::edit(&path, |text: String| async {
        assert!(matches!(fs::write(&alias, "conflict".to_owned()).await,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock));
        Ok::<_, fs::Error>(text)
    })
    .await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fs::edit(&alias, |text: String| async move {
            assert_eq!(text, "old");
            Ok::<_, fs::Error>("edited".to_owned())
        }),
    )
    .await??;
    assert_eq!(std::fs::read(&path)?, b"edited");
    Ok(())
}

#[tokio::test]
async fn non_unicode_case_aliases_share_a_lock() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    // The names start with an unpaired surrogate: they are not valid Unicode,
    // but NTFS accepts them and still ignores the case of the other letters.
    let path = dir.path().join(OsString::from_wide(&[0xd800, 0x61]));
    let alias = dir.path().join(OsString::from_wide(&[0xd800, 0x41]));
    std::fs::write(&path, "old")?;
    assert_eq!(std::fs::read(&alias)?, b"old", "NTFS recognizes the alias");
    fs::edit(&path, |text: String| async move {
        assert!(
            matches!(fs::write(&alias, "conflict".to_owned()).await,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock),
            "case aliases must not acquire independent locks"
        );
        Ok::<_, fs::Error>(text)
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn distinct_unicode_names_do_not_share_a_lock() -> anyhow::Result<()> {
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
    fs::edit(&first, |text: String| async move {
        fs::write(&second, "updated".to_owned()).await?;
        Ok::<_, fs::Error>(text)
    })
    .await?;
    Ok(())
}

#[tokio::test]
#[allow(
    unsafe_code,
    reason = "query a short alias of a file owned by this test"
)]
async fn short_file_aliases_share_a_lock_and_preserve_the_long_name() -> anyhow::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("long-document-filename.txt");
    std::fs::write(&path, "old")?;
    let input: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut buffer = vec![0_u16; 32768];
    // SAFETY: `input` ends with a null character, and `buffer` has room for
    // the number of characters passed with it.
    let length =
        unsafe { GetShortPathNameW(input.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 {
        return Err(io::Error::last_os_error().into());
    }
    assert!((length as usize) < buffer.len());
    let alias = std::path::PathBuf::from(OsString::from_wide(&buffer[..length as usize]));
    if alias.file_name() == path.file_name() {
        eprintln!("8.3 alias generation is disabled on the test volume");
        return Ok(());
    }
    fs::edit(&path, |_: String| async move {
        assert!(
            matches!(fs::write(&alias, "conflict".to_owned()).await,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock),
            "short and long names must share one lock"
        );
        Ok::<_, fs::Error>("first".to_owned())
    })
    .await?;
    // The replacement is a new file, whose short name may differ, so query it
    // again.
    // SAFETY: as above.
    let length =
        unsafe { GetShortPathNameW(input.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
    assert!(length > 0 && (length as usize) < buffer.len());
    let alias = std::path::PathBuf::from(OsString::from_wide(&buffer[..length as usize]));
    fs::write(&alias, "second".to_owned()).await?;
    assert_eq!(
        std::fs::read(&path)?,
        b"second",
        "writing through 8.3 must preserve the long name"
    );
    Ok(())
}

#[tokio::test]
async fn edit_text_and_path_apis() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    // A name with Cyrillic letters and spaces, and text with a CRLF line
    // ending, which must reach the handler unchanged.
    let path = dir.path().join("данные с пробелами");
    fs::write(&path, "head猫\r\ntail".to_owned()).await?;
    fs::edit(&path, |text: String| async move {
        assert_eq!(text, "head猫\r\ntail");
        Ok::<_, fs::Error>("changed".to_owned())
    })
    .await?;
    assert_eq!(fs::read::<String>(&path).await?, "changed");
    assert_eq!(
        fs::canonicalize(&path).await?,
        std::fs::canonicalize(&path)?
    );
    assert!(fs::read::<String>(dir.path()).await.is_err());
    Ok(())
}

#[tokio::test]
async fn long_paths_and_multiple_binary_streams_survive_edit() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut parent = dir.path().to_path_buf();
    for _ in 0..12 {
        parent.push("длинная папка с пробелами");
    }
    fs::mkdir(&parent).await?;
    let path = parent.join("document.txt");
    // Longer than MAX_PATH, the limit of the classic Win32 path functions.
    assert!(path.as_os_str().encode_wide().count() > 260);
    fs::write(&path, "old".to_owned()).await?;
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
        |s: String| async move { Ok::<_, fs::Error>(s + "!") },
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
async fn failed_commit_releases_lock_and_removes_staging_file() -> anyhow::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("protected");
    std::fs::write(&path, "old")?;
    // Share the file for reading and writing, but not for deletion: while this
    // handle is open, Windows refuses to rename another file over it.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1 | 2)
        .open(&path)?;
    assert!(fs::write(&path, "new".to_owned()).await.is_err());
    assert_eq!(std::fs::read(&path)?, b"old");
    assert_eq!(
        std::fs::read_dir(dir.path())?.count(),
        1,
        "failed transaction leaked staging file"
    );
    drop(held);
    fs::write(&path, "after".to_owned()).await?;
    assert_eq!(std::fs::read(&path)?, b"after");
    Ok(())
}

#[tokio::test]
async fn junction_parent_aliases_share_a_lock() -> anyhow::Result<()> {
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
    fs::write(&path, "old".to_owned()).await?;
    let alias_path = alias.join("data");
    fs::edit(&path, |text: String| async move {
        assert!(matches!(fs::write(alias_path, "conflict".to_owned()).await,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock));
        Ok::<_, fs::Error>(text)
    })
    .await?;
    std::fs::remove_dir(&alias)?;
    Ok(())
}

#[tokio::test]
async fn readonly_target_failure_does_not_leak_readonly_staging_files() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("readonly");
    std::fs::write(&path, "old")?;
    let original_permissions = std::fs::metadata(&path)?.permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions)?;
    let result = fs::write(&path, "new".to_owned()).await;
    let count = std::fs::read_dir(dir.path())?.count();
    let contents = std::fs::read(&path)?;
    // Restore the permissions before asserting: the temporary directory cannot
    // be removed while it contains a read-only file.
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
async fn replacement_preserves_a_protected_dacl() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("security");
    std::fs::write(&path, "old")?;
    // Grant access by SID rather than by account name: under OpenSSH,
    // USERDOMAIN may name a workgroup instead of the account's domain, and a
    // SID, which icacls takes prefixed with *, does not depend on the locale.
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
    fs::write(&path, "new".to_owned()).await?;
    let after = std::process::Command::new("icacls").arg(&path).output()?;
    assert!(after.status.success());
    assert_eq!(
        before.stdout, after.stdout,
        "replacement changed the protected ACL"
    );
    Ok(())
}
