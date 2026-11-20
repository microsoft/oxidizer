// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::io;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::windows::fs::MetadataExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr::{self, NonNull};
#[cfg(test)]
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

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
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation, INHERIT_ONLY_ACE,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation,
    TokenUser,
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
enum LocalAllocation {
    Native(*mut c_void),
    #[cfg(test)]
    Injected(Arc<AtomicUsize>),
}

impl LocalAllocation {
    const fn pointer(&self) -> *mut c_void {
        match self {
            Self::Native(pointer) => *pointer,
            #[cfg(test)]
            Self::Injected(_) => ptr::null_mut(),
        }
    }
}

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        match self {
            Self::Native(pointer) => {
                // SAFETY: this pointer is owned and was returned by a LocalAlloc-family API.
                unsafe { LocalFree(*pointer) };
            }
            #[cfg(test)]
            Self::Injected(releases) => {
                releases.fetch_add(1, Ordering::Relaxed);
            }
        }
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
            descriptor: LocalAllocation::Native(descriptor),
        })
    }

    pub(super) fn native(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).expect("fixed Windows SECURITY_ATTRIBUTES fits a DWORD"),
            lpSecurityDescriptor: self.descriptor.pointer(),
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
    owned_handle_with(handle, io::Error::last_os_error, |handle| {
        // SAFETY: callers pass a fresh owning handle returned by a creation/open API.
        unsafe { OwnedHandle::from_raw_handle(handle) }
    })
}

fn owned_handle_with<T>(handle: HANDLE, error: impl FnOnce() -> io::Error, own: impl FnOnce(HANDLE) -> T) -> io::Result<T> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(error());
    }
    Ok(own(handle))
}

#[cfg_attr(coverage_nightly, coverage(off))] // Token-query failures require inaccessible processes or invalidating an owned handle.
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
pub(super) fn process_owner(process: HANDLE) -> io::Result<MonitorOwner> {
    process_owner_with(
        || process_token(process),
        |token, storage, capacity, length| {
            // SAFETY: the helper supplies an aligned buffer of the advertised capacity, or a null size-query buffer.
            unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, storage, capacity, length) }
        },
        io::Error::last_os_error,
        sid_string,
    )
}

fn process_owner_with<T>(
    token: impl FnOnce() -> io::Result<T>,
    mut query: impl FnMut(&T, *mut c_void, u32, &mut u32) -> i32,
    error: impl Fn() -> io::Error,
    sid: impl FnOnce(PSID) -> io::Result<String>,
) -> io::Result<MonitorOwner> {
    let token = token()?;
    // #[gamma::skip(literal.int_increment, reason = "the size query overwrites length, or either initial value zero or one remains below size_of TOKEN_USER and returns the same native error")]
    let mut length = 0;
    query(&token, ptr::null_mut(), 0, &mut length);
    if (length as usize) < std::mem::size_of::<TOKEN_USER>() {
        return Err(error());
    }
    // TOKEN_USER contains pointers; use pointer-aligned storage rather than Vec<u8>.
    let mut storage = vec![0_usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
    if query(&token, storage.as_mut_ptr().cast(), length, &mut length) == 0 {
        return Err(error());
    }
    // SAFETY: successful GetTokenInformation initialized a TOKEN_USER in aligned storage.
    let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
    sid(user.User.Sid).map(MonitorOwner::WindowsSid)
}

#[cfg_attr(coverage_nightly, coverage(off))] // OS-owned SIDs cannot safely be corrupted to force conversion failures.
fn sid_string(sid: PSID) -> io::Result<String> {
    sid_string_with(
        |text| {
            // SAFETY: sid belongs to a live token/security descriptor, and text is writable.
            unsafe { ConvertSidToStringSidW(sid, text) }
        },
        io::Error::last_os_error,
        |text| LocalAllocation::Native(text.cast()),
        |text, offset| {
            // SAFETY: the successful converter returned a terminated UTF-16 allocation.
            unsafe { *text.wrapping_add(offset) }
        },
        |text, length| {
            // SAFETY: the scan established the readable prefix, and its owning allocation remains alive.
            String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
        },
    )
}

fn sid_string_with<A>(
    convert: impl FnOnce(&mut *mut u16) -> i32,
    error: impl FnOnce() -> io::Error,
    own: impl FnOnce(*mut u16) -> A,
    mut read: impl FnMut(*mut u16, usize) -> u16,
    decode: impl FnOnce(*mut u16, usize) -> Result<String, std::string::FromUtf16Error>,
) -> io::Result<String> {
    let mut text = ptr::null_mut();
    if convert(&mut text) == 0 {
        return Err(error());
    }
    let _allocation = own(text);
    let mut length = 0;
    while read(text, length) != 0 {
        // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        length +=
        // #[gamma::skip(literal.int_decrement, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        1;
    }
    decode(text, length).map_err(|_error| io::Error::new(io::ErrorKind::InvalidData, "OS SID is not valid UTF-16"))
}

fn directory_file(path: &Path) -> io::Result<File> {
    directory_file_with(
        path,
        drive_volume,
        |parent, name, access, disposition, options| open_native(parent, name, access, disposition, options, None),
        ordinary_directory,
    )
}

fn directory_file_with<F>(
    path: &Path,
    volume: impl FnOnce(u8) -> io::Result<PathBuf>,
    mut open: impl FnMut(Option<&F>, &OsStr, u32, u32, u32) -> io::Result<F>,
    mut ordinary: impl FnMut(&F) -> io::Result<()>,
) -> io::Result<F> {
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
    let root = volume(drive)?;
    let access = FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL;
    let mut file = open(None, root.as_os_str(), access, FILE_OPEN, FILE_DIRECTORY_FILE)?;
    ordinary(&file)?;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(denied("monitor registry path contains a traversal component"));
        };
        file = open(Some(&file), name, access, FILE_OPEN, FILE_DIRECTORY_FILE)?;
        ordinary(&file)?;
    }
    Ok(file)
}

#[cfg_attr(coverage_nightly, coverage(off))] // DOS-device query failures and malformed kernel mappings require changing machine drive configuration.
fn drive_volume(drive: u8) -> io::Result<PathBuf> {
    drive_volume_with(
        drive,
        |device, target, capacity| {
            // SAFETY: device is terminated and target is a writable bounded UTF-16 buffer.
            unsafe { QueryDosDeviceW(device.as_ptr(), target.as_mut_ptr(), capacity) }
        },
        io::Error::last_os_error,
    )
}

fn drive_volume_with(
    drive: u8,
    query: impl FnOnce(&[u16; 3], &mut [u16], u32) -> u32,
    error: impl FnOnce() -> io::Error,
) -> io::Result<PathBuf> {
    let device = [u16::from(drive), u16::from(b':'), 0];
    // DOS device queries inspect the local object namespace, not the target filesystem.
    let mut target = vec![0_u16; 32_768];
    let capacity = u32::try_from(target.len()).expect("bounded DOS device buffer fits a DWORD");
    let length = query(&device, &mut target, capacity);
    if length == 0 {
        return Err(error());
    }
    dos_device_mapping_path(&target, length as usize)
}

fn dos_device_mapping_path(target: &[u16], length: usize) -> io::Result<PathBuf> {
    let end = target[..length]
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
    ordinary_directory_with_attributes(|| file.metadata().map(|metadata| metadata.file_attributes()))
}

fn ordinary_directory_with_attributes(attributes: impl FnOnce() -> io::Result<u32>) -> io::Result<()> {
    let attributes = attributes()?;
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(denied("monitor directory is not an ordinary directory"));
    }
    Ok(())
}

pub(crate) fn directory_entries(path: &Path) -> io::Result<Vec<io::Result<PathBuf>>> {
    read_directory(&directory_file(path)?, path)
}

fn read_directory(directory: &File, path: &Path) -> io::Result<Vec<io::Result<PathBuf>>> {
    read_directory_with(
        path,
        |class, storage| {
            // SAFETY: directory is live; storage is aligned and writable for its fixed size.
            unsafe {
                GetFileInformationByHandleEx(
                    directory.as_raw_handle(),
                    class,
                    storage.as_mut_ptr().cast(),
                    u32::try_from(size_of_val(storage)).expect("fixed directory buffer fits a DWORD"),
                )
            }
        },
        io::Error::last_os_error,
    )
}

