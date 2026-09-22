#![cfg(windows)]
//! Windows path identity and filesystem API contracts.
use fairway_fs as fs;
use std::{ffi::OsString, io, os::windows::ffi::OsStringExt, time::Duration};

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
