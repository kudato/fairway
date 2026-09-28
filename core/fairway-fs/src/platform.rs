//! Filesystem steps that differ between platforms; the Unix and Windows
//! details are in the submodules.

use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
};

/// Returns the canonical form of the directory path `path`, with all symbolic
/// links resolved.
///
/// On macOS, the result also has the letter case that the filesystem stores,
/// so that different spellings of one directory give the same path on a
/// filesystem that ignores case.
pub(crate) fn canonical_parent(path: &Path) -> Result<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        unix::canonical_parent(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::canonicalize(path).map_err(Cause::from)
    }
}

/// Returns the key under which `path`, a target returned by
/// [`lock::target`](crate::lock::target), is locked.
///
/// On macOS and Windows, the file name is converted to upper case if the
/// filesystem ignores case, and on macOS it is also brought to one Unicode
/// normalization form, so that all spellings of the name share a lock.
/// Elsewhere the key is the path itself.
pub(crate) fn lock_key(path: &Path) -> Result<std::path::PathBuf> {
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

use crate::{Error, Operation, error::Cause};

/// The result of a step whose error still lacks the operation and the path;
/// the caller adds them.
type Result<T> = std::result::Result<T, Cause>;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::copy_metadata;
#[cfg(windows)]
pub(crate) use windows::{canonical_target, copy_metadata};

/// Checks that `path` is a regular file and not a symbolic link, and returns
/// whether it exists; a missing file is an error only if it is `required`.
pub(crate) fn check_target(path: &Path, required: bool) -> std::result::Result<bool, Error> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(Error::message(
            Operation::ValidateTarget,
            Some(path),
            io::ErrorKind::InvalidInput,
            "the target must be a regular file, not a symlink, directory, or special file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound && !required => Ok(false),
        Err(source) => Err(Error::io(Operation::ValidateTarget, Some(path), source)),
    }
}

/// Opens the file at `path` that a replacement will replace, or returns
/// `None` if it does not exist and is not `required`.
pub(crate) fn open_original(
    path: &Path,
    required: bool,
) -> std::result::Result<Option<File>, Error> {
    if !check_target(path, required)? {
        return Ok(None);
    }
    let mut options = OpenOptions::new();
    // The file is replaced, not written, but a file that the process may not
    // write to must not be replaced either. Opening it for writing checks this
    // before anything is encoded or a handler is called.
    options.read(true).write(true);
    no_follow(&mut options);
    let file = options
        .open(path)
        .map_err(|source| Error::io(Operation::Open, Some(path), source))?;
    // Check the opened file itself: the path may have changed since the check
    // above.
    if !file
        .metadata()
        .map_err(|source| Error::io(Operation::Metadata, Some(path), source))?
        .is_file()
    {
        return Err(Error::message(
            Operation::ValidateTarget,
            Some(path),
            io::ErrorKind::InvalidInput,
            "the target is not a regular file",
        ));
    }
    Ok(Some(file))
}

/// Opens the lock file at `path`, creating it if needed; a symbolic link or
/// anything else that is not a regular file is rejected.
pub(crate) fn lock_file(path: &Path) -> Result<File> {
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
        return Err(Cause::Message(
            io::ErrorKind::InvalidInput,
            "invalid lock file",
        ));
    }
    Ok(file)
}

/// Keeps `options` from following a symbolic link at the end of the path.
///
/// On Unix, opening a link then fails; on Windows, the link itself is opened,
/// and the callers reject it because it is not a regular file. On Unix,
/// opening a FIFO also does not block waiting for the other end.
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