fn read_directory_with(
    path: &Path,
    mut query: impl FnMut(i32, &mut [u64; 1_024]) -> i32,
    mut error: impl FnMut() -> io::Error,
) -> io::Result<Vec<io::Result<PathBuf>>> {
    // Enumerate through the local handle; reopening the pathname would reintroduce
    // ancestor-replacement races after directory validation.
    // #[gamma::skip(literal.int_increment, reason = "successful native queries initialize every returned record field and name; directory_names follows initialized next offsets and never reads the unreported suffix, while failure ignores storage")]
    let mut storage = [0_u64; 1_024];
    let mut class = FileFullDirectoryRestartInfo;
    let mut entries = Vec::new();
    loop {
        let result = query(class, &mut storage);
        // #[gamma::skip(cond.always_false, tag = "outofmem", reason = "November 6 campaign f1fbf4111500: bypassing directory enumeration termination exceeded the memory limit")]
        if result == 0 {
            let error = error();
            if error.raw_os_error() != Some(ERROR_NO_MORE_FILES.cast_signed()) {
                entries.push(Err(error));
            }
            return Ok(entries);
        }
        // #[gamma::skip(stmt.delete_assign, tag = "outofmem", reason = "November 4 campaign: this mutation exceeded the memory limit")]
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
        // #[gamma::skip(range.exclusive_to_inclusive, expr.decrement, expr.increment, reason = "the fixed ABI fields end before NAME minus one; every valid nonempty UTF-16 name supplies at least two bytes after NAME, and shorter records still fail below")]
        let header = bytes.get(..NAME).ok_or_else(invalid)?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the checked native header is larger than the four-byte next-offset field")]
        let next = u32::from_ne_bytes(*header.first_chunk::<4>().ok_or_else(invalid)?) as usize;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the checked native header includes the fixed FileNameLength field in full")]
        let length = u32::from_ne_bytes(*header.get(NAME_LENGTH..).and_then(<[u8]>::first_chunk::<4>).ok_or_else(invalid)?) as usize;
        // #[gamma::skip(logical.or_remove_left, reason = "a zero-length name still fails the subsequent requirement for one normal path component with the same invalid-entry error")]
        if {
            // #[gamma::skip(literal.int_increment, reason = "length one is already rejected as odd; zero would still fail the subsequent required normal nonempty path component with the same invalid-entry error")]
            length == 0
        } || !length.is_multiple_of(2)
        {
            return Err(invalid());
        }
        // #[gamma::skip(expr.increment, reason = "increasing the admitted extent by one only defers an oversized-name error to the following checked slice, which returns the identical invalid-entry error")]
        let end = directory_name_end(NAME, length, bytes.len())?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "directory_name_end checked addition and proved end <= bytes.len(); length is nonzero and NAME was checked above, so this slice cannot fail")]
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
        // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        bytes = bytes.get(next..).ok_or_else(invalid)?;
    }
}

fn directory_name_end(start: usize, length: usize, maximum: usize) -> io::Result<usize> {
    start
        .checked_add(length)
        .filter(|end| *end <= maximum)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid native directory entry"))
}

pub(crate) fn open_directory(path: &Path, owner: &MonitorOwner) -> io::Result<File> {
    let file = directory_file(path)?;
    verify_access(&file, owner)?;
    Ok(file)
}

fn verify_access(file: &File, expected: &MonitorOwner) -> io::Result<MonitorOwner> {
    verify_access_with(|| native_access(file), expected)
}

fn verify_access_with(query: impl FnOnce() -> io::Result<NativeAccess>, expected: &MonitorOwner) -> io::Result<MonitorOwner> {
    verify_grants(query()?, expected)
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
fn native_access(file: &File) -> io::Result<NativeAccess> {
    native_access_with(
        |owner, acl, descriptor| {
            // SAFETY: file owns a valid handle and the helper supplies writable output pointers.
            unsafe {
                GetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    owner,
                    ptr::null_mut(),
                    acl,
                    ptr::null_mut(),
                    descriptor,
                )
            }
        },
        LocalAllocation::Native,
        sid_string,
        |acl, index| {
            let mut entry = ptr::null_mut();
            // SAFETY: the live descriptor owns the ACL and the helper bounds index by its AceCount.
            if unsafe { GetAce(acl, index, &raw mut entry) } == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(entry)
            }
        },
    )
}

fn native_access_with<A>(
    query: impl FnOnce(&mut PSID, &mut *mut ACL, &mut PSECURITY_DESCRIPTOR) -> u32,
    own: impl FnOnce(PSECURITY_DESCRIPTOR) -> A,
    mut sid_string: impl FnMut(PSID) -> io::Result<String>,
    mut get_entry: impl FnMut(*mut ACL, u32) -> io::Result<*mut c_void>,
) -> io::Result<NativeAccess> {
    let mut owner = ptr::null_mut();
    let mut acl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let status = query(&mut owner, &mut acl, &mut descriptor);
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status.cast_signed()));
    }
    let _allocation = own(descriptor);
    validate_descriptor_presence(descriptor, owner)?;
    let actual = sid_string(owner)?;
    // An inherited owner/admin/System ACL is safe too; validate its grants
    // rather than rewriting an existing user directory's inheritance policy.
    let acl = checked_native_acl(acl)?;
    let mut grants = Vec::new();
    // SAFETY: GetSecurityInfo supplied a valid ACL within the live descriptor.
    for index in 0..unsafe { acl.as_ref().AceCount } {
        let entry = get_entry(acl.as_ptr(), u32::from(index))?;
        let entry = checked_native_entry(entry)?;
        // SAFETY: OS-generated ACL entries contain an ACE_HEADER.
        let header = unsafe { entry.cast::<ACE_HEADER>().as_ref() };
        // ACCESS_ALLOWED_ACE_TYPE is zero in the Windows ACL wire format.
        validate_access_entry_header(*header)?;
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

fn validate_descriptor_presence(descriptor: PSECURITY_DESCRIPTOR, owner: PSID) -> io::Result<()> {
    if descriptor.is_null() || owner.is_null() {
        return Err(denied("monitor registry security descriptor is incomplete"));
    }
    Ok(())
}

fn checked_native_acl(acl: *mut ACL) -> io::Result<NonNull<ACL>> {
    NonNull::new(acl).ok_or_else(|| denied("monitor registry must have an access-control list"))
}

fn checked_native_entry(entry: *mut c_void) -> io::Result<NonNull<c_void>> {
    NonNull::new(entry).ok_or_else(|| denied("monitor registry ACL contains a null entry"))
}

fn validate_access_entry_header(header: ACE_HEADER) -> io::Result<()> {
    if header.AceType != 0
        || usize::from(header.AceSize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
        || u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0
    {
        return Err(denied("monitor registry ACL contains an unexpected entry"));
    }
    Ok(())
}

fn make_directory(path: &Path, attributes: &Attributes) -> io::Result<File> {
    make_directory_with(
        path,
        directory_file,
        |parent, name, access, disposition, options| open_native(Some(parent), name, access, disposition, options, Some(attributes)),
        ordinary_directory,
    )
}

fn make_directory_with<F>(
    path: &Path,
    parent_directory: impl FnOnce(&Path) -> io::Result<F>,
    open: impl FnOnce(&F, &OsStr, u32, u32, u32) -> io::Result<F>,
    ordinary: impl FnOnce(&F) -> io::Result<()>,
) -> io::Result<F> {
    let parent = path.parent().ok_or_else(|| denied("monitor registry needs a parent directory"))?;
    let name = path.file_name().ok_or_else(|| denied("monitor registry needs a directory name"))?;
    let directory = parent_directory(parent)?;
    let file = open(
        &directory,
        name,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL,
        FILE_OPEN_IF,
        FILE_DIRECTORY_FILE,
    )?;
    ordinary(&file)?;
    Ok(file)
}

pub(crate) fn create_directory(path: &Path, owner: &MonitorOwner) -> io::Result<()> {
    let file = make_directory(path, &Attributes::for_directory(owner)?)?;
    verify_access(&file, owner).map(drop)
}

#[cfg_attr(coverage_nightly, coverage(off))] // Elevation query failures require invalidating a valid current-process token.
// #[gamma::skip(fn_value.ok, reason = "native adapter: the actual current-process token query needs an owned Windows token; is_elevated_with tests token/query errors, exact request size, output interpretation and token release")]
pub(super) fn is_elevated() -> io::Result<bool> {
    is_elevated_with(
        // SAFETY: current-process pseudo-handle is valid for token queries.
        || process_token(unsafe { GetCurrentProcess() }),
        |token, elevation, capacity, length| {
            // SAFETY: elevation is a properly sized output structure and all outputs remain live.
            unsafe {
                GetTokenInformation(
                    token.as_raw_handle(),
                    TokenElevation,
                    ptr::from_mut(elevation).cast(),
                    capacity,
                    length,
                )
            }
        },
        io::Error::last_os_error,
    )
}

fn is_elevated_with<T>(
    token: impl FnOnce() -> io::Result<T>,
    query: impl FnOnce(&T, &mut TOKEN_ELEVATION, u32, &mut u32) -> i32,
    error: impl FnOnce() -> io::Error,
) -> io::Result<bool> {
    let token = token()?;
    let mut elevation = TOKEN_ELEVATION::default();
    // #[gamma::skip(literal.int_increment, reason = "GetTokenInformation length is an output only and this function never reads it")]
    let mut length = 0;
    let capacity = u32::try_from(std::mem::size_of_val(&elevation)).expect("fixed Windows TOKEN_ELEVATION fits a DWORD");
    if query(&token, &mut elevation, capacity, &mut length) == 0 {
        return Err(error());
    }
    Ok(elevation.TokenIsElevated != 0)
}

pub(crate) fn create_descriptor(path: &Path, owner: &MonitorOwner) -> io::Result<File> {
    create_descriptor_with(path, owner, open_directory, Attributes::for_owner, |directory, name, attributes| {
        open_native(
            Some(directory),
            name,
            GENERIC_WRITE,
            FILE_CREATE,
            FILE_NON_DIRECTORY_FILE,
            attributes,
        )
    })
}

fn create_descriptor_with<F, A>(
    path: &Path,
    owner: &MonitorOwner,
    directory: impl FnOnce(&Path, &MonitorOwner) -> io::Result<F>,
    attributes: impl FnOnce(&MonitorOwner) -> io::Result<A>,
    create: impl FnOnce(&F, &OsStr, Option<&A>) -> io::Result<F>,
) -> io::Result<F> {
    let parent = path.parent().ok_or_else(|| denied("descriptor needs a directory"))?;
    let directory = directory(parent, owner)?;
    let attributes = attributes(owner)?;
    let name = path.file_name().ok_or_else(|| denied("descriptor needs a file name"))?;
    create(&directory, name, Some(&attributes))
}

#[cfg_attr(coverage_nightly, coverage(off))] // Native opens already reject directories/reparse points; post-open metadata failures require external object changes.
pub(crate) fn read_descriptor(path: &Path) -> io::Result<(File, MonitorOwner)> {
    read_descriptor_with(
        path,
        directory_file,
        |directory, name| {
            open_native(
                Some(directory),
                name,
                GENERIC_READ | READ_CONTROL,
                FILE_OPEN,
                FILE_NON_DIRECTORY_FILE,
                None,
            )
        },
        |file| file.metadata().map(|metadata| metadata.file_attributes()),
        |file| handle_owner(file.as_raw_handle()),
        verify_access,
    )
}

fn read_descriptor_with<F>(
    path: &Path,
    directory: impl FnOnce(&Path) -> io::Result<F>,
    open: impl FnOnce(&F, &OsStr) -> io::Result<F>,
    attributes: impl FnOnce(&F) -> io::Result<u32>,
    owner: impl FnOnce(&F) -> io::Result<MonitorOwner>,
    mut verify: impl FnMut(&F, &MonitorOwner) -> io::Result<MonitorOwner>,
) -> io::Result<(F, MonitorOwner)> {
    let parent = path.parent().ok_or_else(|| denied("descriptor needs a directory"))?;
    let directory = directory(parent)?;
    let name = path.file_name().ok_or_else(|| denied("descriptor needs a file name"))?;
    let file = open(&directory, name)?;
    let attributes = attributes(&file)?;
    if attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(denied("monitor descriptor is not an ordinary file"));
    }
    let owner = owner(&file)?;
    verify(&directory, &owner)?;
    verify(&file, &owner)?;
    Ok((file, owner))
}

#[cfg_attr(coverage_nightly, coverage(off))] // NtCreateFile status and object-manager failure paths depend on native handles and filesystem state.
fn open_native(
    directory: Option<&File>,
    name: &OsStr,
    access: u32,
    disposition: u32,
    options: u32,
    security: Option<&Attributes>,
) -> io::Result<File> {
    open_native_with(
        NativeOpenRequest {
            directory: directory.map_or(ptr::null_mut(), std::os::windows::io::AsRawHandle::as_raw_handle),
            name,
            access,
            disposition,
            options,
            security: security.map_or(ptr::null(), |security| security.descriptor.pointer().cast()),
        },
        |request, handle, status| {
            // SAFETY: root/security resources remain owned by the caller, all borrowed names outlive this synchronous call.
            unsafe {
                NtCreateFile(
                    handle,
                    request.access,
                    request.attributes,
                    status,
                    request.allocation_size,
                    request.file_attributes,
                    request.share,
                    request.disposition,
                    request.options,
                    request.ea,
                    request.ea_length,
                )
            }
        },
        |status| {
            // SAFETY: translating a returned NTSTATUS has no pointer preconditions.
            unsafe { RtlNtStatusToDosError(status) }
        },
        |handle| owned_handle(handle).map(File::from),
    )
}

#[derive(Clone, Copy, Debug)]
struct NativeOpenRequest<'a> {
    directory: HANDLE,
    name: &'a OsStr,
    access: u32,
    disposition: u32,
    options: u32,
    security: *const c_void,
}

