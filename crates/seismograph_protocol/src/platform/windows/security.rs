// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::windows::fs::MetadataExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::{io, ptr};

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
};
use windows_sys::Win32::Foundation::{
    ERROR_NO_MORE_FILES, GENERIC_ALL, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree, OBJ_CASE_INSENSITIVE,
    OBJ_DONT_REPARSE, RtlNtStatusToDosError, UNICODE_STRING,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation, INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FULL_DIR_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FileFullDirectoryInfo, FileFullDirectoryRestartInfo, GetFileInformationByHandleEx, QueryDosDeviceW,
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
    #[cfg_attr(test, mutants::skip)] // LocalFree releases an OS allocation; its absence is only observable as a native memory leak.
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

#[cfg_attr(coverage_nightly, coverage(off))] // Token-query failures require inaccessible processes or invalidating an owned handle.
#[cfg_attr(test, mutants::skip)] // Native restricted-token tests retain the OS token boundary.
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

#[cfg_attr(coverage_nightly, coverage(off))] // Token buffer/query failures cannot be injected for a valid process token.
#[cfg_attr(test, mutants::skip)] // Native identity tests retain actual token reads; SID/policy validation stays measured.
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

#[cfg_attr(coverage_nightly, coverage(off))] // OS-owned SIDs cannot safely be corrupted to force conversion failures.
#[cfg_attr(test, mutants::skip)] // Native ACL and peer-identity tests retain SID conversion.
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
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Err(denied("monitor registry requires an absolute local disk path"));
    };
    let (Prefix::Disk(drive) | Prefix::VerbatimDisk(drive)) = prefix.kind() else {
        return Err(denied("monitor registry requires an absolute local disk path"));
    };
    if components.next() != Some(Component::RootDir) {
        return Err(denied("monitor registry requires an absolute local disk path"));
    }
    let root = drive_volume(drive)?;
    let access = FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL;
    let mut file = open_native(None, root.as_os_str(), access, FILE_OPEN, FILE_DIRECTORY_FILE, None)?;
    ordinary_directory(&file)?;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(denied("monitor registry path contains a traversal component"));
        };
        file = open_native(Some(&file), name, access, FILE_OPEN, FILE_DIRECTORY_FILE, None)?;
        ordinary_directory(&file)?;
    }
    Ok(file)
}

#[cfg_attr(coverage_nightly, coverage(off))] // DOS-device query failures and malformed kernel mappings require changing machine drive configuration.
#[cfg_attr(test, mutants::skip)] // Native traversal tests retain this call; local-volume acceptance stays measured separately.
fn drive_volume(drive: u8) -> io::Result<PathBuf> {
    let device = [u16::from(drive), u16::from(b':'), 0];
    // DOS device queries inspect the local object namespace, not the target filesystem.
    let mut target = vec![0_u16; 32_768];
    // SAFETY: device is terminated and target is a writable, bounded UTF-16 buffer.
    let length = unsafe {
        QueryDosDeviceW(
            device.as_ptr(),
            target.as_mut_ptr(),
            u32::try_from(target.len()).expect("bounded DOS device buffer fits a DWORD"),
        )
    };
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    let end = target[..length as usize]
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(|| denied("DOS device mapping is unterminated"))?;
    let target = String::from_utf16(&target[..end]).map_err(|source| io::Error::new(io::ErrorKind::InvalidData, source))?;
    local_volume_path(&target)
}

