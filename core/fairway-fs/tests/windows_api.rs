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
