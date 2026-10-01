//! Filesystem steps that differ between platforms; the Unix and Windows
//! details are in the submodules.

use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
};

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
pub(crate) use windows::{copy_metadata, finish_new_file};

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

/// Creates the empty adjacent reservation, also used as the output file.
pub(crate) fn create_lock(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o666);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.attributes(windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN);
    }
    options.open(path)
}

/// Makes an existing file's replacement private before any data is written.
/// A new file keeps the creation mode, including the process umask and the
/// parent directory's inherited permissions.
pub(crate) fn prepare_output(file: &File, replacing: bool) -> io::Result<()> {
    #[cfg(unix)]
    if replacing {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = (file, replacing);
    Ok(())
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