fn local_volume_path(target: &str) -> io::Result<PathBuf> {
    // Resolve drive letters once, then bypass mutable DOS aliases entirely.
    // Network redirectors, substituted paths, and device namespaces are not registries.
    let number = target
        .strip_prefix(r"\Device\HarddiskVolume")
        .filter(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| denied("monitor registry requires a local disk volume"))?;
    Ok(PathBuf::from(format!(r"\Device\HarddiskVolume{number}\")))
}

fn ordinary_directory(file: &File) -> io::Result<()> {
    let attributes = file.metadata()?.file_attributes();
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(denied("monitor directory is not an ordinary directory"));
    }
    Ok(())
}

pub(crate) fn directory_entries(path: &Path) -> io::Result<Vec<io::Result<PathBuf>>> {
    read_directory(&directory_file(path)?, path)
}

fn read_directory(directory: &File, path: &Path) -> io::Result<Vec<io::Result<PathBuf>>> {
    // Enumerate through the local handle; reopening the pathname would reintroduce
    // ancestor-replacement races after directory validation.
    let mut storage = [0_u64; 1_024];
    let mut class = FileFullDirectoryRestartInfo;
    let mut entries = Vec::new();
    loop {
        // SAFETY: directory is live; storage is aligned and writable for the supplied size.
        let result = unsafe {
            GetFileInformationByHandleEx(
                directory.as_raw_handle(),
                class,
                storage.as_mut_ptr().cast(),
                u32::try_from(size_of_val(&storage)).expect("fixed directory buffer fits a DWORD"),
            )
        };
        if result == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_NO_MORE_FILES.cast_signed()) {
                entries.push(Err(error));
            }
            return Ok(entries);
        }
        class = FileFullDirectoryInfo;
        // SAFETY: initialized u64 storage has no padding, and the synchronous write has completed.
        let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), size_of_val(&storage)) };
        entries.extend(directory_names(bytes)?.into_iter().map(|name| Ok(path.join(name))));
    }
}

fn directory_names(mut bytes: &[u8]) -> io::Result<Vec<OsString>> {
    const NAME: usize = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileName);
    const NAME_LENGTH: usize = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileNameLength);
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid native directory entry");
    let mut names = Vec::new();
    loop {
        let header = bytes.get(..NAME).ok_or_else(invalid)?;
        let next = u32::from_ne_bytes(*header.first_chunk::<4>().ok_or_else(invalid)?) as usize;
        let length = u32::from_ne_bytes(*header.get(NAME_LENGTH..).and_then(<[u8]>::first_chunk::<4>).ok_or_else(invalid)?) as usize;
        if length == 0 || !length.is_multiple_of(2) {
            return Err(invalid());
        }
        let end = NAME.checked_add(length).ok_or_else(invalid)?;
        let units = bytes
            .get(NAME..end)
            .ok_or_else(invalid)?
            .chunks_exact(2)
            .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        let name = OsString::from_wide(&units);
        if name != "." && name != ".." {
            let mut components = Path::new(&name).components();
            if units.contains(&0) || !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
                return Err(invalid());
            }
            names.push(name);
        }
        if next == 0 {
            return Ok(names);
        }
        if next < end || !next.is_multiple_of(align_of::<u64>()) {
            return Err(invalid());
        }
        bytes = bytes.get(next..).ok_or_else(invalid)?;
    }
}

pub(crate) fn open_directory(path: &Path, owner: &MonitorOwner) -> io::Result<File> {
    let file = directory_file(path)?;
    verify_access(&file, owner)?;
    Ok(file)
}

fn verify_access(file: &File, expected: &MonitorOwner) -> io::Result<MonitorOwner> {
    verify_grants(native_access(file)?, expected)
}

