// All Windows handles and buffers stay owned for the duration of each call.
#![allow(unsafe_code)]

use std::{
    ffi::c_void,
    fs::File,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle},
    ptr,
};

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

pub(crate) fn copy_metadata(source: &File, target: &File) -> io::Result<()> {
    copy_streams(source, target)?;
    copy_security(source, target)?;
    target.set_permissions(source.metadata()?.permissions())
}

pub(super) fn lock_key(path: &std::path::Path) -> io::Result<std::path::PathBuf> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_CASE_SENSITIVE_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES,
        FileCaseSensitiveInfo, GetFileInformationByHandleEx,
    };
    let parent = path.parent().expect("normalized target");
    let directory = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(parent)?;
    let mut info = FILE_CASE_SENSITIVE_INFO::default();
    // SAFETY: live directory handle and an output buffer of the documented size.
    let success = unsafe {
        GetFileInformationByHandleEx(
            directory.as_raw_handle(),
            FileCaseSensitiveInfo,
            (&mut info as *mut FILE_CASE_SENSITIVE_INFO).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    };
    if success == 0 {
        let error = io::Error::last_os_error();
        // Older Windows/filesystems without this query use ordinary insensitive names.
        if !matches!(error.raw_os_error(), Some(1 | 50 | 87)) {
            return Err(error);
        }
    }
    if info.Flags & 1 != 0 {
        return Ok(path.to_owned());
    }
    let name = path.file_name().expect("normalized target");
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Globalization::{LCMAP_UPPERCASE, LCMapStringEx};
    // Windows filesystem casing must preserve UTF-16 (including unpaired
    // surrogates) and must not expand distinct names such as sharp s into SS.
    // Invariant, non-linguistic Windows casing provides those semantics.
    let source: Vec<u16> = name.encode_wide().collect();
    let length = i32::try_from(source.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "file name is too long"))?;
    let locale = [0_u16]; // LOCALE_NAME_INVARIANT
    // SAFETY: input and locale are live UTF-16 buffers with explicit lengths.
    let required = unsafe {
        LCMapStringEx(
            locale.as_ptr(),
            LCMAP_UPPERCASE,
            source.as_ptr(),
            length,
            ptr::null_mut(),
            0,
            ptr::null(),
            ptr::null(),
            0,
        )
    };
    if required == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut mapped = vec![0_u16; required as usize];
    // SAFETY: output has the capacity returned by the same mapping query.
    let written = unsafe {
        LCMapStringEx(
            locale.as_ptr(),
            LCMAP_UPPERCASE,
            source.as_ptr(),
            length,
            mapped.as_mut_ptr(),
            required,
            ptr::null(),
            ptr::null(),
            0,
        )
    };
    if written == 0 {
        return Err(io::Error::last_os_error());
    }
    mapped.truncate(written as usize);
    Ok(parent.join(std::ffi::OsString::from_wide(&mapped)))
}

struct Security {
    descriptor: PSECURITY_DESCRIPTOR,
    owner: PSID,
    group: PSID,
    dacl: *mut ACL,
}

impl Security {
    fn read(file: &File) -> io::Result<Self> {
        let mut value = Self {
            descriptor: ptr::null_mut(),
            owner: ptr::null_mut(),
            group: ptr::null_mut(),
            dacl: ptr::null_mut(),
        };
        // SAFETY: all out-pointers are initialized and valid; GetSecurityInfo
        // allocates the descriptor and returns interior pointers into it.
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
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        if value.owner.is_null() || value.group.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file security descriptor has no owner or group",
            ));
        }
        Ok(value)
    }
}

impl Drop for Security {
    fn drop(&mut self) {
        // SAFETY: descriptor is null or the allocation returned by GetSecurityInfo.
        unsafe {
            LocalFree(self.descriptor);
        }
    }
}

fn copy_security(source: &File, target: &File) -> io::Result<()> {
    let source = Security::read(source)?;
    let current = Security::read(target)?;
    let mut information = DACL_SECURITY_INFORMATION;
    let mut access = READ_CONTROL | WRITE_DAC;
    // Avoid requiring WRITE_OWNER when the newly created file already has the
    // correct owner and group. Request it explicitly when a change is needed.
    // SAFETY: these SID pointers are valid for the lifetime of their descriptors.
    if unsafe { EqualSid(source.owner, current.owner) } == 0 {
        information |= OWNER_SECURITY_INFORMATION;
        access |= WRITE_OWNER;
    }
    // SAFETY: as above, both group SIDs belong to live descriptors.
    if unsafe { EqualSid(source.group, current.group) } == 0 {
        information |= GROUP_SECURITY_INFORMATION;
        access |= WRITE_OWNER;
    }
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: descriptor and out-pointers are valid.
    if unsafe { GetSecurityDescriptorControl(source.descriptor, &mut control, &mut revision) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    information |= if control & SE_DACL_PROTECTED != 0 {
        PROTECTED_DACL_SECURITY_INFORMATION
    } else {
        UNPROTECTED_DACL_SECURITY_INFORMATION
    };
    let writable = reopen(target, access)?;
    // SAFETY: handle and security pointers remain live until the call returns.
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
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    Ok(())
}

fn reopen(file: &File, access: u32) -> io::Result<File> {
    // SAFETY: file is a synchronous live handle; success returns a new owned handle.
    let handle = unsafe {
        ReOpenFile(
            file.as_raw_handle(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: the successful ReOpenFile handle has a single Rust owner.
        Ok(unsafe { File::from_raw_handle(handle) })
    }
}

struct Backup<'a> {
    file: &'a File,
    context: *mut c_void,
    writing: bool,
}

impl Backup<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let mut count = 0;
        // SAFETY: synchronous handle, writable buffer, and persistent context pointer.
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
            return Err(io::Error::last_os_error());
        }
        Ok(count as usize)
    }

    fn read_exact(&mut self, mut bytes: &mut [u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let count = self.read(bytes)?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete metadata stream",
                ));
            }
            bytes = &mut bytes[count..];
        }
        Ok(())
    }

    fn write_all(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let mut count = 0;
            // SAFETY: synchronous handle, readable buffer, and persistent context pointer.
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
                return Err(io::Error::last_os_error());
            }
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "incomplete metadata write",
                ));
            }
            bytes = &bytes[count as usize..];
        }
        Ok(())
    }

    fn skip(&mut self, count: u64) -> io::Result<()> {
        let mut low = 0;
        let mut high = 0;
        // SAFETY: valid read context; skips the remaining payload of this stream.
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
            return Err(io::Error::last_os_error());
        }
        if (u64::from(high) << 32 | u64::from(low)) != count {
            return Err(io::Error::new(
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
        // SAFETY: bAbort frees the context and ignores the handle and buffer.
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

fn copy_streams(source: &File, target: &File) -> io::Result<()> {
    // Separate handles avoid changing the editor's read position. Primary file
    // data is skipped by BackupSeek rather than copied a second time.
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
        // WIN32_STREAM_ID's fixed wire header ends before cStreamName at byte 20,
        // independently of the struct's trailing alignment padding.
        let mut header = [0_u8; 20];
        let count = input.read(&mut header)?;
        if count == 0 {
            break;
        }
        input.read_exact(&mut header[count..])?;
        let id = u32::from_le_bytes(header[0..4].try_into().expect("stream id"));
        let mut remaining = u64::from_le_bytes(header[8..16].try_into().expect("stream size"));
        let name_len = u32::from_le_bytes(header[16..20].try_into().expect("name size")) as usize;
        if name_len > 64 * 1024 {
            return Err(io::Error::new(
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
