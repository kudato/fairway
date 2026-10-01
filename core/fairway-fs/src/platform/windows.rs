//! The Windows steps for copying filesystem metadata and file attributes.
//!
//! Metadata is copied between open handles rather than paths, so that it
//! cannot reach another file that has taken one of the paths in the meantime.

// The Win32 functions called here have no safe wrappers in the standard
// library.
#![allow(unsafe_code)]

use std::{
    ffi::c_void,
    fs::File,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle},
    ptr,
};

use super::{Cause, Result};

use windows_sys::Win32::{
    Foundation::{ERROR_SUCCESS, HANDLE, INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        ACL,
        Authorization::{GetSecurityInfo, SE_FILE_OBJECT, SetSecurityInfo},
        DACL_SECURITY_INFORMATION, EqualSid, GROUP_SECURITY_INFORMATION,
        GetSecurityDescriptorControl, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    },
    Storage::FileSystem::{
        BACKUP_ALTERNATE_DATA, BACKUP_EA_DATA, BACKUP_PROPERTY_DATA, BackupRead, BackupSeek,
        BackupWrite, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, READ_CONTROL, ReOpenFile, WRITE_DAC, WRITE_OWNER,
    },
};

/// Gives `target` the extended attributes, alternate data streams, and
/// property data of `source`, then its owner, group, and DACL, and last its
/// file attributes, such as hidden and read-only.
pub(crate) fn copy_metadata(source: &File, target: &File) -> Result<()> {
    copy_streams(source, target)?;
    copy_security(source, target)?;
    // On Windows, `Permissions` holds all the file attributes, not only the
    // read-only one.
    target
        .set_permissions(source.metadata()?.permissions())
        .map_err(Cause::from)
}

/// Removes the staging file's hidden attribute before publishing a new file.
pub(crate) fn finish_new_file(file: &File) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL, FILE_BASIC_INFO, FileBasicInfo,
        SetFileInformationByHandle,
    };
    let attributes = file.metadata()?.file_attributes() & !FILE_ATTRIBUTE_HIDDEN;
    let mut info = FILE_BASIC_INFO {
        FileAttributes: if attributes == 0 {
            FILE_ATTRIBUTE_NORMAL
        } else {
            attributes
        },
        ..Default::default()
    };
    // SAFETY: the file owns the handle, and `info` has the size passed here.
    let success = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileBasicInfo,
            (&mut info as *mut FILE_BASIC_INFO).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

/// The owner, group, and DACL of a file.
///
/// They point into the security descriptor that GetSecurityInfo allocated,
/// which is freed on drop.
struct Security {
    descriptor: PSECURITY_DESCRIPTOR,
    owner: PSID,
    group: PSID,
    dacl: *mut ACL,
}

impl Security {
    /// Reads the owner, group, and DACL of `file`.
    fn read(file: &File) -> Result<Self> {
        let mut value = Self {
            descriptor: ptr::null_mut(),
            owner: ptr::null_mut(),
            group: ptr::null_mut(),
            dacl: ptr::null_mut(),
        };
        // SAFETY: `file` keeps the handle open during the call, and the output
        // arguments point to the fields of `value`. Dropping `value` frees the
        // descriptor that the call allocates.
        let error = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut value.owner,
                &mut value.group,
                &mut value.dacl,
                ptr::null_mut(),
                &mut value.descriptor,
            )
        };
        if error != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(error as i32).into());
        }
        if value.owner.is_null() || value.group.is_null() {
            return Err(Cause::Message(
                io::ErrorKind::InvalidData,
                "file security descriptor has no owner or group",
            ));
        }
        Ok(value)
    }
}

impl Drop for Security {
    fn drop(&mut self) {
        // SAFETY: `descriptor` is either null, which LocalFree ignores, or the
        // allocation returned by GetSecurityInfo, which is freed only here.
        unsafe {
            LocalFree(self.descriptor);
        }
    }
}