fn verify_grants(access: NativeAccess, expected: &MonitorOwner) -> io::Result<MonitorOwner> {
    let actual = access.owner;
    let expected_sid = owner_sid(expected)?;
    if actual != ADMINISTRATORS && actual != SYSTEM && expected_sid != actual {
        return Err(denied("monitor registry has an untrusted OS owner"));
    }
    let mut administrators = false;
    let mut system = false;
    let mut user = false;
    for (sid, mask) in access.grants {
        let full = mask & GENERIC_ALL != 0 || mask & FILE_ALL_ACCESS == FILE_ALL_ACCESS;
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

struct NativeAccess {
    owner: String,
    grants: Vec<(String, u32)>,
}

#[cfg_attr(coverage_nightly, coverage(off))] // Security-query/GetAce failures and malformed OS-owned ACL pointers cannot safely be injected.
#[cfg_attr(test, mutants::skip)] // Native ACL rejection tests retain the boundary; owner/grant authorization stays measured in verify_access.
fn native_access(file: &File) -> io::Result<NativeAccess> {
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
    if descriptor.is_null() || owner.is_null() {
        return Err(denied("monitor registry security descriptor is incomplete"));
    }
    let actual = sid_string(owner)?;
    // An inherited owner/admin/System ACL is safe too; validate its grants
    // rather than rewriting an existing user directory's inheritance policy.
    let acl = std::ptr::NonNull::new(acl).ok_or_else(|| denied("monitor registry must have an access-control list"))?;
    let mut grants = Vec::new();
    // SAFETY: GetSecurityInfo supplied a valid ACL within the live descriptor.
    for index in 0..unsafe { acl.as_ref().AceCount } {
        let mut entry = ptr::null_mut();
        // SAFETY: index is bounded by AceCount, and entry is writable.
        if unsafe { GetAce(acl.as_ptr(), u32::from(index), &raw mut entry) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let entry = std::ptr::NonNull::new(entry).ok_or_else(|| denied("monitor registry ACL contains a null entry"))?;
        // SAFETY: OS-generated ACL entries contain an ACE_HEADER.
        let header = unsafe { entry.cast::<windows_sys::Win32::Security::ACE_HEADER>().as_ref() };
        // ACCESS_ALLOWED_ACE_TYPE is zero in the Windows ACL wire format.
        if header.AceType != 0
            || usize::from(header.AceSize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
            || u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0
        {
            return Err(denied("monitor registry ACL contains an unexpected entry"));
        }
        let entry = entry.cast::<ACCESS_ALLOWED_ACE>().as_ptr();
        // SAFETY: the validated access-allowed ACE contains a variable-length
        // SID. Project from the original allocation, not a reference to SidStart.
        let sid = unsafe { &raw mut (*entry).SidStart };
        let sid = sid_string(sid.cast())?;
        // SAFETY: the validated ACE's fixed header includes this mask.
        let mask = unsafe { (*entry).Mask };
        grants.push((sid, mask));
    }
    Ok(NativeAccess { owner: actual, grants })
}

fn make_directory(path: &Path, attributes: &Attributes) -> io::Result<File> {
    let parent = path.parent().ok_or_else(|| denied("monitor registry needs a parent directory"))?;
    let name = path.file_name().ok_or_else(|| denied("monitor registry needs a directory name"))?;
    let directory = directory_file(parent)?;
    let file = open_native(
        Some(&directory),
        name,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL,
        FILE_OPEN_IF,
        FILE_DIRECTORY_FILE,
        Some(attributes),
    )?;
    ordinary_directory(&file)?;
    Ok(file)
}

pub(crate) fn create_directory(path: &Path, owner: &MonitorOwner) -> io::Result<()> {
    let file = make_directory(path, &Attributes::for_directory(owner)?)?;
    verify_access(&file, owner).map(drop)
}

#[cfg_attr(coverage_nightly, coverage(off))] // Elevation query failures require invalidating a valid current-process token.
#[cfg_attr(test, mutants::skip)] // Native discovery retains elevation checks; pure access policy remains measured.
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
    let name = path.file_name().ok_or_else(|| denied("descriptor needs a file name"))?;
    open_native(
        Some(&directory),
        name,
        GENERIC_WRITE,
        FILE_CREATE,
        FILE_NON_DIRECTORY_FILE,
        Some(&attributes),
    )
}

#[cfg_attr(coverage_nightly, coverage(off))] // Native opens already reject directories/reparse points; post-open metadata failures require external object changes.
#[cfg_attr(test, mutants::skip)] // Native descriptor/ACL tests retain this boundary; path and grant policy stay measured separately.
pub(crate) fn read_descriptor(path: &Path) -> io::Result<(File, MonitorOwner)> {
    let parent = path.parent().ok_or_else(|| denied("descriptor needs a directory"))?;
    let directory = directory_file(parent)?;
    let name = path.file_name().ok_or_else(|| denied("descriptor needs a file name"))?;
    let file = open_native(
        Some(&directory),
        name,
        GENERIC_READ | READ_CONTROL,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE,
        None,
    )?;
    let attributes = file.metadata()?.file_attributes();
    if attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(denied("monitor descriptor is not an ordinary file"));
    }
    let owner = handle_owner(file.as_raw_handle())?;
    verify_access(&directory, &owner)?;
    verify_access(&file, &owner)?;
    Ok((file, owner))
}

#[cfg_attr(coverage_nightly, coverage(off))] // NtCreateFile status and object-manager failure paths depend on native handles and filesystem state.
#[cfg_attr(test, mutants::skip)] // Native directory, descriptor, and junction tests retain no-reparse/relative-open behavior.
fn open_native(
    directory: Option<&File>,
    name: &OsStr,
    access: u32,
    disposition: u32,
    options: u32,
    security: Option<&Attributes>,
) -> io::Result<File> {
    let mut name = wide(name)?;
    let length = u16::try_from((name.len() - 1) * 2)
        .map_err(|_error| io::Error::new(io::ErrorKind::InvalidInput, "native file name is too long"))?;
    let name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(std::mem::size_of::<OBJECT_ATTRIBUTES>()).expect("fixed Windows OBJECT_ATTRIBUTES fits a DWORD"),
        RootDirectory: directory.map_or(ptr::null_mut(), std::os::windows::io::AsRawHandle::as_raw_handle),
        ObjectName: &raw const name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: security.map_or(ptr::null(), |security| security.descriptor.0.cast()),
        SecurityQualityOfService: ptr::null(),
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = ptr::null_mut();
    // SAFETY: any root handle remains live; name, attributes, and outputs outlive
    // the synchronous call. No reparse point may redirect resolution.
    let result = unsafe {
        NtCreateFile(
            &raw mut handle,
            access | 0x0010_0000, // SYNCHRONIZE is required for synchronous native file I/O.
            &raw const attributes,
            &raw mut status,
            ptr::null(),
            0,
            FILE_SHARE_READ | if options & FILE_DIRECTORY_FILE != 0 { FILE_SHARE_WRITE } else { 0 },
            disposition,
            options | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
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

#[cfg_attr(coverage_nightly, coverage(off))] // GetSecurityInfo failure requires a native object/permission failure, not a portable fixture.
#[cfg_attr(test, mutants::skip)] // Native pipe identity and descriptor ownership tests retain OS-owner verification.
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
    fn native_names_and_owner_sids_reject_malformed_input() {
        assert_eq!(wide(OsStr::new("bad\0name")).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        for owner in [
            MonitorOwner::UnixUser(1001),
            MonitorOwner::WindowsSid("1001".into()),
            MonitorOwner::WindowsSid("invalid".into()),
        ] {
            assert_eq!(owner_sid(&owner).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "native Windows descriptor conversion and directory enumeration")]
    fn native_security_rejects_bad_descriptors_traversal_and_nondirectories() {
        Attributes::from_sddl("invalid security descriptor").unwrap_err();
        let temporary = tempfile::tempdir().unwrap();
        assert_eq!(
            directory_file(&temporary.path().join("..")).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        let path = temporary.path().join("file");
        std::fs::write(&path, b"keep").unwrap();
        let file = File::open(&path).unwrap();
        assert_eq!(ordinary_directory(&file).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        let entries = read_directory(&file, &path).unwrap();
        assert_eq!(entries.len(), 1);
        entries.into_iter().next().unwrap().unwrap_err();
        create_directory(&path, &current_owner().unwrap()).unwrap_err();
        assert_eq!(std::fs::read(&path).unwrap(), b"keep");
        create_directory(Path::new(r"C:\"), &current_owner().unwrap()).unwrap_err();
    }

    #[test]
    fn acl_policy_requires_trusted_ownership_and_all_three_full_access_grants() {
        let sid = "S-1-5-21-1-2-3-1001";
        let other = "S-1-5-21-1-2-3-1002";
        let owner = MonitorOwner::WindowsSid(sid.into());
        let grants = || {
            vec![
                (sid.into(), GENERIC_ALL),
                (ADMINISTRATORS.into(), FILE_ALL_ACCESS),
                (SYSTEM.into(), GENERIC_ALL),
            ]
        };
        for actual in [sid, ADMINISTRATORS, SYSTEM] {
            assert_eq!(
                verify_grants(
                    NativeAccess {
                        owner: actual.into(),
                        grants: grants()
                    },
                    &owner
                )
                .unwrap(),
                MonitorOwner::WindowsSid(actual.into())
            );
        }
        let mut invalid = vec![NativeAccess {
            owner: other.into(),
            grants: grants(),
        }];
        for index in 0..3 {
            let mut missing = grants();
            missing.remove(index);
            invalid.push(NativeAccess {
                owner: sid.into(),
                grants: missing,
            });
            for mask in [GENERIC_READ, FILE_READ_ATTRIBUTES] {
                let mut partial = grants();
                partial[index].1 = mask;
                invalid.push(NativeAccess {
                    owner: sid.into(),
                    grants: partial,
                });
            }
        }
        let mut foreign = grants();
        foreign.push((other.into(), GENERIC_READ));
        invalid.push(NativeAccess {
            owner: sid.into(),
            grants: foreign,
        });
        for access in invalid {
            assert_eq!(verify_grants(access, &owner).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        }
    }

    #[test]
    fn network_and_device_namespaces_are_rejected_before_opening() {
        for path in [
            r"\\server.invalid\share\monitor",
            r"\\?\UNC\server.invalid\share\monitor",
            r"\\.\UNC\server.invalid\share\monitor",
            r"\Device\Mup\server.invalid\share\monitor",
            r"C:relative",
            "relative",
        ] {
            assert_eq!(directory_file(Path::new(path)).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        }
        assert_eq!(
            local_volume_path(r"\Device\HarddiskVolume42").unwrap(),
            Path::new(r"\Device\HarddiskVolume42\")
        );
        for target in [
            r"\Device\Mup\server.invalid\share",
            r"\Device\LanmanRedirector\server.invalid\share",
            r"\??\C:\substituted",
            r"\Device\HarddiskVolume42\redirect",
            r"\Device\HarddiskVolume",
        ] {
            assert_eq!(local_volume_path(target).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        }
    }

    fn directory_row(name: &[u16]) -> Vec<u8> {
        let mut bytes = vec![0; std::mem::offset_of!(FILE_FULL_DIR_INFO, FileName)];
        let length = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileNameLength);
        bytes[length..length + 4].copy_from_slice(&u32::try_from(name.len() * 2).unwrap().to_ne_bytes());
        for unit in name {
            bytes.extend_from_slice(&unit.to_ne_bytes());
        }
        bytes
    }

    #[test]
    fn native_directory_records_validate_names_and_offsets() {
        let name = "entry.monitor".encode_utf16().collect::<Vec<_>>();
        let first = directory_row(&name);
        assert_eq!(directory_names(&first).unwrap(), [OsString::from("entry.monitor")]);
        assert_eq!(
            directory_names(&directory_row(&[0xd800])).unwrap(),
            [OsString::from_wide(&[0xd800])]
        );
        for name in [".", ".."] {
            assert!(
                directory_names(&directory_row(&name.encode_utf16().collect::<Vec<_>>()))
                    .unwrap()
                    .is_empty()
            );
        }
        for name in ["", "bad\0name", r"parent\child", r"C:\absolute"] {
            assert_eq!(
                directory_names(&directory_row(&name.encode_utf16().collect::<Vec<_>>()))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        for bytes in [&[][..], &first[..first.len() - 1]] {
            assert_eq!(directory_names(bytes).unwrap_err().kind(), io::ErrorKind::InvalidData);
        }
        let length = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileNameLength);
        let mut odd = first.clone();
        odd[length..length + 4].copy_from_slice(&3_u32.to_ne_bytes());
        assert_eq!(directory_names(&odd).unwrap_err().kind(), io::ErrorKind::InvalidData);
        for next in [1, u32::try_from(first.len() + 1).unwrap(), u32::MAX] {
            let mut bytes = first.clone();
            bytes[..4].copy_from_slice(&next.to_ne_bytes());
            assert_eq!(directory_names(&bytes).unwrap_err().kind(), io::ErrorKind::InvalidData);
        }
        let next = first.len().next_multiple_of(align_of::<u64>());
        let mut multiple = first.clone();
        multiple.resize(next, 0);
        multiple[..4].copy_from_slice(&u32::try_from(next).unwrap().to_ne_bytes());
        multiple.extend_from_slice(&first);
        assert_eq!(
            directory_names(&multiple).unwrap(),
            [OsString::from("entry.monitor"), OsString::from("entry.monitor")]
        );
        let unaligned = first.len() + 1;
        let mut multiple = first.clone();
        multiple.resize(unaligned, 0);
        multiple[..4].copy_from_slice(&u32::try_from(unaligned).unwrap().to_ne_bytes());
        multiple.extend_from_slice(&first);
        assert_eq!(directory_names(&multiple).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    #[cfg_attr(miri, ignore = "native and careful tests exercise handle-based Windows directory enumeration")]
    fn enumeration_uses_the_handle_not_the_display_path_and_reads_all_pages() {
        let temporary = tempfile::tempdir().unwrap();
        let display = Path::new(r"\\must-not-be-contacted.invalid\share");
        let mut expected = Vec::new();
        for index in 0..80 {
            let name = format!("{index:02}-{}.monitor", "x".repeat(120));
            std::fs::write(temporary.path().join(&name), b"entry").unwrap();
            expected.push(display.join(name));
        }
        let file = directory_file(temporary.path()).unwrap();
        let mut actual = read_directory(&file, display)
            .unwrap()
            .into_iter()
            .collect::<io::Result<Vec<_>>>()
            .unwrap();
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected);
        let mut actual = directory_entries(temporary.path())
            .unwrap()
            .into_iter()
            .collect::<io::Result<Vec<_>>>()
            .unwrap();
        let mut expected = expected
            .iter()
            .map(|path| temporary.path().join(path.file_name().unwrap()))
            .collect::<Vec<_>>();
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected);
    }

    #[test]
    #[cfg_attr(miri, ignore = "native Windows named-pipe security descriptors")]
    fn native_pipe_attributes_grant_only_the_owner_administrators_and_system() {
        let directory = crate::testing::Directory::new();
        let descriptor = directory.descriptor();
        let listener = super::super::Listener::bind(&descriptor).unwrap();
        let file = File::from(listener.accept.lock().unwrap().handle.try_clone().unwrap());
        assert_eq!(verify_access(&file, &descriptor.owner).unwrap(), descriptor.owner);
    }

    fn junction(path: &Path, target: &Path) {
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, OPEN_EXISTING,
        };
        use windows_sys::Win32::System::IO::DeviceIoControl;
        use windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;

        std::fs::create_dir(path).unwrap();
        let target = target.canonicalize().unwrap();
        let target = target.to_str().unwrap();
        let target = target.strip_prefix(r"\\?\").unwrap_or(target);
        let substitute = format!(r"\??\{target}").encode_utf16().chain([0]).collect::<Vec<_>>();
        let print = target.encode_utf16().chain([0]).collect::<Vec<_>>();
        // Windows mount-point reparse data: tag, payload length, name offsets, UTF-16 names.
        let mut data = 0xa000_0003_u32.to_le_bytes().to_vec();
        data.extend_from_slice(&u16::try_from(8 + (substitute.len() + print.len()) * 2).unwrap().to_le_bytes());
        data.extend_from_slice(&0_u16.to_le_bytes());
        for value in [0, (substitute.len() - 1) * 2, substitute.len() * 2, (print.len() - 1) * 2] {
            data.extend_from_slice(&u16::try_from(value).unwrap().to_le_bytes());
        }
        for unit in substitute.into_iter().chain(print) {
            data.extend_from_slice(&unit.to_le_bytes());
        }
        let path = wide(path.as_os_str()).unwrap();
        // SAFETY: path is terminated; this new directory's non-inheritable handle is owned.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        let handle = owned_handle(handle).unwrap();
        let mut returned = 0;
        // SAFETY: the complete mount-point buffer and output count outlive the synchronous call.
        let result = unsafe {
            DeviceIoControl(
                handle.as_raw_handle(),
                FSCTL_SET_REPARSE_POINT,
                data.as_ptr().cast(),
                u32::try_from(data.len()).unwrap(),
                ptr::null_mut(),
                0,
                &raw mut returned,
                ptr::null_mut(),
            )
        };
        assert_ne!(result, 0, "{}", io::Error::last_os_error());
    }

    #[test]
    #[cfg_attr(miri, ignore = "native Windows reparse-point traversal is covered by native and careful tests")]
    fn directory_open_rejects_an_intermediate_junction() {
        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("target");
        let registry = target.join("monitor");
        std::fs::create_dir_all(&registry).unwrap();
        directory_file(&registry).unwrap();
        let redirect = temporary.path().join("redirect");
        junction(&redirect, &target);
        directory_file(&redirect.join("monitor")).unwrap_err();
        std::fs::remove_dir(&redirect).unwrap();
    }

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
