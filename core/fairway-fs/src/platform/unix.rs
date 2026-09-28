//! The Unix steps: copying metadata, and on macOS, resolving directory names
//! and computing lock keys.
//!
//! Metadata is copied between open descriptors rather than paths, so that it
//! cannot reach another file that has taken one of the paths in the meantime.

// Some of the descriptor-based calls have no safe wrapper in the standard
// library.
#![allow(unsafe_code)]

use std::{
    collections::HashSet,
    fs::File,
    io,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
};

use xattr::FileExt;

use super::{Cause, Result};

#[cfg(target_os = "macos")]
use std::{
    fs::FileTimes,
    os::macos::fs::{FileTimesExt, MetadataExt as _},
};

/// Implements [`super::canonical_parent`]: after resolving the symbolic links,
/// asks the kernel for the directory's path, which has the letter case stored
/// by the filesystem.
#[cfg(target_os = "macos")]
pub(super) fn canonical_parent(path: &std::path::Path) -> Result<std::path::PathBuf> {
    use std::{
        ffi::CStr,
        os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    };
    let path = std::fs::canonicalize(path)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_EVTONLY)
        .open(&path)?;
    let mut bytes = [0_u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes, the size of `bytes`, and
    // `file` keeps the descriptor open during the call.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, bytes.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error().into());
    }
    let name = CStr::from_bytes_until_nul(&bytes)
        .map_err(|source| Cause::External(io::ErrorKind::Other, Box::new(source)))?;
    Ok(std::path::PathBuf::from(std::ffi::OsStr::from_bytes(
        name.to_bytes(),
    )))
}

/// Implements [`super::lock_key`]: the name is converted to Unicode
/// normalization form D, and to upper case first if the directory's
/// filesystem ignores case. A name that is not valid UTF-8 is kept as it is.
#[cfg(target_os = "macos")]
pub(super) fn lock_key(path: &std::path::Path) -> Result<std::path::PathBuf> {
    use std::os::unix::fs::OpenOptionsExt;
    use unicode_normalization::UnicodeNormalization;
    let parent = path.parent().expect("normalized target");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_EVTONLY)
        .open(parent)?;
    // SAFETY: fpathconf takes no pointers, and `file` keeps the descriptor open
    // during the call.
    let sensitive = unsafe { libc::fpathconf(file.as_raw_fd(), libc::_PC_CASE_SENSITIVE) };
    if sensitive == -1 {
        return Err(io::Error::last_os_error().into());
    }
    let name = path.file_name().expect("normalized target");
    match name.to_str() {
        Some(name) => {
            let name: String = if sensitive == 0 {
                name.to_uppercase().nfd().collect()
            } else {
                name.nfd().collect()
            };
            Ok(parent.join(name))
        }
        None => Ok(path.to_owned()),
    }
}

/// Gives `target` the owner, group, permissions, and extended attributes of
/// `source`; on Linux, the attributes include the POSIX ACL. On macOS, also
/// copies the ACL, the creation time, and the file flags.
///
/// Afterwards it checks that the result matches, because some changes can
/// undo others or be ignored by the filesystem.
pub(crate) fn copy_metadata(source: &File, target: &File) -> Result<()> {
    let before = source.metadata()?;
    #[cfg(target_os = "macos")]
    if before.st_flags()
        & (libc::UF_IMMUTABLE | libc::UF_APPEND | libc::SF_IMMUTABLE | libc::SF_APPEND)
        != 0
    {
        // Such a file cannot be replaced anyway, and copying these flags would
        // make the temporary file impossible to delete after the rename fails.
        return Err(Cause::Message(
            io::ErrorKind::PermissionDenied,
            "cannot replace a file with immutable or append-only flags",
        ));
    }
    // Change the owner before the mode: changing the owner clears the
    // set-user-ID and set-group-ID bits, which `set_permissions` then restores.
    let current = target.metadata()?;
    if (before.uid(), before.gid()) != (current.uid(), current.gid()) {
        // SAFETY: fchown takes no pointers, and `target` keeps the descriptor
        // open during the call.
        if unsafe { libc::fchown(target.as_raw_fd(), before.uid(), before.gid()) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    target.set_permissions(before.permissions())?;
    copy_attributes(source, target)?;
    #[cfg(target_os = "macos")]
    {
        // COPYFILE_ACL copies only the ACL and leaves the contents and the
        // timestamps alone.
        // SAFETY: both descriptors stay open during the call, and a null state
        // makes fcopyfile use a temporary one.
        if unsafe {
            libc::fcopyfile(
                source.as_raw_fd(),
                target.as_raw_fd(),
                std::ptr::null_mut(),
                libc::COPYFILE_ACL,
            )
        } != 0
        {
            return Err(io::Error::last_os_error().into());
        }
        // Keep the creation time, but not the old modification time.
        target.set_times(FileTimes::new().set_created(before.created()?))?;
        // SAFETY: fchflags takes no pointers, and `target` keeps the descriptor
        // open during the call.
        if unsafe { libc::fchflags(target.as_raw_fd(), before.st_flags()) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    // On Linux, the POSIX ACL was copied with the extended attributes, as
    // system.posix_acl_access. Setting an ACL can change the permission bits,
    // so check the owner and the mode last.
    let after = target.metadata()?;
    if (before.uid(), before.gid(), before.mode() & 0o7777)
        != (after.uid(), after.gid(), after.mode() & 0o7777)
    {
        return Err(Cause::Message(
            io::ErrorKind::PermissionDenied,
            "could not preserve file ownership and permissions",
        ));
    }
    #[cfg(target_os = "macos")]
    if before.created()? != after.created()? || before.st_flags() != after.st_flags() {
        return Err(Cause::Message(
            io::ErrorKind::Other,
            "could not preserve file creation time and flags",
        ));
    }
    Ok(())
}

/// Returns the names of the extended attributes of `file`, or none if its
/// filesystem does not support them.
fn attributes(file: &File) -> Result<HashSet<std::ffi::OsString>> {
    match file.list_xattr() {
        Ok(names) => Ok(names.collect()),
        Err(error) if error.raw_os_error() == Some(libc::ENOTSUP) => Ok(HashSet::new()),
        Err(error) => Err(error.into()),
    }
}

/// Makes the extended attributes of `target` equal to those of `source`.
fn copy_attributes(source: &File, target: &File) -> Result<()> {
    let names = attributes(source)?;
    // The new file may have received attributes that the original does not
    // have, such as an ACL inherited from the default ACL of the directory.
    for name in attributes(target)?.difference(&names) {
        target.remove_xattr(name)?;
    }
    for name in names {
        let bytes = source.get_xattr(&name)?.ok_or_else(|| {
            Cause::Message(
                io::ErrorKind::Other,
                "a source attribute disappeared during copying",
            )
        })?;
        // Write only attributes that differ: setting some of them, such as
        // security labels, needs privileges that the process may lack.
        if target.get_xattr(&name)?.as_ref() != Some(&bytes) {
            target.set_xattr(&name, &bytes)?;
        }
    }
    Ok(())
}
