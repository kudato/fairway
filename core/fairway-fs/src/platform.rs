use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
};

pub(crate) fn canonical_parent(path: &Path) -> io::Result<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        unix::canonical_parent(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::canonicalize(path)
    }
}

pub(crate) fn lock_key(path: &Path) -> io::Result<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        unix::lock_key(path)
    }
    #[cfg(windows)]
    {
        windows::lock_key(path)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Ok(path.to_owned())
    }
}

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::copy_metadata;
#[cfg(windows)]
pub(crate) use windows::{canonical_target, copy_metadata};

pub(crate) fn check_target(path: &Path, required: bool) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the target must be a regular file, not a symlink, directory, or special file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound && !required => Ok(false),
        Err(error) => Err(error),
    }
}

pub(crate) fn open_original(path: &Path, required: bool) -> io::Result<Option<File>> {
    if !check_target(path, required)? {
        return Ok(None);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    no_follow(&mut options);
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the target is not a regular file",
        ));
    }
    Ok(Some(file))
}

pub(crate) fn lock_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    no_follow(&mut options);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid lock file",
        ));
    }
    Ok(file)
}

fn no_follow(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
}