struct NativeCreate<'a> {
    access: u32,
    attributes: &'a OBJECT_ATTRIBUTES,
    allocation_size: *const i64,
    file_attributes: u32,
    share: u32,
    disposition: u32,
    options: u32,
    ea: *const c_void,
    ea_length: u32,
}

fn open_native_with<T>(
    request: NativeOpenRequest<'_>,
    create: impl FnOnce(&NativeCreate<'_>, &mut HANDLE, &mut IO_STATUS_BLOCK) -> i32,
    status_error: impl FnOnce(i32) -> u32,
    own: impl FnOnce(HANDLE) -> io::Result<T>,
) -> io::Result<T> {
    let mut name = wide(request.name)?;
    let length = u16::try_from((name.len() - 1) * 2)
        .map_err(|_error| io::Error::new(io::ErrorKind::InvalidInput, "native file name is too long"))?;
    let name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(std::mem::size_of::<OBJECT_ATTRIBUTES>()).expect("fixed Windows OBJECT_ATTRIBUTES fits a DWORD"),
        RootDirectory: request.directory,
        ObjectName: &raw const name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: request.security.cast(),
        SecurityQualityOfService: ptr::null(),
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = ptr::null_mut();
    let native = NativeCreate {
        access: request.access | 0x0010_0000, // SYNCHRONIZE is required for synchronous native file I/O.
        attributes: &attributes,
        allocation_size: ptr::null(),
        file_attributes: 0,
        share: FILE_SHARE_READ
            | if request.options & FILE_DIRECTORY_FILE != 0 {
                FILE_SHARE_WRITE
            } else {
                // #[gamma::skip(literal.int_increment, reason = "FILE_SHARE_READ is already bit one in the enclosing OR; replacing this zero with one cannot add a flag")]
                0
            },
        disposition: request.disposition,
        options: request.options | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
        ea: ptr::null(),
        ea_length: 0,
    };
    let result = create(&native, &mut handle, &mut status);
    if result < 0 {
        return Err(io::Error::from_raw_os_error(status_error(result).cast_signed()));
    }
    own(handle)
}

#[cfg_attr(coverage_nightly, coverage(off))] // GetSecurityInfo failure requires a native object/permission failure, not a portable fixture.
pub(super) fn handle_owner(handle: HANDLE) -> io::Result<MonitorOwner> {
    handle_owner_with(
        |owner, descriptor| {
            // SAFETY: the caller retains a valid file/pipe handle; outputs remain writable.
            unsafe {
                GetSecurityInfo(
                    handle,
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION,
                    owner,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    descriptor,
                )
            }
        },
        LocalAllocation::Native,
        sid_string,
    )
}

fn handle_owner_with<A>(
    query: impl FnOnce(&mut PSID, &mut PSECURITY_DESCRIPTOR) -> u32,
    own: impl FnOnce(PSECURITY_DESCRIPTOR) -> A,
    sid: impl FnOnce(PSID) -> io::Result<String>,
) -> io::Result<MonitorOwner> {
    let mut owner = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    let status = query(&mut owner, &mut descriptor);
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status.cast_signed()));
    }
    let _allocation = own(descriptor);
    sid(owner).map(MonitorOwner::WindowsSid)
}

