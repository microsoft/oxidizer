// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::c_void;
use std::fs::File;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::fs::MetadataExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::path::Path;
use std::{io, ptr};

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
};
use windows_sys::Win32::Foundation::{
    GENERIC_ALL, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree, OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE,
    RtlNtStatusToDosError, UNICODE_STRING,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation, INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateDirectoryW, CreateFileW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    READ_CONTROL,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::monitor::MonitorOwner;

const ADMINISTRATORS: &str = "S-1-5-32-544";
const SYSTEM: &str = "S-1-5-18";

#[derive(Debug)]
struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: this pointer is owned and was returned by a LocalAlloc-family API.
        unsafe { LocalFree(self.0) };
    }
}

#[derive(Debug)]
pub(super) struct Attributes {
    descriptor: LocalAllocation,
}

impl Attributes {
    pub(super) fn for_owner(owner: &MonitorOwner) -> io::Result<Self> {
        let sid = owner_sid(owner)?;
        Self::from_sddl(&format!("O:{sid}D:P(A;;GA;;;{sid})(A;;GA;;;BA)(A;;GA;;;SY)"))
    }

    fn for_directory(owner: &MonitorOwner) -> io::Result<Self> {
        let sid = owner_sid(owner)?;
        Self::from_sddl(&format!("O:{sid}D:P(A;OICI;GA;;;{sid})(A;OICI;GA;;;BA)(A;OICI;GA;;;SY)"))
    }

    fn from_sddl(sddl: &str) -> io::Result<Self> {
        let text = wide(std::ffi::OsStr::new(sddl))?;
        let mut descriptor = ptr::null_mut();
        // SAFETY: text is terminated and the output pointer is writable.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(text.as_ptr(), SDDL_REVISION_1, &raw mut descriptor, ptr::null_mut())
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            descriptor: LocalAllocation(descriptor),
        })
    }

    pub(super) fn native(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).expect("fixed Windows SECURITY_ATTRIBUTES fits a DWORD"),
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: 0,
        }
    }
}

fn owner_sid(owner: &MonitorOwner) -> io::Result<&str> {
    let MonitorOwner::WindowsSid(sid) = owner else {
        return Err(denied("a Windows monitor requires a Windows user SID"));
    };
    if sid.parse::<MonitorOwner>().as_ref().is_ok_and(|parsed| parsed == owner) {
        Ok(sid)
    } else {
        Err(denied("invalid Windows monitor SID"))
    }
}

pub(super) fn wide(text: &std::ffi::OsStr) -> io::Result<Vec<u16>> {
    let mut wide: Vec<_> = text.encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "native name contains a null character"));
    }
    wide.push(0);
    Ok(wide)
}

pub(super) fn owned_handle(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: callers pass a fresh owning handle returned by a creation/open API.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn process_token(process: HANDLE) -> io::Result<OwnedHandle> {
    let mut token = ptr::null_mut();
    // SAFETY: process is valid, and token is a writable output pointer.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    owned_handle(token)
}

pub(crate) fn current_owner() -> io::Result<MonitorOwner> {
    // SAFETY: GetCurrentProcess returns a valid non-owning pseudo-handle.
    process_owner(unsafe { GetCurrentProcess() })
}

pub(super) fn process_owner(process: HANDLE) -> io::Result<MonitorOwner> {
    let token = process_token(process)?;
    let mut length = 0;
    // SAFETY: the null buffer with length zero requests the required size.
    unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, ptr::null_mut(), 0, &raw mut length) };
    if (length as usize) < std::mem::size_of::<TOKEN_USER>() {
        return Err(io::Error::last_os_error());
    }
    // TOKEN_USER contains pointers; use pointer-aligned storage rather than Vec<u8>.
    let mut storage = vec![0_usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
    // SAFETY: storage is aligned and large enough for the requested token information.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            storage.as_mut_ptr().cast(),
            length,
            &raw mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful GetTokenInformation initialized a TOKEN_USER in aligned storage.
    let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
    sid_string(user.User.Sid).map(MonitorOwner::WindowsSid)
}