/// Gives `target` the owner, group, and DACL of `source`, including whether
/// the DACL inherits entries from the parent directory.
fn copy_security(source: &File, target: &File) -> Result<()> {
    let source = Security::read(source)?;
    let current = Security::read(target)?;
    let mut information = DACL_SECURITY_INFORMATION;
    let mut access = READ_CONTROL | WRITE_DAC;
    // Changing the owner or the group needs WRITE_OWNER access, which the
    // process may not have, so they are set only if they differ.
    // SAFETY: the SIDs point into the descriptors of `source` and `current`,
    // which are alive.
    if unsafe { EqualSid(source.owner, current.owner) } == 0 {
        information |= OWNER_SECURITY_INFORMATION;
        access |= WRITE_OWNER;
    }
    // SAFETY: as above.
    if unsafe { EqualSid(source.group, current.group) } == 0 {
        information |= GROUP_SECURITY_INFORMATION;
        access |= WRITE_OWNER;
    }
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: `source.descriptor` is a valid descriptor, and the output
    // arguments point to local variables.
    if unsafe { GetSecurityDescriptorControl(source.descriptor, &mut control, &mut revision) } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    // Setting a DACL also sets whether it inherits entries from the parent
    // directory, so pass on the original's choice.
    information |= if control & SE_DACL_PROTECTED != 0 {
        PROTECTED_DACL_SECURITY_INFORMATION
    } else {
        UNPROTECTED_DACL_SECURITY_INFORMATION
    };
    let writable = reopen(target, access)?;
    // SAFETY: `writable` keeps the handle open during the call, and the SIDs
    // and the DACL point into the descriptor of `source`, which is alive.
    let error = unsafe {
        SetSecurityInfo(
            writable.as_raw_handle(),
            SE_FILE_OBJECT,
            information,
            source.owner,
            source.group,
            source.dacl,
            ptr::null(),
        )
    };
    if error != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(error as i32).into());
    }
    Ok(())
}

/// Opens a second handle to `file` with the `access` rights.
///
/// Unlike a duplicate, the new handle has its own file position and can have
/// rights that the handle of `file` lacks.
fn reopen(file: &File, access: u32) -> Result<File> {
    // SAFETY: `file` keeps its handle open during the call.
    let handle = unsafe {
        ReOpenFile(
            file.as_raw_handle(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error().into())
    } else {
        // SAFETY: the handle is new and open, nothing else owns it, and it is
        // synchronous, as `File` requires, because no flags were passed.
        Ok(unsafe { File::from_raw_handle(handle) })
    }
}

/// A session of BackupRead or BackupWrite calls on a file, which read or write
/// its data as a sequence of streams, each with a header.
struct Backup<'a> {
    file: &'a File,
    /// The state that Windows keeps between the calls of the session; null
    /// before the first call.
    context: *mut c_void,
    /// Whether the session writes; it decides which function frees `context`.
    writing: bool,
}

