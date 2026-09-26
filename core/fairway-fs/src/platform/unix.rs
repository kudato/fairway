// Native metadata APIs require descriptors, not a second path lookup.
#![allow(unsafe_code)]

use std::{
    collections::HashSet,
    fs::File,
    io,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
};

use xattr::FileExt;

#[cfg(target_os = "macos")]
use std::{
    fs::FileTimes,
    os::macos::fs::{FileTimesExt, MetadataExt as _},
};

#[cfg(target_os = "macos")]
pub(super) fn canonical_parent(path: &std::path::Path) -> io::Result<std::path::PathBuf> {
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
    // SAFETY: F_GETPATH expects a writable PATH_MAX byte buffer and a live descriptor.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, bytes.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let name = CStr::from_bytes_until_nul(&bytes).map_err(io::Error::other)?;
    Ok(std::path::PathBuf::from(std::ffi::OsStr::from_bytes(
        name.to_bytes(),
    )))
}

#[cfg(target_os = "macos")]
pub(super) fn lock_key(path: &std::path::Path) -> io::Result<std::path::PathBuf> {
    use std::os::unix::fs::OpenOptionsExt;
    use unicode_normalization::UnicodeNormalization;
    let parent = path.parent().expect("normalized target");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_EVTONLY)
        .open(parent)?;
    // SAFETY: fpathconf only reads the capabilities of this live directory descriptor.
    let sensitive = unsafe { libc::fpathconf(file.as_raw_fd(), libc::_PC_CASE_SENSITIVE) };
    if sensitive == -1 {
        return Err(io::Error::last_os_error());
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

pub(crate) fn copy_metadata(source: &File, target: &File) -> io::Result<()> {
    let before = source.metadata()?;
    #[cfg(target_os = "macos")]
    if before.st_flags()
        & (libc::UF_IMMUTABLE | libc::UF_APPEND | libc::SF_IMMUTABLE | libc::SF_APPEND)
        != 0
    {
        // These files cannot be replaced. Copying their protection would also
        // prevent cleanup of the temporary file after the failed replacement.
        return Err(io::Error::from_raw_os_error(libc::EPERM));
    }
    let current = target.metadata()?;
    if (before.uid(), before.gid()) != (current.uid(), current.gid()) {
        // SAFETY: both descriptors are live and owned by this transaction.
        if unsafe { libc::fchown(target.as_raw_fd(), before.uid(), before.gid()) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    target.set_permissions(before.permissions())?;
    copy_attributes(source, target)?;
    #[cfg(target_os = "macos")]
    {
        // COPYFILE_ACL copies only the ACL, leaving file contents and timestamps alone.
        // SAFETY: live file descriptors; a null state requests copyfile's temporary state.
        if unsafe {
            libc::fcopyfile(
                source.as_raw_fd(),
                target.as_raw_fd(),
                std::ptr::null_mut(),
                libc::COPYFILE_ACL,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        // Preserve creation time without restoring the old modification time.
        target.set_times(FileTimes::new().set_created(before.created()?))?;
        // SAFETY: the descriptor is live; flags come from the original file.
        if unsafe { libc::fchflags(target.as_raw_fd(), before.st_flags()) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    // Linux POSIX ACLs are copied above as system.posix_acl_access. Verify mode
    // and ownership after ACL changes, which may also modify permission bits.
    let after = target.metadata()?;
    if (before.uid(), before.gid(), before.mode() & 0o7777)
        != (after.uid(), after.gid(), after.mode() & 0o7777)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "could not preserve file ownership and permissions",
        ));
    }
    #[cfg(target_os = "macos")]
    if before.created()? != after.created()? || before.st_flags() != after.st_flags() {
        return Err(io::Error::other(
            "could not preserve file creation time and flags",
        ));
    }
    Ok(())
}

fn attributes(file: &File) -> io::Result<HashSet<std::ffi::OsString>> {
    match file.list_xattr() {
        Ok(names) => Ok(names.collect()),
        Err(error) if error.raw_os_error() == Some(libc::ENOTSUP) => Ok(HashSet::new()),
        Err(error) => Err(error),
    }
}

fn copy_attributes(source: &File, target: &File) -> io::Result<()> {
    let names = attributes(source)?;
    for name in attributes(target)?.difference(&names) {
        target.remove_xattr(name)?;
    }
    for name in names {
        let bytes = source
            .get_xattr(&name)?
            .ok_or_else(|| io::Error::other("a source attribute disappeared during copying"))?;
        if target.get_xattr(&name)?.as_ref() != Some(&bytes) {
            target.set_xattr(&name, &bytes)?;
        }
    }
    Ok(())
}