fn sid_string(sid: PSID) -> io::Result<String> {
    let mut text = ptr::null_mut();
    // SAFETY: sid belongs to a live token/security descriptor, and text is writable.
    if unsafe { ConvertSidToStringSidW(sid, &raw mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _allocation = LocalAllocation(text.cast());
    let mut length = 0;
    // SAFETY: ConvertSidToStringSidW returned a terminated UTF-16 string.
    while unsafe { *text.wrapping_add(length) } != 0 {
        length += 1;
    }
    // SAFETY: length describes the readable converted SID string.
    String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
        .map_err(|_error| io::Error::new(io::ErrorKind::InvalidData, "OS SID is not valid UTF-16"))
}

fn directory_file(path: &Path) -> io::Result<File> {
    let text = wide(path.as_os_str())?;
    // No delete sharing prevents replacing the directory. Descriptor opens are
    // relative to this handle, not redirected through a mutable pathname.
    // SAFETY: text is terminated; the handle is non-inheritable and owned.
    let handle = unsafe {
        CreateFileW(
            text.as_ptr(),
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    let file = File::from(owned_handle(handle)?);
    let attributes = file.metadata()?.file_attributes();
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(denied("monitor directory is not an ordinary directory"));
    }
    Ok(file)
}

pub(crate) fn open_directory(path: &Path, owner: &MonitorOwner) -> io::Result<File> {
    let file = directory_file(path)?;
    verify_access(&file, owner)?;
    Ok(file)
}

fn verify_access(file: &File, expected: &MonitorOwner) -> io::Result<MonitorOwner> {
    let mut owner = ptr::null_mut();
    let mut acl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: file owns a valid handle; all output pointers are writable.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            &raw mut acl,
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status.cast_signed()));
    }
    let _allocation = LocalAllocation(descriptor);
    let actual = sid_string(owner)?;
    let expected_sid = owner_sid(expected)?;
    if actual != ADMINISTRATORS && actual != SYSTEM && expected_sid != actual {
        return Err(denied("monitor registry has an untrusted OS owner"));
    }
    // An inherited owner/admin/System ACL is safe too; validate its grants
    // rather than rewriting an existing user directory's inheritance policy.
    if acl.is_null() {
        return Err(denied("monitor registry must have an access-control list"));
    }
    let mut administrators = false;
    let mut system = false;
    let mut user = false;
    // SAFETY: GetSecurityInfo supplied a valid ACL within the live descriptor.
    for index in 0..unsafe { (*acl).AceCount } {
        let mut entry = ptr::null_mut();
        // SAFETY: index is bounded by AceCount, and entry is writable.
        if unsafe { GetAce(acl, u32::from(index), &raw mut entry) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: OS-generated ACL entries contain an ACE_HEADER.
        let header = unsafe { &*entry.cast::<windows_sys::Win32::Security::ACE_HEADER>() };
        // ACCESS_ALLOWED_ACE_TYPE is zero in the Windows ACL wire format.
        if header.AceType != 0 || u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
            return Err(denied("monitor registry ACL contains an unexpected entry"));
        }
        // SAFETY: an access-allowed ACE contains the mask followed by a SID.
        let entry = unsafe { &*entry.cast::<ACCESS_ALLOWED_ACE>() };
        let sid = sid_string(ptr::from_ref(&entry.SidStart).cast_mut().cast())?;
        let full = entry.Mask & GENERIC_ALL != 0 || entry.Mask & FILE_ALL_ACCESS == FILE_ALL_ACCESS;
        if expected_sid == sid {
            user |= full;
        }
        if sid == ADMINISTRATORS {
            administrators |= full;
        } else if sid == SYSTEM {
            system |= full;
        } else if expected_sid != sid {
            return Err(denied("monitor registry ACL grants access to another user"));
        }
    }
    if !administrators || !system || !user {
        return Err(denied("monitor registry ACL must grant its user and administrators full access"));
    }
    Ok(MonitorOwner::WindowsSid(actual))
}

fn make_directory(path: &Path, attributes: &Attributes) -> io::Result<()> {
    let text = wide(path.as_os_str())?;
    // SAFETY: text is terminated; the security descriptor outlives the call.
    if unsafe { CreateDirectoryW(text.as_ptr(), &attributes.native()) } == 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error);
        }
    }
    Ok(())
}