impl Backup<'_> {
    /// Reads the next bytes into `bytes` and returns their number, which can
    /// be less than requested; 0 means that the file has no more streams.
    fn read(&mut self, bytes: &mut [u8]) -> Result<usize> {
        let mut count = 0;
        // SAFETY: `file` keeps the handle open, `bytes` has the length passed
        // with it, and `context` belongs to this session.
        if unsafe {
            BackupRead(
                self.file.as_raw_handle(),
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                &mut count,
                0,
                0,
                &mut self.context,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(count as usize)
    }

    /// Fills `bytes`; running out of data first is an error.
    fn read_exact(&mut self, mut bytes: &mut [u8]) -> Result<()> {
        while !bytes.is_empty() {
            let count = self.read(bytes)?;
            if count == 0 {
                return Err(Cause::Message(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete metadata stream",
                ));
            }
            bytes = &mut bytes[count..];
        }
        Ok(())
    }

    /// Writes all of `bytes`.
    fn write_all(&mut self, mut bytes: &[u8]) -> Result<()> {
        while !bytes.is_empty() {
            let mut count = 0;
            // SAFETY: `file` keeps the handle open, `bytes` has the length
            // passed with it, and `context` belongs to this session.
            if unsafe {
                BackupWrite(
                    self.file.as_raw_handle(),
                    bytes.as_ptr(),
                    bytes.len() as u32,
                    &mut count,
                    0,
                    0,
                    &mut self.context,
                )
            } == 0
            {
                return Err(io::Error::last_os_error().into());
            }
            if count == 0 {
                return Err(Cause::Message(
                    io::ErrorKind::WriteZero,
                    "incomplete metadata write",
                ));
            }
            bytes = &bytes[count as usize..];
        }
        Ok(())
    }

    /// Skips the next `count` bytes of the current stream without reading
    /// them; the stream must have that many left.
    fn skip(&mut self, count: u64) -> Result<()> {
        let mut low = 0;
        let mut high = 0;
        // SAFETY: `file` keeps the handle open, the output arguments point to
        // local variables, and `context` belongs to this reading session.
        if unsafe {
            BackupSeek(
                self.file.as_raw_handle(),
                count as u32,
                (count >> 32) as u32,
                &mut low,
                &mut high,
                &mut self.context,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if (u64::from(high) << 32 | u64::from(low)) != count {
            return Err(Cause::Message(
                io::ErrorKind::UnexpectedEof,
                "incomplete metadata stream",
            ));
        }
        Ok(())
    }
}

impl Drop for Backup<'_> {
    fn drop(&mut self) {
        let mut count = 0;
        // SAFETY: with bAbort set, the call only frees the context of this
        // session and ignores the handle and the buffer.
        unsafe {
            if self.writing {
                BackupWrite(
                    ptr::null_mut::<c_void>() as HANDLE,
                    ptr::null(),
                    0,
                    &mut count,
                    1,
                    0,
                    &mut self.context,
                );
            } else {
                BackupRead(
                    ptr::null_mut::<c_void>() as HANDLE,
                    ptr::null_mut(),
                    0,
                    &mut count,
                    1,
                    0,
                    &mut self.context,
                );
            }
        }
    }
}

/// Copies the extended attributes, alternate data streams, and property data
/// of `source` to `target`.
///
/// The other streams, such as the old contents and the object ID, are skipped
/// without being read. The security descriptor is not among the streams read;
/// [`copy_security`] copies it.
fn copy_streams(source: &File, target: &File) -> Result<()> {
    // Work through handles of their own, so that the file positions of
    // `source` and `target` do not move.
    let source = reopen(source, FILE_GENERIC_READ)?;
    let target = reopen(target, FILE_GENERIC_READ | FILE_GENERIC_WRITE)?;
    let mut input = Backup {
        file: &source,
        context: ptr::null_mut(),
        writing: false,
    };
    let mut output = Backup {
        file: &target,
        context: ptr::null_mut(),
        writing: true,
    };
    let mut buffer = vec![0; 64 * 1024];
    loop {
        // Each stream starts with a WIN32_STREAM_ID header: 20 bytes followed
        // by the stream name. The header is parsed from bytes, because the
        // struct is padded to 24 bytes.
        let mut header = [0_u8; 20];
        let count = input.read(&mut header)?;
        if count == 0 {
            break;
        }
        input.read_exact(&mut header[count..])?;
        let id = u32::from_le_bytes(header[0..4].try_into().expect("stream id"));
        let mut remaining = u64::from_le_bytes(header[8..16].try_into().expect("stream size"));
        let name_len = u32::from_le_bytes(header[16..20].try_into().expect("name size")) as usize;
        // Stream names take at most a few hundred bytes; a larger size means
        // corrupt data and must not cause a huge allocation.
        if name_len > 64 * 1024 {
            return Err(Cause::Message(
                io::ErrorKind::InvalidData,
                "metadata stream name is too long",
            ));
        }
        let mut name = vec![0; name_len];
        input.read_exact(&mut name)?;
        if !matches!(
            id,
            BACKUP_EA_DATA | BACKUP_ALTERNATE_DATA | BACKUP_PROPERTY_DATA
        ) {
            if remaining != 0 {
                input.skip(remaining)?;
            }
            continue;
        }
        output.write_all(&header)?;
        output.write_all(&name)?;
        while remaining != 0 {
            let length = remaining.min(buffer.len() as u64) as usize;
            input.read_exact(&mut buffer[..length])?;
            output.write_all(&buffer[..length])?;
            remaining -= length as u64;
        }
    }
    Ok(())
}