fn denied(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[test]
    fn nov6_directory_name_extent_uses_the_actual_record_limit_without_native_io() {
        let row = directory_row(&[u16::from(b'x')]);
        assert_eq!(directory_names(&row).unwrap(), [OsString::from("x")]);
        let mut invalid = row;
        let offset = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileNameLength);
        invalid[offset..offset + 4].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert_eq!(directory_names(&invalid).unwrap_err().kind(), io::ErrorKind::InvalidData);
        invalid[offset..offset + 4].copy_from_slice(&4u32.to_ne_bytes());
        assert_eq!(directory_names(&invalid).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    use std::cell::{Cell, RefCell};

    use super::*;

    #[derive(Debug)]
    struct DirectoryPolicyFile<'a> {
        tag: &'static str,
        events: &'a RefCell<Vec<&'static str>>,
    }

    impl Drop for DirectoryPolicyFile<'_> {
        fn drop(&mut self) {
            self.events
                .borrow_mut()
                .push(if self.tag == "root" { "root-drop" } else { "child-drop" });
        }
    }

    #[test]
    fn november_directory_walk_propagates_each_open_validation_and_traversal_failure_with_exact_ownership() {
        for case in ["root-open", "root-verify", "child-open", "child-verify", "traversal", "success"] {
            assert_directory_walk(case);
        }
    }

    fn assert_directory_walk(case: &str) {
        let events = RefCell::new(Vec::new());
        let path = Path::new(if case == "traversal" { r"C:\child\..\leaf" } else { r"C:\child" });
        let result = directory_file_with(
            path,
            |drive| {
                assert_eq!(drive, b'C');
                events.borrow_mut().push("volume");
                Ok(PathBuf::from(r"\Device\HarddiskVolume7\"))
            },
            |parent: Option<&DirectoryPolicyFile<'_>>, name, access, disposition, options| {
                assert_eq!((access, disposition, options), (0x0002_0081, 1, 1));
                let tag = if let Some(parent) = parent {
                    assert_eq!(parent.tag, "root");
                    assert_eq!(name, "child");
                    events.borrow_mut().push("open-child");
                    "child"
                } else {
                    assert_eq!(name, r"\Device\HarddiskVolume7\");
                    events.borrow_mut().push("open-root");
                    "root"
                };
                if case == format!("{tag}-open") {
                    Err(io::Error::other(case.to_owned()))
                } else {
                    Ok(DirectoryPolicyFile { tag, events: &events })
                }
            },
            |file| {
                events
                    .borrow_mut()
                    .push(if file.tag == "root" { "verify-root" } else { "verify-child" });
                if case == format!("{}-verify", file.tag) {
                    Err(io::Error::other(case.to_owned()))
                } else {
                    Ok(())
                }
            },
        );
        if case == "success" {
            let file = result.unwrap();
            assert_eq!(file.tag, "child");
            assert_eq!(
                *events.borrow(),
                ["volume", "open-root", "verify-root", "open-child", "root-drop", "verify-child"]
            );
            drop(file);
        } else {
            let error = result.unwrap_err();
            if case == "traversal" {
                assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                assert_eq!(error.to_string(), "monitor registry path contains a traversal component");
            } else {
                assert_eq!(error.to_string(), case);
            }
        }
        let expected = match case {
            "root-open" => vec!["volume", "open-root"],
            "root-verify" => vec!["volume", "open-root", "verify-root", "root-drop"],
            "child-open" => vec!["volume", "open-root", "verify-root", "open-child", "root-drop"],
            _ => vec![
                "volume",
                "open-root",
                "verify-root",
                "open-child",
                "root-drop",
                "verify-child",
                "child-drop",
            ],
        };
        assert_eq!(*events.borrow(), expected);
    }

    #[test]
    fn november_drive_query_preserves_zero_failure_and_exact_full_capacity_utf16_bounds() {
        for case in ["error", "empty", "unterminated", "ordinary", "capacity"] {
            let prefix = r"\Device\HarddiskVolume";
            let mapping = if case == "capacity" {
                format!("{prefix}{}", "7".repeat(32_768 - prefix.len() - 1))
            } else {
                format!("{prefix}7")
            };
            let units = mapping.encode_utf16().chain([0]).collect::<Vec<_>>();
            let result = drive_volume_with(
                b'C',
                |device, target, capacity| {
                    assert_eq!(*device, [67, 58, 0]);
                    assert_eq!(capacity, 32_768);
                    assert_eq!(target.len(), 32_768);
                    match case {
                        "error" => 0,
                        "empty" => {
                            target[0] = 0;
                            1
                        }
                        _ => {
                            target[..units.len()].copy_from_slice(&units);
                            u32::try_from(units.len() - usize::from(case == "unterminated")).unwrap()
                        }
                    }
                },
                || {
                    assert_eq!(case, "error");
                    io::Error::from_raw_os_error(5)
                },
            );
            match case {
                "error" => assert_eq!(result.unwrap_err().raw_os_error(), Some(5)),
                "empty" => assert_eq!(result.unwrap_err().to_string(), "monitor registry requires a local disk volume"),
                "unterminated" => assert_eq!(result.unwrap_err().to_string(), "DOS device mapping is unterminated"),
                _ => assert_eq!(result.unwrap(), PathBuf::from(format!("{mapping}\\"))),
            }
        }
    }

    #[test]
    fn november_native_directory_enumeration_propagates_parser_failures_and_preserves_partial_os_errors() {
        const NAME: usize = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileName);
        const LENGTH: usize = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileNameLength);
        for case in ["success", "malformed", "native-error"] {
            let calls = Cell::new(0);
            let result = read_directory_with(
                Path::new(r"C:\registry"),
                |class, storage| {
                    let call = calls.get();
                    calls.set(call + 1);
                    assert_eq!(
                        class,
                        if call == 0 {
                            FileFullDirectoryRestartInfo
                        } else {
                            FileFullDirectoryInfo
                        }
                    );
                    if case == "native-error" || call == 1 {
                        return 0;
                    }
                    assert_eq!(call, 0);
                    let mut record = vec![0; (NAME + 4).next_multiple_of(8)];
                    record[LENGTH..LENGTH + 4].copy_from_slice(&if case == "malformed" { 1_u32 } else { 4 }.to_ne_bytes());
                    record[NAME..NAME + 4].copy_from_slice(&[65, 0, 66, 0]);
                    for (word, chunk) in storage.iter_mut().zip(record.chunks_exact(8)) {
                        *word = u64::from_ne_bytes(chunk.try_into().unwrap());
                    }
                    1
                },
                || {
                    io::Error::from_raw_os_error(if case == "native-error" {
                        5
                    } else {
                        ERROR_NO_MORE_FILES.cast_signed()
                    })
                },
            );
            match case {
                "success" => {
                    let mut entries = result.unwrap();
                    assert_eq!(entries.len(), 1);
                    assert_eq!(entries.remove(0).unwrap(), PathBuf::from(r"C:\registry\AB"));
                    assert_eq!(calls.get(), 2);
                }
                "malformed" => {
                    let error = result.unwrap_err();
                    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                    assert_eq!(error.to_string(), "invalid native directory entry");
                    assert_eq!(calls.get(), 1);
                }
                _ => {
                    let mut entries = result.unwrap();
                    assert_eq!(entries.len(), 1);
                    assert_eq!(entries.remove(0).unwrap_err().raw_os_error(), Some(5));
                    assert_eq!(calls.get(), 1);
                }
            }
        }
    }

    #[test]
    fn november_directory_name_extent_models_32_bit_overflow_without_large_native_records() {
        assert_eq!(directory_name_end(68, 4, 72).unwrap(), 72);
        for (start, length, maximum) in [
            (68, 4, 71),
            (68, u32::MAX as usize - 1, u32::MAX as usize),
            (usize::MAX, 1, usize::MAX),
        ] {
            let error = directory_name_end(start, length, maximum).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(error.to_string(), "invalid native directory entry");
        }
    }

    #[test]
    fn november_directory_creation_preserves_parent_open_and_validation_failures_with_owned_handles() {
        for case in ["parent", "open", "verify", "success"] {
            let releases = Cell::new(0);
            let result = make_directory_with(
                Path::new(r"C:\new"),
                |parent| {
                    assert_eq!(parent, Path::new(r"C:\"));
                    if case == "parent" {
                        Err(io::Error::other("parent refused"))
                    } else {
                        Ok(DropCount(&releases))
                    }
                },
                |_, name, access, disposition, options| {
                    assert_eq!(releases.get(), 0);
                    assert_eq!(name, "new");
                    assert_eq!((access, disposition, options), (0x0002_0081, 3, 1));
                    if case == "open" {
                        Err(io::Error::other("open refused"))
                    } else {
                        Ok(DropCount(&releases))
                    }
                },
                |_| {
                    assert_eq!(releases.get(), 0);
                    if case == "verify" {
                        Err(io::Error::other("verify refused"))
                    } else {
                        Ok(())
                    }
                },
            );
            if case == "success" {
                let file = result.unwrap();
                assert_eq!(releases.get(), 1);
                drop(file);
                assert_eq!(releases.get(), 2);
            } else {
                assert_eq!(result.unwrap_err().to_string(), format!("{case} refused"));
                assert_eq!(
                    releases.get(),
                    match case {
                        "parent" => 0,
                        "open" => 1,
                        _ => 2,
                    }
                );
            }
        }
    }

    #[test]
    fn november_handle_owner_preserves_query_and_sid_errors_and_frees_only_owned_descriptors() {
        for case in ["query-error", "sid-error", "success"] {
            let releases = Cell::new(0);
            let result = handle_owner_with(
                |owner, descriptor| {
                    *owner = ptr::dangling_mut();
                    *descriptor = ptr::dangling_mut();
                    if case == "query-error" { 5 } else { 0 }
                },
                |descriptor| {
                    assert_ne!(case, "query-error");
                    assert_eq!(descriptor, ptr::dangling_mut());
                    DropCount(&releases)
                },
                |owner| {
                    assert_ne!(case, "query-error");
                    assert_eq!(owner, ptr::dangling_mut());
                    assert_eq!(releases.get(), 0);
                    if case == "sid-error" {
                        Err(io::Error::other("SID refused"))
                    } else {
                        Ok("S-1-5-18".into())
                    }
                },
            );
            match case {
                "query-error" => assert_eq!(result.unwrap_err().raw_os_error(), Some(5)),
                "sid-error" => assert_eq!(result.unwrap_err().to_string(), "SID refused"),
                _ => assert_eq!(result.unwrap(), MonitorOwner::WindowsSid("S-1-5-18".into())),
            }
            assert_eq!(releases.get(), usize::from(case != "query-error"));
        }
    }

    #[derive(Debug)]
    struct DropCount<'a>(&'a Cell<usize>);

    impl Drop for DropCount<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn november_security_attributes_release_their_owned_descriptor_exactly_once() {
        let releases = Arc::new(AtomicUsize::new(0));
        let attributes = Attributes {
            descriptor: LocalAllocation::Injected(Arc::clone(&releases)),
        };
        assert_eq!(releases.load(Ordering::Relaxed), 0);
        assert!(attributes.native().lpSecurityDescriptor.is_null());
        assert_eq!(releases.load(Ordering::Relaxed), 0);
        drop(attributes);
        assert_eq!(releases.load(Ordering::Relaxed), 1);
    }

    #[derive(Debug)]
    struct OwnedTestResource<'a> {
        name: &'static str,
        dropped: &'a RefCell<Vec<&'static str>>,
    }

    impl Drop for OwnedTestResource<'_> {
        fn drop(&mut self) {
            self.dropped.borrow_mut().push(self.name);
        }
    }

    #[test]
    fn november_descriptor_creation_keeps_attributes_live_and_releases_partial_ownership_on_error() {
        let owner = MonitorOwner::WindowsSid("S-1-5-21".into());
        for case in ["parent", "directory", "attributes", "name", "create", "success"] {
            let dropped = RefCell::new(Vec::new());
            let calls = RefCell::new(Vec::new());
            let path = match case {
                "parent" => Path::new(""),
                "name" => Path::new(r"C:\registry\.."),
                _ => Path::new(r"C:\registry\descriptor"),
            };
            let result = create_descriptor_with(
                path,
                &owner,
                |parent, actual_owner| {
                    calls.borrow_mut().push("directory");
                    assert_eq!(parent, Path::new(r"C:\registry"));
                    assert_eq!(actual_owner, &owner);
                    if case == "directory" {
                        Err(io::Error::other("directory denied"))
                    } else {
                        Ok(OwnedTestResource {
                            name: "directory",
                            dropped: &dropped,
                        })
                    }
                },
                |actual_owner| {
                    calls.borrow_mut().push("attributes");
                    assert_eq!(actual_owner, &owner);
                    assert!(dropped.borrow().is_empty());
                    if case == "attributes" {
                        Err(io::Error::other("attributes denied"))
                    } else {
                        Ok(OwnedTestResource {
                            name: "attributes",
                            dropped: &dropped,
                        })
                    }
                },
                |directory, name, attributes| {
                    calls.borrow_mut().push("create");
                    assert_eq!(directory.name, "directory");
                    assert_eq!(name, OsStr::new("descriptor"));
                    assert_eq!(attributes.unwrap().name, "attributes");
                    assert!(dropped.borrow().is_empty());
                    if case == "create" {
                        Err(io::Error::other("creation denied"))
                    } else {
                        Ok(OwnedTestResource {
                            name: "file",
                            dropped: &dropped,
                        })
                    }
                },
            );
            if case == "success" {
                assert_eq!(*dropped.borrow(), ["attributes", "directory"]);
                let file = result.unwrap();
                assert_eq!(file.name, "file");
                drop(file);
                assert_eq!(*dropped.borrow(), ["attributes", "directory", "file"]);
            } else {
                let error = result.unwrap_err();
                assert_eq!(
                    error.to_string(),
                    match case {
                        "parent" => "descriptor needs a directory",
                        "directory" => "directory denied",
                        "attributes" => "attributes denied",
                        "name" => "descriptor needs a file name",
                        _ => "creation denied",
                    }
                );
                assert_eq!(
                    *dropped.borrow(),
                    match case {
                        "parent" | "directory" => vec![],
                        "attributes" => vec!["directory"],
                        _ => vec!["attributes", "directory"],
                    }
                );
            }
            assert_eq!(
                *calls.borrow(),
                match case {
                    "parent" => vec![],
                    "directory" => vec!["directory"],
                    "attributes" | "name" => vec!["directory", "attributes"],
                    _ => vec!["directory", "attributes", "create"],
                }
            );
        }
    }

    #[test]
    fn november_descriptor_read_admits_only_ordinary_files_and_verifies_both_retained_handles() {
        for case in [
            "parent",
            "directory",
            "name",
            "open",
            "metadata",
            "directory-bit",
            "reparse-bit",
            "both-bits",
            "owner",
            "verify-directory",
            "verify-file",
            "success",
        ] {
            let dropped = RefCell::new(Vec::new());
            let calls = RefCell::new(Vec::new());
            let owner = MonitorOwner::WindowsSid("S-1-5-21".into());
            let path = match case {
                "parent" => Path::new(""),
                "name" => Path::new(r"C:\registry\.."),
                _ => Path::new(r"C:\registry\descriptor"),
            };
            let result = read_descriptor_with(
                path,
                |parent| {
                    calls.borrow_mut().push("directory");
                    assert_eq!(parent, Path::new(r"C:\registry"));
                    if case == "directory" {
                        Err(io::Error::other("directory denied"))
                    } else {
                        Ok(OwnedTestResource {
                            name: "directory",
                            dropped: &dropped,
                        })
                    }
                },
                |directory, name| {
                    calls.borrow_mut().push("open");
                    assert_eq!(directory.name, "directory");
                    assert_eq!(name, OsStr::new("descriptor"));
                    if case == "open" {
                        Err(io::Error::other("open denied"))
                    } else {
                        Ok(OwnedTestResource {
                            name: "file",
                            dropped: &dropped,
                        })
                    }
                },
                |file| {
                    calls.borrow_mut().push("metadata");
                    assert_eq!(file.name, "file");
                    match case {
                        "metadata" => Err(io::Error::other("metadata denied")),
                        "directory-bit" => Ok(FILE_ATTRIBUTE_DIRECTORY),
                        "reparse-bit" => Ok(FILE_ATTRIBUTE_REPARSE_POINT),
                        "both-bits" => Ok(FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT),
                        _ => Ok(0),
                    }
                },
                |file| {
                    calls.borrow_mut().push("owner");
                    assert_eq!(file.name, "file");
                    if case == "owner" {
                        Err(io::Error::other("owner denied"))
                    } else {
                        Ok(owner.clone())
                    }
                },
                |file, actual_owner| {
                    calls.borrow_mut().push(file.name);
                    assert_eq!(actual_owner, &owner);
                    assert!(dropped.borrow().is_empty());
                    if (case == "verify-directory" && file.name == "directory") || (case == "verify-file" && file.name == "file") {
                        Err(io::Error::other("verification denied"))
                    } else {
                        Ok(MonitorOwner::WindowsSid(SYSTEM.into()))
                    }
                },
            );
            if case == "success" {
                let (file, actual_owner) = result.unwrap();
                assert_eq!(actual_owner, owner);
                assert_eq!(*dropped.borrow(), ["directory"]);
                assert_eq!(*calls.borrow(), ["directory", "open", "metadata", "owner", "directory", "file"]);
                drop(file);
                assert_eq!(*dropped.borrow(), ["directory", "file"]);
            } else {
                assert_descriptor_read_failure(case, &result.unwrap_err(), &dropped.borrow(), calls.borrow().len());
            }
        }
    }

    fn assert_descriptor_read_failure(case: &str, error: &io::Error, dropped: &[&str], calls: usize) {
        assert_eq!(
            error.to_string(),
            match case {
                "parent" => "descriptor needs a directory",
                "directory" => "directory denied",
                "name" => "descriptor needs a file name",
                "open" => "open denied",
                "metadata" => "metadata denied",
                "directory-bit" | "reparse-bit" | "both-bits" => "monitor descriptor is not an ordinary file",
                "owner" => "owner denied",
                _ => "verification denied",
            }
        );
        let expected_drops: &[&str] = match case {
            "parent" | "directory" => &[],
            "name" | "open" => &["directory"],
            _ => &["file", "directory"],
        };
        assert_eq!(dropped, expected_drops);
        assert_eq!(
            calls,
            match case {
                "parent" => 0,
                "directory" | "name" => 1,
                "open" => 2,
                "metadata" | "directory-bit" | "reparse-bit" | "both-bits" => 3,
                "owner" => 4,
                "verify-directory" => 5,
                _ => 6,
            }
        );
    }

    #[test]
    fn november_handle_admission_rejects_both_invalid_sentinels_before_taking_ownership() {
        let valid = ptr::dangling_mut::<u8>().cast();
        for handle in [ptr::null_mut(), INVALID_HANDLE_VALUE, valid] {
            let errors = Cell::new(0);
            let owners = Cell::new(0);
            let result = owned_handle_with(
                handle,
                || {
                    errors.set(errors.get() + 1);
                    io::Error::from_raw_os_error(6)
                },
                |actual| {
                    assert_eq!(actual, valid);
                    owners.set(owners.get() + 1);
                    7_u8
                },
            );
            if handle == valid {
                assert_eq!(result.unwrap(), 7);
                assert_eq!(owners.get(), 1);
                assert_eq!(errors.get(), 0);
            } else {
                assert_eq!(result.unwrap_err().raw_os_error(), Some(6));
                assert_eq!(owners.get(), 0);
                assert_eq!(errors.get(), 1);
            }
        }
    }

    #[test]
    fn november_native_open_passes_exact_access_sharing_flags_names_and_status_ownership() {
        for options in [0, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE] {
            for (access, expected_access) in [(0, 0x0010_0000), (GENERIC_READ, 0x8010_0000)] {
                for status in [i32::MIN, -1, 0, 1] {
                    let owners = Cell::new(0);
                    let errors = Cell::new(0);
                    let root: HANDLE = ptr::dangling_mut::<u8>().cast();
                    let security = if options == FILE_DIRECTORY_FILE {
                        root.cast_const()
                    } else {
                        ptr::null()
                    };
                    let handle = ptr::dangling_mut::<u16>().cast();
                    let result = open_native_with(
                        NativeOpenRequest {
                            directory: root,
                            name: OsStr::new("Δ.dat"),
                            access,
                            disposition: FILE_CREATE,
                            options,
                            security,
                        },
                        |request, output, _| {
                            assert_eq!(request.access, expected_access);
                            assert_eq!(request.disposition, FILE_CREATE);
                            assert_eq!(request.options, options | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT);
                            assert_eq!(request.share, if options == FILE_DIRECTORY_FILE { 3 } else { 1 });
                            assert_eq!(request.file_attributes, 0);
                            assert_eq!(request.ea_length, 0);
                            assert!(request.ea.is_null());
                            assert!(request.allocation_size.is_null());
                            let attributes = request.attributes;
                            assert_eq!(attributes.Length as usize, std::mem::size_of::<OBJECT_ATTRIBUTES>());
                            assert_eq!(attributes.RootDirectory, root);
                            assert_eq!(attributes.Attributes, 0x1040);
                            assert_eq!(attributes.SecurityDescriptor.cast::<c_void>(), security);
                            assert!(attributes.SecurityQualityOfService.is_null());
                            // SAFETY: the helper retains the UNICODE_STRING and its complete terminated buffer during this callback.
                            let name = unsafe { &*attributes.ObjectName };
                            assert_eq!(name.Length, 10);
                            assert_eq!(name.MaximumLength, 10);
                            // SAFETY: the five advertised code units and trailing terminator are owned by the helper.
                            assert_eq!(unsafe { std::slice::from_raw_parts(name.Buffer, 6) }, [0x394, 46, 100, 97, 116, 0]);
                            *output = handle;
                            status
                        },
                        |actual| {
                            assert_eq!(actual, status);
                            errors.set(errors.get() + 1);
                            13
                        },
                        |actual| {
                            assert_eq!(actual, handle);
                            owners.set(owners.get() + 1);
                            Ok(42_u8)
                        },
                    );
                    if status < 0 {
                        assert_eq!(result.unwrap_err().raw_os_error(), Some(13));
                        assert_eq!(errors.get(), 1);
                        assert_eq!(owners.get(), 0);
                    } else {
                        assert_eq!(result.unwrap(), 42);
                        assert_eq!(errors.get(), 0);
                        assert_eq!(owners.get(), 1);
                    }
                }
            }
        }
    }

    #[test]
    fn november_native_open_rejects_invalid_names_without_entering_the_native_adapter() {
        for name in [OsString::from("a\0b"), OsString::from("a".repeat(32_768))] {
            let error = open_native_with::<u8>(
                NativeOpenRequest {
                    directory: ptr::null_mut(),
                    name: &name,
                    access: 0,
                    disposition: 0,
                    options: 0,
                    security: ptr::null(),
                },
                |_, _, _| panic!("invalid names must not reach NtCreateFile"),
                |_| panic!("invalid names must not translate a native status"),
                |_| panic!("invalid names must not acquire a native handle"),
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                error.to_string(),
                if name.len() == 3 {
                    "native name contains a null character"
                } else {
                    "native file name is too long"
                }
            );
        }
    }

    #[test]
    fn november_sid_conversion_scans_every_code_unit_and_frees_after_success_or_decode_error() {
        for (units, expected) in [
            (vec![0], Some("")),
            ("S-1-5-21".encode_utf16().chain([0]).collect(), Some("S-1-5-21")),
            (vec![0xd83d, 0xdd12, 0], Some("🔒")),
            (vec![0xd800, 0], None),
            (vec![0xdc00, 0], None),
        ] {
            let dropped = Cell::new(0);
            let reads = RefCell::new(Vec::new());
            let text = units.as_ptr().cast_mut();
            let result = sid_string_with(
                |output| {
                    *output = text;
                    1
                },
                || panic!("successful conversion must not query an OS error"),
                |pointer| {
                    assert_eq!(pointer, text);
                    DropCount(&dropped)
                },
                |pointer, offset| {
                    assert_eq!(pointer, text);
                    assert_eq!(dropped.get(), 0);
                    reads.borrow_mut().push(offset);
                    units[offset]
                },
                |pointer, length| {
                    assert_eq!(pointer, text);
                    assert_eq!(length, units.len() - 1);
                    assert_eq!(dropped.get(), 0);
                    String::from_utf16(&units[..length])
                },
            );
            if let Some(expected) = expected {
                assert_eq!(result.unwrap(), expected);
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                assert_eq!(error.to_string(), "OS SID is not valid UTF-16");
            }
            assert_eq!(dropped.get(), 1);
            assert_eq!(*reads.borrow(), (0..units.len()).collect::<Vec<_>>());
        }
        let error = sid_string_with(
            |_| 0,
            || io::Error::from_raw_os_error(1722),
            |_| panic!("failed conversion must not acquire an allocation"),
            |_, _| panic!("failed conversion must not read text"),
            |_, _| panic!("failed conversion must not decode text"),
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(1722));
    }

    #[test]
    fn november_process_owner_queries_size_before_storage_and_preserves_every_failure() {
        let size = u32::try_from(std::mem::size_of::<TOKEN_USER>()).unwrap();
        for case in ["token", "zero", "short", "query", "sid", "exact", "rounded"] {
            let dropped = Cell::new(0);
            let queries = Cell::new(0);
            let expected_sid = ptr::dangling_mut::<u8>().cast();
            let result = process_owner_with(
                || {
                    if case == "token" {
                        Err(io::Error::other("token denied"))
                    } else {
                        Ok(DropCount(&dropped))
                    }
                },
                |_, storage, capacity, length| {
                    assert_eq!(dropped.get(), 0);
                    queries.set(queries.get() + 1);
                    if queries.get() == 1 {
                        assert!(storage.is_null());
                        assert_eq!(capacity, 0);
                        *length = match case {
                            "zero" => 0,
                            "short" => size - 1,
                            "rounded" => size + 1,
                            _ => size,
                        };
                        return 0;
                    }
                    assert_eq!(queries.get(), 2);
                    assert_eq!(capacity, if case == "rounded" { size + 1 } else { size });
                    assert!(!storage.is_null());
                    assert_eq!(storage.addr() % std::mem::align_of::<TOKEN_USER>(), 0);
                    if case == "query" {
                        return 0;
                    }
                    // SAFETY: the helper provided aligned, zero-initialized storage large enough for TOKEN_USER.
                    unsafe { (*storage.cast::<TOKEN_USER>()).User.Sid = expected_sid };
                    1
                },
                || io::Error::from_raw_os_error(1300),
                |sid| {
                    assert_eq!(sid, expected_sid);
                    assert_eq!(dropped.get(), 0);
                    if case == "sid" {
                        Err(io::Error::other("SID conversion denied"))
                    } else {
                        Ok("S-1-5-21".into())
                    }
                },
            );
            match case {
                "token" => assert_eq!(result.unwrap_err().to_string(), "token denied"),
                "zero" | "short" | "query" => assert_eq!(result.unwrap_err().raw_os_error(), Some(1300)),
                "sid" => assert_eq!(result.unwrap_err().to_string(), "SID conversion denied"),
                _ => assert_eq!(result.unwrap(), MonitorOwner::WindowsSid("S-1-5-21".into())),
            }
            assert_eq!(
                queries.get(),
                if case == "token" {
                    0
                } else if matches!(case, "zero" | "short") {
                    1
                } else {
                    2
                }
            );
            assert_eq!(dropped.get(), usize::from(case != "token"));
        }
    }

    #[test]
    fn november_elevation_queries_preserve_token_lifetime_errors_and_nonzero_boolean_values() {
        for (token_ok, query_ok, value) in [
            (false, true, 0),
            (true, false, 1),
            (true, true, 0),
            (true, true, 1),
            (true, true, 2),
            (true, true, u32::MAX),
        ] {
            let dropped = Cell::new(0);
            let calls = Cell::new(0);
            let result = is_elevated_with(
                || {
                    if token_ok {
                        Ok(DropCount(&dropped))
                    } else {
                        Err(io::Error::other("elevation token denied"))
                    }
                },
                |_, elevation, capacity, length| {
                    calls.set(calls.get() + 1);
                    assert_eq!(dropped.get(), 0);
                    assert_eq!(capacity as usize, std::mem::size_of::<TOKEN_ELEVATION>());
                    elevation.TokenIsElevated = value;
                    *length = capacity;
                    i32::from(query_ok)
                },
                || io::Error::from_raw_os_error(1008),
            );
            if !token_ok {
                assert_eq!(result.unwrap_err().to_string(), "elevation token denied");
            } else if !query_ok {
                assert_eq!(result.unwrap_err().raw_os_error(), Some(1008));
            } else {
                assert_eq!(result.unwrap(), value != 0);
            }
            assert_eq!(calls.get(), usize::from(token_ok));
            assert_eq!(dropped.get(), usize::from(token_ok));
        }
    }

    fn access_entries(case: &str) -> [ACCESS_ALLOWED_ACE; 2] {
        let size = u16::try_from(std::mem::size_of::<ACCESS_ALLOWED_ACE>()).unwrap();
        [
            ACCESS_ALLOWED_ACE {
                Header: ACE_HEADER {
                    AceType: u8::from(case == "type"),
                    AceFlags: if case == "inherit-only" {
                        u8::try_from(INHERIT_ONLY_ACE).unwrap()
                    } else {
                        0
                    },
                    AceSize: if case == "size" { size - 1 } else { size },
                },
                Mask: 0x1234,
                SidStart: 17,
            },
            ACCESS_ALLOWED_ACE {
                Header: ACE_HEADER {
                    AceType: 0,
                    AceFlags: 0,
                    AceSize: size,
                },
                Mask: 0xabcd,
                SidStart: 19,
            },
        ]
    }

    #[test]
    fn november_native_access_validates_owned_descriptors_and_each_ace_before_extracting_grants() {
        for case in [
            "query",
            "descriptor",
            "owner",
            "owner-sid",
            "acl",
            "ace-query",
            "entry",
            "type",
            "size",
            "inherit-only",
            "entry-sid",
            "success",
        ] {
            let dropped = Cell::new(0);
            let sid_calls = Cell::new(0);
            let indices = RefCell::new(Vec::new());
            let mut acl = ACL {
                AceCount: 2,
                ..ACL::default()
            };
            let mut entries = access_entries(case);
            let entry_pointers = [(&raw mut entries[0]).cast(), (&raw mut entries[1]).cast()];
            let sid_pointers = [(&raw mut entries[0].SidStart).cast(), (&raw mut entries[1].SidStart).cast()];
            let acl_pointer = &raw mut acl;
            let owner_pointer = ptr::dangling_mut::<u8>().cast();
            let descriptor_pointer = ptr::dangling_mut::<u8>().cast();
            let result = native_access_with(
                |owner, acl, descriptor| {
                    *owner = if case == "owner" { ptr::null_mut() } else { owner_pointer };
                    *acl = if case == "acl" { ptr::null_mut() } else { acl_pointer };
                    *descriptor = if case == "descriptor" {
                        ptr::null_mut()
                    } else {
                        descriptor_pointer
                    };
                    u32::from(case == "query") * 5
                },
                |_| DropCount(&dropped),
                |sid| {
                    assert_eq!(dropped.get(), 0);
                    let call = sid_calls.get();
                    sid_calls.set(call + 1);
                    assert_eq!(sid, if call == 0 { owner_pointer } else { sid_pointers[call - 1] });
                    if (case == "owner-sid" && call == 0) || (case == "entry-sid" && call == 1) {
                        Err(io::Error::other("SID denied"))
                    } else {
                        Ok(if call == 0 {
                            "owner"
                        } else if call == 1 {
                            "user-one"
                        } else {
                            "user-two"
                        }
                        .into())
                    }
                },
                |acl, index| {
                    assert_eq!(acl, acl_pointer);
                    assert_eq!(dropped.get(), 0);
                    assert_eq!(index as usize, indices.borrow().len());
                    indices.borrow_mut().push(index);
                    match case {
                        "ace-query" => Err(io::Error::other("ACE query denied")),
                        "entry" => Ok(ptr::null_mut()),
                        _ => Ok(entry_pointers[index as usize]),
                    }
                },
            );
            if case == "success" {
                let access = result.unwrap();
                assert_eq!(access.owner, "owner");
                assert_eq!(access.grants, [("user-one".into(), 0x1234), ("user-two".into(), 0xabcd)]);
                assert_eq!(*indices.borrow(), [0, 1]);
                assert_eq!(sid_calls.get(), 3);
            } else {
                let Err(error) = result else {
                    panic!("fault {case} must return its error")
                };
                match case {
                    "query" => assert_eq!(error.raw_os_error(), Some(5)),
                    "descriptor" | "owner" => assert_eq!(error.to_string(), "monitor registry security descriptor is incomplete"),
                    "owner-sid" | "entry-sid" => assert_eq!(error.to_string(), "SID denied"),
                    "acl" => assert_eq!(error.to_string(), "monitor registry must have an access-control list"),
                    "ace-query" => assert_eq!(error.to_string(), "ACE query denied"),
                    "entry" => assert_eq!(error.to_string(), "monitor registry ACL contains a null entry"),
                    _ => assert_eq!(error.to_string(), "monitor registry ACL contains an unexpected entry"),
                }
            }
            assert_eq!(dropped.get(), usize::from(case != "query"));
        }
        let error = verify_access_with(
            || Err(io::Error::other("security query denied")),
            &MonitorOwner::WindowsSid("S-1-5-21".into()),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "security query denied");
    }

    #[test]
    fn gamma_round2_directory_metadata_rejects_reparse_points_and_preserves_failures() {
        ordinary_directory_with_attributes(|| Ok(FILE_ATTRIBUTE_DIRECTORY)).unwrap();
        for attributes in [
            0,
            FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT,
        ] {
            let error = ordinary_directory_with_attributes(|| Ok(attributes)).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(error.to_string(), "monitor directory is not an ordinary directory");
        }
        let error = ordinary_directory_with_attributes(|| Err(io::Error::other("round2 metadata refused"))).unwrap_err();
        assert_eq!(error.to_string(), "round2 metadata refused");
    }

    #[test]
    fn gamma_round2_dos_mapping_decode_respects_reported_length_and_utf16() {
        let text = r"\Device\HarddiskVolume12";
        let short = text.encode_utf16().chain([0]).collect::<Vec<_>>();
        assert_eq!(
            dos_device_mapping_path(&short, short.len()).unwrap(),
            Path::new(r"\Device\HarddiskVolume12\")
        );
        let mut full = vec![u16::from(b'x'); 32_768];
        full[..short.len()].copy_from_slice(&short);
        *full.last_mut().unwrap() = 0;
        assert_eq!(
            dos_device_mapping_path(&full, full.len()).unwrap(),
            Path::new(r"\Device\HarddiskVolume12\")
        );
        let error = dos_device_mapping_path(&short, short.len() - 1).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "DOS device mapping is unterminated");
        assert_eq!(
            dos_device_mapping_path(&[0xd800, 0], 2).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn gamma_round2_acl_entry_header_requires_allowed_type_full_size_and_effective_grant() {
        let size = u16::try_from(std::mem::size_of::<ACCESS_ALLOWED_ACE>()).unwrap();
        for (kind, flags, size, accepted) in [
            (0, 0, size, true),
            (0, 0x10, size, true),
            (1, 0, size, false),
            (0, 0, size - 1, false),
            (0, u8::try_from(INHERIT_ONLY_ACE).unwrap(), size, false),
        ] {
            let result = validate_access_entry_header(ACE_HEADER {
                AceType: kind,
                AceFlags: flags,
                AceSize: size,
            });
            if accepted {
                result.unwrap();
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                assert_eq!(error.to_string(), "monitor registry ACL contains an unexpected entry");
            }
        }
    }

    #[test]
    fn gamma_round2_native_attributes_do_not_inherit_handles() {
        let attributes = std::mem::ManuallyDrop::new(Attributes {
            descriptor: LocalAllocation::Native(ptr::null_mut()),
        });
        let native = attributes.native();
        assert_eq!(native.nLength as usize, std::mem::size_of::<SECURITY_ATTRIBUTES>());
        assert!(native.lpSecurityDescriptor.is_null());
        assert_eq!(native.bInheritHandle, 0);
    }

    #[test]
    fn gamma_round2_native_name_validation_returns_errors_before_opening_handles() {
        let error = open_native(None, OsStr::new("x\0y"), 0, 0, 0, None).unwrap_err();
        assert_eq!(
            (error.kind(), error.to_string()),
            (io::ErrorKind::InvalidInput, "native name contains a null character".into())
        );
        let name = "x".repeat(32_768);
        let error = open_native(None, OsStr::new(&name), 0, 0, 0, None).unwrap_err();
        assert_eq!(
            (error.kind(), error.to_string()),
            (io::ErrorKind::InvalidInput, "native file name is too long".into())
        );
        let attributes = std::mem::ManuallyDrop::new(Attributes {
            descriptor: LocalAllocation::Native(ptr::null_mut()),
        });
        assert_eq!(
            make_directory(Path::new(""), &attributes).unwrap_err().to_string(),
            "monitor registry needs a parent directory"
        );
        assert_eq!(
            make_directory(Path::new("."), &attributes).unwrap_err().to_string(),
            "monitor registry needs a directory name"
        );
    }

    #[test]
    fn gamma_round2_directory_records_validate_empty_names_and_overlapping_offsets() {
        const NAME: usize = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileName);
        const LENGTH: usize = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileNameLength);
        let mut bytes = vec![0; NAME + 8];
        bytes[LENGTH..LENGTH + 4].copy_from_slice(&0_u32.to_ne_bytes());
        assert_eq!(directory_names(&bytes).unwrap_err().kind(), io::ErrorKind::InvalidData);
        bytes[LENGTH..LENGTH + 4].copy_from_slice(&2_u32.to_ne_bytes());
        bytes[NAME..NAME + 2].copy_from_slice(&u16::from(b'x').to_ne_bytes());
        assert_eq!(directory_names(&bytes).unwrap(), [OsString::from("x")]);
        bytes[..4].copy_from_slice(&8_u32.to_ne_bytes());
        assert_eq!(directory_names(&bytes).unwrap_err().kind(), io::ErrorKind::InvalidData);
        bytes[..4].fill(0);
        assert_eq!(directory_names(&bytes[..NAME + 2]).unwrap(), [OsString::from("x")]);
        for end in 0..NAME {
            assert_eq!(directory_names(&bytes[..end]).unwrap_err().kind(), io::ErrorKind::InvalidData);
        }
        let next = NAME & !7;
        let mut overlap = vec![0; next + NAME + 2];
        overlap[..4].copy_from_slice(&u32::try_from(next).unwrap().to_ne_bytes());
        overlap[LENGTH..LENGTH + 4].copy_from_slice(&2_u32.to_ne_bytes());
        overlap[NAME..NAME + 2].copy_from_slice(&u16::from(b'.').to_ne_bytes());
        overlap[next + LENGTH..next + LENGTH + 4].copy_from_slice(&2_u32.to_ne_bytes());
        overlap[next + NAME..].copy_from_slice(&u16::from(b'x').to_ne_bytes());
        assert_eq!(directory_names(&overlap[next..]).unwrap(), [OsString::from("x")]);
        assert_eq!(directory_names(&overlap).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn gamma_survivor_security_validation_reports_exact_pure_errors() {
        assert_eq!(
            owner_sid(&MonitorOwner::UnixUser(1)).unwrap_err().to_string(),
            "a Windows monitor requires a Windows user SID"
        );
        assert_eq!(
            Attributes::for_owner(&MonitorOwner::UnixUser(1)).unwrap_err().to_string(),
            "a Windows monitor requires a Windows user SID"
        );
        assert_eq!(
            Attributes::for_directory(&MonitorOwner::UnixUser(1)).unwrap_err().to_string(),
            "a Windows monitor requires a Windows user SID"
        );
        assert_eq!(
            Attributes::from_sddl("x\0y").unwrap_err().to_string(),
            "native name contains a null character"
        );
        assert_eq!(
            owner_sid(&MonitorOwner::WindowsSid("bad".into())).unwrap_err().to_string(),
            "invalid Windows monitor SID"
        );
        assert_eq!(
            wide(OsStr::new("x\0y")).unwrap_err().to_string(),
            "native name contains a null character"
        );
        assert_eq!(wide(OsStr::new("x")).unwrap(), [120, 0]);
        assert_eq!(
            local_volume_path(r"\Device\HarddiskVolumeX").unwrap_err().to_string(),
            "monitor registry requires a local disk volume"
        );
        assert_eq!(directory_names(&[]).unwrap_err().to_string(), "invalid native directory entry");
        assert_eq!(
            directory_names(&directory_row(&[])).unwrap_err().to_string(),
            "invalid native directory entry"
        );
        let expected = MonitorOwner::WindowsSid("S-1-5-21-1-2-3-1001".into());
        let trusted = || {
            vec![
                (expected.to_string(), GENERIC_ALL),
                (ADMINISTRATORS.into(), FILE_ALL_ACCESS),
                (SYSTEM.into(), GENERIC_ALL),
            ]
        };
        let access = NativeAccess {
            owner: "S-1-5-21-1-2-3-2002".into(),
            grants: trusted(),
        };
        assert_eq!(
            verify_grants(access, &expected).unwrap_err().to_string(),
            "monitor registry has an untrusted OS owner"
        );
        let mut grants = trusted();
        grants.push(("S-1-5-21-1-2-3-2002".into(), GENERIC_READ));
        let access = NativeAccess {
            owner: expected.to_string(),
            grants,
        };
        assert_eq!(
            verify_grants(access, &expected).unwrap_err().to_string(),
            "monitor registry ACL grants access to another user"
        );
        let access = NativeAccess {
            owner: expected.to_string(),
            grants: vec![(expected.to_string(), GENERIC_ALL)],
        };
        assert_eq!(
            verify_grants(access, &expected).unwrap_err().to_string(),
            "monitor registry ACL must grant its user and administrators full access"
        );
    }

    #[test]
    fn november_directory_header_fields_fit_before_the_smallest_nonempty_name() {
        let name = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileName);
        let length = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileNameLength);
        assert!(length + size_of::<u32>() < name);
        assert!(size_of::<u32>() < name);
        let row = directory_row(&[120]);
        assert_eq!(row.len(), name + size_of::<u16>());
        assert_eq!(directory_names(&row).unwrap(), [OsString::from("x")]);
        for end in 0..row.len() {
            let error = directory_names(&row[..end]).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "prefix {end}");
            assert_eq!(error.to_string(), "invalid native directory entry");
        }
        let error = directory_names(&directory_row(&[])).unwrap_err();
        assert_eq!(error.to_string(), "invalid native directory entry");
    }

    #[test]
    fn gamma_survivor_directory_record_offsets_reject_overlapping_next_rows() {
        let mut row = directory_row(&[120]);
        row[..4].copy_from_slice(&8_u32.to_ne_bytes());
        assert_eq!(directory_names(&row).unwrap_err().to_string(), "invalid native directory entry");
        let mut row = directory_row(&[120]);
        row[..4].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert_eq!(directory_names(&row).unwrap_err().to_string(), "invalid native directory entry");
    }

    #[test]
    fn gamma_survivor_directory_rows_keep_names_filter_dots_and_reject_truncated_or_component_names() {
        for units in [&[46_u16][..], &[46, 46][..]] {
            assert!(directory_names(&directory_row(units)).unwrap().is_empty());
        }
        for units in [&[120_u16, 0][..], &[120, 92, 121][..], &[92, 120][..]] {
            assert_eq!(
                directory_names(&directory_row(units)).unwrap_err().to_string(),
                "invalid native directory entry"
            );
        }
        let row = directory_row(&[120]);
        assert_eq!(
            directory_names(&row[..row.len() - 1]).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let aligned = (row.len() + 7) & !7;
        let mut missing_next = row.clone();
        missing_next[..4].copy_from_slice(&u32::try_from(aligned + 8).unwrap().to_ne_bytes());
        assert_eq!(directory_names(&missing_next).unwrap_err().kind(), io::ErrorKind::InvalidData);
        let mut rows = row;
        rows.resize(aligned, 0);
        rows[..4].copy_from_slice(&u32::try_from(aligned).unwrap().to_ne_bytes());
        rows.extend(directory_row(&[121]));
        assert_eq!(directory_names(&rows).unwrap(), [OsString::from("x"), OsString::from("y")]);
    }

    #[test]
    fn gamma_survivor_native_file_path_rejection_precedes_any_handle_open() {
        let owner = MonitorOwner::WindowsSid("S-1-5-21-1-2-3-1001".into());
        assert_eq!(
            create_descriptor(Path::new(""), &owner).unwrap_err().to_string(),
            "descriptor needs a directory"
        );
        assert_eq!(
            read_descriptor(Path::new("")).unwrap_err().to_string(),
            "descriptor needs a directory"
        );
        assert_eq!(
            directory_entries(Path::new("relative")).unwrap_err().to_string(),
            "monitor registry requires an absolute local disk path"
        );
        assert_eq!(
            create_directory(Path::new("unused"), &MonitorOwner::UnixUser(1))
                .unwrap_err()
                .to_string(),
            "a Windows monitor requires a Windows user SID"
        );
        for path in [r"\\server.invalid\share", r"\\?\UNC\server.invalid\share", "relative", "C:relative"] {
            assert_eq!(
                directory_file(Path::new(path)).unwrap_err().to_string(),
                "monitor registry requires an absolute local disk path"
            );
        }
    }

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
            LocalAllocation::Native(sid)
        });
        let disabled = sids.each_ref().map(|sid| SID_AND_ATTRIBUTES {
            Sid: sid.pointer(),
            Attributes: 0,
        });
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
    #[test]
    fn gamma_round2_native_security_pointer_checks_reject_each_missing_component_without_dereferencing() {
        let pointer = NonNull::<c_void>::dangling().as_ptr();
        validate_descriptor_presence(pointer, pointer).unwrap();
        for (descriptor, owner) in [
            (ptr::null_mut(), pointer),
            (pointer, ptr::null_mut()),
            (ptr::null_mut(), ptr::null_mut()),
        ] {
            let error = validate_descriptor_presence(descriptor, owner).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(error.to_string(), "monitor registry security descriptor is incomplete");
        }
        assert_eq!(
            checked_native_acl(ptr::null_mut()).unwrap_err().to_string(),
            "monitor registry must have an access-control list"
        );
        assert_eq!(
            checked_native_entry(ptr::null_mut()).unwrap_err().to_string(),
            "monitor registry ACL contains a null entry"
        );
        let acl = NonNull::<ACL>::dangling();
        assert_eq!(checked_native_acl(acl.as_ptr()).unwrap(), acl);
        assert_eq!(checked_native_entry(pointer).unwrap().as_ptr(), pointer);
    }
}