pub(crate) fn create_directory(path: &Path, owner: &MonitorOwner) -> io::Result<()> {
    make_directory(path, &Attributes::for_directory(owner)?)?;
    open_directory(path, owner).map(drop)
}

pub(super) fn is_elevated() -> io::Result<bool> {
    // SAFETY: current-process pseudo-handle is valid for token queries.
    let token = process_token(unsafe { GetCurrentProcess() })?;
    let mut elevation = TOKEN_ELEVATION::default();
    let mut length = 0;
    // SAFETY: elevation is a properly sized native output structure.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            ptr::from_mut(&mut elevation).cast(),
            u32::try_from(std::mem::size_of_val(&elevation)).expect("fixed Windows TOKEN_ELEVATION fits a DWORD"),
            &raw mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(elevation.TokenIsElevated != 0)
}

pub(crate) fn create_descriptor(path: &Path, owner: &MonitorOwner) -> io::Result<File> {
    let parent = path.parent().ok_or_else(|| denied("descriptor needs a directory"))?;
    let directory = open_directory(parent, owner)?;
    let attributes = Attributes::for_owner(owner)?;
    open_relative(&directory, path, GENERIC_WRITE, FILE_CREATE, Some(&attributes))
}

pub(crate) fn read_descriptor(path: &Path) -> io::Result<(File, MonitorOwner)> {
    let parent = path.parent().ok_or_else(|| denied("descriptor needs a directory"))?;
    let directory = directory_file(parent)?;
    let file = open_relative(&directory, path, GENERIC_READ | READ_CONTROL, FILE_OPEN, None)?;
    let attributes = file.metadata()?.file_attributes();
    if attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(denied("monitor descriptor is not an ordinary file"));
    }
    let owner = handle_owner(file.as_raw_handle())?;
    verify_access(&directory, &owner)?;
    verify_access(&file, &owner)?;
    Ok((file, owner))
}

fn open_relative(directory: &File, path: &Path, access: u32, disposition: u32, security: Option<&Attributes>) -> io::Result<File> {
    let name = path.file_name().ok_or_else(|| denied("descriptor needs a file name"))?;
    let mut name = wide(name)?;
    let length = u16::try_from((name.len() - 1) * 2)
        .map_err(|_error| io::Error::new(io::ErrorKind::InvalidInput, "descriptor file name is too long"))?;
    let name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(std::mem::size_of::<OBJECT_ATTRIBUTES>()).expect("fixed Windows OBJECT_ATTRIBUTES fits a DWORD"),
        RootDirectory: directory.as_raw_handle(),
        ObjectName: &raw const name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: security.map_or(ptr::null(), |security| security.descriptor.0.cast()),
        SecurityQualityOfService: ptr::null(),
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = ptr::null_mut();
    // SAFETY: directory is an opened ordinary directory; the relative name and
    // descriptor outlive the synchronous call. Reparse points are rejected.
    let result = unsafe {
        NtCreateFile(
            &raw mut handle,
            access | 0x0010_0000, // SYNCHRONIZE is required for synchronous native file I/O.
            &raw const attributes,
            &raw mut status,
            ptr::null(),
            0,
            FILE_SHARE_READ,
            disposition,
            FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
            ptr::null(),
            0,
        )
    };
    if result < 0 {
        // SAFETY: converting a returned NTSTATUS has no pointer preconditions.
        return Err(io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(result) }.cast_signed()));
    }
    owned_handle(handle).map(File::from)
}

pub(super) fn handle_owner(handle: HANDLE) -> io::Result<MonitorOwner> {
    let mut owner = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SAFETY: the caller retains a valid file/pipe handle; outputs are writable.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status.cast_signed()));
    }
    let _allocation = LocalAllocation(descriptor);
    sid_string(owner).map(MonitorOwner::WindowsSid)
}

fn denied(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "native and careful suites exercise Windows token access checks")]
    fn pipe_and_directory_reject_a_token_without_the_allowed_identities() {
        use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
        use windows_sys::Win32::Security::{
            CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, ImpersonateLoggedOnUser, RevertToSelf, SID_AND_ATTRIBUTES, TOKEN_DUPLICATE,
        };

        struct Revert;
        impl Drop for Revert {
            fn drop(&mut self) {
                // SAFETY: restores only this test thread's original security context.
                assert_ne!(unsafe { RevertToSelf() }, 0);
            }
        }

        let directory = crate::testing::Directory::new();
        let descriptor = directory.descriptor();
        let _listener = crate::transport::Listener::bind(&crate::io::MonitorIo::new_native(), &descriptor).unwrap();
        let user = current_owner().unwrap().to_string();
        let sids = [user.as_str(), ADMINISTRATORS, SYSTEM].map(|sid| {
            let text = wide(std::ffi::OsStr::new(sid)).unwrap();
            let mut sid = ptr::null_mut();
            // SAFETY: text is terminated and sid is a writable output pointer.
            assert_ne!(unsafe { ConvertStringSidToSidW(text.as_ptr(), &raw mut sid) }, 0);
            LocalAllocation(sid)
        });
        let disabled = sids.each_ref().map(|sid| SID_AND_ATTRIBUTES { Sid: sid.0, Attributes: 0 });
        // SAFETY: GetCurrentProcess returns a non-owning pseudo-handle.
        let process = unsafe { GetCurrentProcess() };
        let mut token = ptr::null_mut();
        // SAFETY: process and the token output pointer are valid.
        let opened = unsafe { OpenProcessToken(process, TOKEN_QUERY | TOKEN_DUPLICATE, &raw mut token) };
        assert_ne!(opened, 0);
        let token = owned_handle(token).unwrap();
        let mut restricted = ptr::null_mut();
        // SAFETY: all disabled SID allocations outlive token creation; remaining
        // arrays have zero lengths and null pointers.
        let created = unsafe {
            CreateRestrictedToken(
                token.as_raw_handle(),
                DISABLE_MAX_PRIVILEGE,
                u32::try_from(disabled.len()).unwrap(),
                disabled.as_ptr(),
                0,
                ptr::null(),
                0,
                ptr::null(),
                &raw mut restricted,
            )
        };
        assert_ne!(created, 0);
        let restricted = owned_handle(restricted).unwrap();
        // SAFETY: this restricted version of our own token is valid for impersonation.
        assert_ne!(unsafe { ImpersonateLoggedOnUser(restricted.as_raw_handle()) }, 0);
        let _revert = Revert;
        assert_eq!(
            crate::transport::Connection::connect(&crate::io::MonitorIo::new_native(), &descriptor, std::time::Duration::from_secs(1))
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            open_directory(directory.path(), &descriptor.owner).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "native and careful suites verify Windows registry ACLs")]
    fn unsafe_registry_permissions_and_owner_types_are_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("monitor");
        let owner = current_owner().unwrap();
        let sid = owner_sid(&owner).unwrap();
        let attributes = Attributes::from_sddl(&format!(
            "O:{sid}D:P(A;OICI;GA;;;{sid})(A;OICI;GA;;;BA)(A;OICI;GA;;;SY)(A;OICI;GR;;;WD)"
        ))
        .unwrap();
        make_directory(&path, &attributes).unwrap();
        assert_eq!(open_directory(&path, &owner).unwrap_err().kind(), io::ErrorKind::PermissionDenied);

        let ordinary = temporary.path().join("ordinary");
        // No symlink privilege is required to create an ordinary child.
        create_directory(&ordinary, &owner).unwrap();
        assert_eq!(
            open_directory(&ordinary, &MonitorOwner::UnixUser(1001)).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "native Windows ACL inheritance")]
    fn safe_inherited_user_permissions_do_not_require_reprovisioning() {
        let temporary = tempfile::tempdir().unwrap();
        let owner = current_owner().unwrap();
        let parent = temporary.path().join("application");
        create_directory(&parent, &owner).unwrap();
        let child = parent.join("monitor");
        std::fs::create_dir(&child).unwrap();
        open_directory(&child, &owner).unwrap();
    }
}
