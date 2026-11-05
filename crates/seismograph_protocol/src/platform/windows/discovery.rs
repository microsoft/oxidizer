// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::{io, ptr};

use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, HANDLE};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, KEY_WOW64_64KEY, REG_EXPAND_SZ, REG_SZ, RRF_NOEXPAND, RRF_RT_ANY, RegCloseKey,
    RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath};

use super::security::{create_directory, current_owner, is_elevated, open_directory, wide};
use crate::monitor::MonitorOwner;
use crate::platform::{DirectoryEntries, DirectoryEntry, RegistryDirectory};

const PROFILES: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList";
// Windows limits registry key names to 255 characters and paths to 32K UTF-16 units.
const KEY_NAME_UNITS: usize = 256;
const MAX_PATH_UNITS: usize = 32_768;

enum KnownFolder {
    Native(*mut u16),
    #[cfg(test)]
    Injected(Arc<AtomicUsize>),
}

impl Drop for KnownFolder {
    fn drop(&mut self) {
        match self {
            Self::Native(pointer) => {
                // SAFETY: SHGetKnownFolderPath allocated this string with CoTaskMemAlloc.
                unsafe { CoTaskMemFree(pointer.cast()) };
            }
            #[cfg(test)]
            Self::Injected(releases) => {
                releases.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

enum Key {
    Native(HKEY),
    #[cfg(test)]
    Injected {
        closes: Arc<AtomicUsize>,
        result: u32,
        errors: Arc<Mutex<Vec<i32>>>,
    },
}

impl Key {
    const fn handle(&self) -> HKEY {
        match self {
            Self::Native(handle) => *handle,
            #[cfg(test)]
            Self::Injected { .. } => ptr::null_mut(),
        }
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        let result = match self {
            Self::Native(handle) => {
                // SAFETY: this key was opened by RegOpenKeyExW and is owned here.
                unsafe { RegCloseKey(*handle) }
            }
            #[cfg(test)]
            Self::Injected { closes, result, .. } => {
                closes.fetch_add(1, Ordering::Relaxed);
                *result
            }
        };
        if result != 0 {
            match self {
                Self::Native(_) => eprintln!(
                    "seismograph: failed to close profile registry key: {}",
                    io::Error::from_raw_os_error(result.cast_signed())
                ),
                #[cfg(test)]
                Self::Injected { errors, .. } => errors.lock().unwrap().push(result.cast_signed()),
            }
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Known-folder lookup failures depend on Windows profile/COM configuration.
#[cfg_attr(test, mutants::skip)] // Native startup tests retain lookup; profile/path interpretation remains measured.
fn local_app_data() -> io::Result<PathBuf> {
    local_app_data_with(
        |flags, token, path| {
            // SAFETY: the folder ID is valid and path is a writable output pointer.
            unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, flags, token, path) }
        },
        KnownFolder::Native,
        |path, offset| {
            // SAFETY: the successful call supplied a terminated UTF-16 allocation.
            unsafe { *path.wrapping_add(offset) }
        },
        |path, length| {
            // SAFETY: the scan established the readable prefix while its owning allocation remains alive.
            PathBuf::from(OsString::from_wide(unsafe { std::slice::from_raw_parts(path, length) }))
        },
    )
}

fn local_app_data_with<A>(
    query: impl FnOnce(u32, HANDLE, &mut *mut u16) -> i32,
    own: impl FnOnce(*mut u16) -> A,
    mut read: impl FnMut(*mut u16, usize) -> u16,
    decode: impl FnOnce(*mut u16, usize) -> PathBuf,
) -> io::Result<PathBuf> {
    let mut path = ptr::null_mut();
    let result = query(0, ptr::null_mut(), &mut path);
    let _allocation = own(path);
    if result < 0 {
        return Err(io::Error::other(format!(
            "cannot locate local application data: HRESULT {result:#x}"
        )));
    }
    let mut length = 0;
    while read(path, length) != 0 {
        // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        length +=
        // #[gamma::skip(literal.int_decrement, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        1;
    }
    Ok(decode(path, length))
}

fn registry_path(base: &Path) -> PathBuf {
    base.join("Seismograph").join("monitor")
}

pub(crate) fn current_directory(owner: &MonitorOwner) -> io::Result<RegistryDirectory> {
    finish_current_directory(
        prepare_current_directory(owner, local_app_data, create_directory, open_directory),
        |path, file| RegistryDirectory {
            path,
            file,
            temporary: None,
        },
    )
}

fn finish_current_directory<F, D>(prepared: io::Result<(PathBuf, F)>, build: impl FnOnce(PathBuf, F) -> D) -> io::Result<D> {
    let (path, file) = prepared?;
    Ok(build(path, file))
}

fn prepare_current_directory<T>(
    owner: &MonitorOwner,
    base: impl FnOnce() -> io::Result<PathBuf>,
    mut create: impl FnMut(&Path, &MonitorOwner) -> io::Result<()>,
    mut open: impl FnMut(&Path, &MonitorOwner) -> io::Result<T>,
) -> io::Result<(PathBuf, T)> {
    let base = base()?.join("Seismograph");
    create(&base, owner)?;
    let _parent = open(&base, owner)?;
    let path = base.join("monitor");
    create(&path, owner)?;
    let file = open(&path, owner)?;
    Ok((path, file))
}

pub(crate) fn registry_directories() -> io::Result<DirectoryEntries> {
    discover_directories_with(current_owner, local_app_data, is_elevated, registry_directories_for)
}

fn discover_directories_with<D>(
    owner: impl FnOnce() -> io::Result<MonitorOwner>,
    base: impl FnOnce() -> io::Result<PathBuf>,
    elevated: impl FnOnce() -> io::Result<bool>,
    discover: impl FnOnce(MonitorOwner, io::Result<PathBuf>, bool) -> io::Result<D>,
) -> io::Result<D> {
    let owner = owner()?;
    discover(owner, base(), elevated()?)
}

#[cfg_attr(coverage_nightly, coverage(off))] // Registry enumeration failures and malformed live ProfileList keys require machine-wide setup.
#[cfg_attr(test, mutants::skip)] // Native enumeration tests remain; owner-aware deduplication and value/path validation stay measured.
// #[gamma::skip(parameter.default_shadow, reason = "native adapter: this bridge binds live registry key handles; registry_directories_with tests both elevation states, exact native requests, ownership, enumeration and error policies")]
fn registry_directories_for(owner: MonitorOwner, base: io::Result<PathBuf>, elevated: bool) -> io::Result<DirectoryEntries> {
    registry_directories_with(
        owner,
        base,
        elevated,
        |name, reserved, access| {
            let mut key = ptr::null_mut();
            // SAFETY: the predefined root and terminated name are valid; key is writable.
            let result = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, name.as_ptr(), reserved, access, &raw mut key) };
            (result, key)
        },
        Key::Native,
        |key, index, name, length| {
            // SAFETY: key is open, name/length are valid buffers, and optional outputs are null.
            unsafe {
                RegEnumKeyExW(
                    key.handle(),
                    index,
                    name.as_mut_ptr(),
                    length,
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            }
        },
        profile_local_app_data,
    )
}

fn registry_directories_with<K, O>(
    owner: MonitorOwner,
    base: io::Result<PathBuf>,
    elevated: bool,
    open: impl FnOnce(&[u16], u32, u32) -> (u32, K),
    claim: impl FnOnce(K) -> O,
    mut enumerate: impl FnMut(&O, u32, &mut [u16], &mut u32) -> u32,
    mut lookup: impl FnMut(&str) -> io::Result<PathBuf>,
) -> io::Result<DirectoryEntries> {
    let mut entries = vec![base.map(|base| DirectoryEntry {
        path: registry_path(&base),
        owner,
    })];
    if !elevated {
        return Ok(entries);
    }
    // #[gamma::skip(try.propagate_to_unwrap, reason = "PROFILES is a fixed NUL-free registry key, so its UTF-16 conversion cannot fail")]
    let name = wide(std::ffi::OsStr::new(PROFILES))?;
    let (result, key) = open(&name, 0, KEY_READ | KEY_WOW64_64KEY);
    if result != 0 {
        entries.push(Err(io::Error::from_raw_os_error(result.cast_signed())));
        return Ok(entries);
    }
    let key = claim(key);
    for index in profile_indexes() {
        // #[gamma::skip(literal.int_increment, reason = "RegEnumKeyExW initializes the returned prefix; only that prefix is read on success")]
        let mut name = [0_u16; KEY_NAME_UNITS];
        let mut length = u32::try_from(name.len()).expect("registry key buffer fits u32");
        let result = enumerate(&key, index, &mut name, &mut length);
        if result == ERROR_NO_MORE_ITEMS {
            // #[gamma::skip(loop.break_to_continue, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            break;
        }
        if result != 0 {
            entries.push(Err(io::Error::from_raw_os_error(result.cast_signed())));
            break;
        }
        let Ok(sid) = String::from_utf16(&name[..length as usize]) else {
            continue;
        };
        let Ok(owner @ MonitorOwner::WindowsSid(_)) = sid.parse::<MonitorOwner>() else {
            continue;
        };
        entries.push(lookup(&sid).map(|base| DirectoryEntry {
            path: registry_path(&base),
            owner,
        }));
    }
    // #[gamma::skip(call_result.default, reason = "deduplicate_entries returns unit and default_after preserves its sorting and filtering")]
    deduplicate_entries(&mut entries);
    Ok(entries)
}

fn profile_indexes() -> std::ops::Range<u32> {
    0..u32::MAX
}

fn deduplicate_entries(entries: &mut DirectoryEntries) {
    // A different user's claim must not hide the actual owner's registry.
    let mut seen = std::collections::BTreeSet::new();
    entries.retain(|entry| match entry {
        Ok(entry) => seen.insert((entry.path.clone(), entry.owner.clone())),
        Err(_) => true,
    });
}

#[cfg_attr(coverage_nightly, coverage(off))] // Loaded user hives and profile registry lookup failures depend on other users' login state.
#[cfg_attr(test, mutants::skip)] // Native profile discovery retains the lookup; profile-relative resolution has direct tests.
fn profile_local_app_data(sid: &str) -> io::Result<PathBuf> {
    profile_local_app_data_with(sid, registry_string)
}

fn profile_local_app_data_with(sid: &str, mut lookup: impl FnMut(HKEY, &str, &str, bool) -> io::Result<PathBuf>) -> io::Result<PathBuf> {
    let profile = lookup(HKEY_LOCAL_MACHINE, &format!(r"{PROFILES}\{sid}"), "ProfileImagePath", true)?;
    let settings = format!(r"{sid}\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders");
    match lookup(HKEY_USERS, &settings, "Local AppData", false) {
        Ok(path) => profile_relative_path(&path, &profile),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(profile.join("AppData").join("Local")),
        Err(error) => Err(error),
    }
}

fn profile_relative_path(path: &Path, profile: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_owned());
    }
    let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let prefix = "%USERPROFILE%";
    if units.len() > prefix.len()
        && String::from_utf16(&units[..prefix.len()]).is_ok_and(|text| text.eq_ignore_ascii_case(prefix))
        && matches!(units[prefix.len()], 47 | 92)
    {
        return Ok(profile.join(OsString::from_wide(&units[prefix.len() + 1..])));
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "local application data must be absolute or relative to %USERPROFILE%",
    ))
}

#[cfg_attr(coverage_nightly, coverage(off))] // RegGetValueW errors depend on live registry permissions and hive state.
#[cfg_attr(test, mutants::skip)] // Registry byte/type/termination validation is tested independently.
// #[gamma::skip(parameter.default_shadow, reason = "native adapter: RegGetValueW queries a live predefined registry root; registry_string_with tests both expansion modes and exact flags, sizes, UTF-16 values and errors")]
fn registry_string(root: HKEY, key: &str, value: &str, expand: bool) -> io::Result<PathBuf> {
    registry_string_with(key, value, expand, |key, value, flags, kind, data, bytes| {
        #[cfg(test)]
        if let Some(query) = TEST_REGISTRY_QUERY.get() {
            return query(key, value, flags, kind, data, bytes);
        }
        // SAFETY: predefined root, terminated names, and output buffers are valid.
        unsafe { RegGetValueW(root, key.as_ptr(), value.as_ptr(), flags, kind, data.as_mut_ptr().cast(), bytes) }
    })
}

#[cfg(test)]
type TestRegistryQuery = fn(&[u16], &[u16], u32, &mut u32, &mut [u16], &mut u32) -> u32;

#[cfg(test)]
thread_local! {
    static TEST_REGISTRY_QUERY: std::cell::Cell<Option<TestRegistryQuery>> = const { std::cell::Cell::new(None) };
}

fn registry_string_with(
    key: &str,
    value: &str,
    expand: bool,
    query: impl FnOnce(&[u16], &[u16], u32, &mut u32, &mut [u16], &mut u32) -> u32,
) -> io::Result<PathBuf> {
    let key = wide(std::ffi::OsStr::new(key))?;
    let value = wide(std::ffi::OsStr::new(value))?;
    let mut data = vec![0_u16; MAX_PATH_UNITS];
    let mut bytes = u32::try_from(data.len() * size_of::<u16>()).expect("bounded registry path buffer fits u32");
    // #[gamma::skip(literal.int_increment, reason = "the successful registry query overwrites kind; errors return before it is read")]
    let mut kind = 0;
    // Do not expand another user's USERPROFILE against the administrator's environment.
    let flags = RRF_RT_ANY
        | if expand {
            // #[gamma::skip(literal.int_increment, reason = "RRF_RT_ANY already includes bit zero, so OR-ing either zero or one in this arm gives identical registry query flags")]
            0
        } else {
            RRF_NOEXPAND
        };
    let result = query(&key, &value, flags, &mut kind, &mut data, &mut bytes);
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result.cast_signed()));
    }
    registry_value_path(kind, bytes as usize, &data, expand)
}

fn registry_value_path(kind: u32, bytes: usize, data: &[u16], expand: bool) -> io::Result<PathBuf> {
    if !matches!(kind, REG_SZ | REG_EXPAND_SZ) || !bytes.is_multiple_of(2) || bytes > data.len() * 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "user profile path is not a bounded UTF-16 string",
        ));
    }
    let units = &data[..bytes / 2];
    let length = units
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "user profile path is unterminated"))?;
    let path = PathBuf::from(OsString::from_wide(&units[..length]));
    if expand && !path.is_absolute() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "user profile path must be absolute"));
    }
    Ok(path)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[test]
    fn nov6_registry_adapter_preserves_both_expansion_modes_without_registry_io() {
        thread_local! {
            static EXPECTED_FLAGS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        }
        struct Restore(Option<TestRegistryQuery>);
        impl Drop for Restore {
            fn drop(&mut self) {
                TEST_REGISTRY_QUERY.set(self.0);
            }
        }
        let _restore = Restore(TEST_REGISTRY_QUERY.replace(Some(|key, value, flags, kind, data, bytes| {
            assert_eq!(key.last(), Some(&0));
            assert_eq!(value.last(), Some(&0));
            assert_eq!(flags, EXPECTED_FLAGS.get());
            let path: Vec<u16> = r"C:\Profiles\Alice".encode_utf16().chain([0]).collect();
            data[..path.len()].copy_from_slice(&path);
            *bytes = u32::try_from(path.len() * 2).unwrap();
            *kind = REG_SZ;
            0
        })));
        for expand in [false, true] {
            EXPECTED_FLAGS.set(RRF_RT_ANY | if expand { 0 } else { RRF_NOEXPAND });
            assert_eq!(
                registry_string(std::ptr::null_mut(), "key", "value", expand).unwrap(),
                Path::new(r"C:\Profiles\Alice")
            );
        }
    }

    #[test]
    fn november_current_directory_completion_preserves_preparation_error_and_moves_owned_resource_once() {
        let error = finish_current_directory(Err::<(PathBuf, ()), _>(io::Error::from_raw_os_error(5)), |_, ()| -> PathBuf {
            panic!("failed preparation must not construct a directory")
        })
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(5));
        let releases = Arc::new(AtomicUsize::new(0));
        let (path, owner) = finish_current_directory(
            Ok((PathBuf::from("prepared-path"), KnownFolder::Injected(Arc::clone(&releases)))),
            |path, owner| {
                assert_eq!(releases.load(Ordering::Relaxed), 0);
                assert_eq!(path, Path::new("prepared-path"));
                (path, owner)
            },
        )
        .unwrap();
        assert_eq!(path, Path::new("prepared-path"));
        assert_eq!(releases.load(Ordering::Relaxed), 0);
        drop(owner);
        assert_eq!(releases.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn november_registry_entry_routing_preserves_query_order_elevation_and_each_exact_error() {
        for elevated in [false, true] {
            for case in ["owner", "base", "elevation", "success"] {
                let events = std::cell::RefCell::new(Vec::new());
                let owner = MonitorOwner::WindowsSid("S-1-5-18".into());
                let result = discover_directories_with(
                    || {
                        events.borrow_mut().push("owner");
                        if case == "owner" {
                            Err(io::Error::from_raw_os_error(5))
                        } else {
                            Ok(owner.clone())
                        }
                    },
                    || {
                        events.borrow_mut().push("base");
                        if case == "base" {
                            Err(io::Error::from_raw_os_error(6))
                        } else {
                            Ok(PathBuf::from("profile"))
                        }
                    },
                    || {
                        events.borrow_mut().push("elevation");
                        if case == "elevation" {
                            Err(io::Error::from_raw_os_error(7))
                        } else {
                            Ok(elevated)
                        }
                    },
                    |actual_owner, base, actual_elevated| {
                        events.borrow_mut().push("discover");
                        assert_eq!(actual_owner, owner);
                        assert_eq!(actual_elevated, elevated);
                        Ok((actual_owner, base?, actual_elevated))
                    },
                );
                if case == "success" {
                    assert_eq!(result.unwrap(), (owner, PathBuf::from("profile"), elevated));
                } else {
                    assert_eq!(
                        result.unwrap_err().raw_os_error(),
                        Some(match case {
                            "owner" => 5,
                            "base" => 6,
                            _ => 7,
                        })
                    );
                }
                assert_eq!(
                    *events.borrow(),
                    match case {
                        "owner" => vec!["owner"],
                        "elevation" => vec!["owner", "base", "elevation"],
                        _ => vec!["owner", "base", "elevation", "discover"],
                    }
                );
            }
        }
    }

    use super::*;

    #[test]
    fn november_owned_registry_keys_close_once_and_report_exactly_nonzero_close_statuses() {
        for result in [0_u32, 1, 2, u32::MAX] {
            let closes = Arc::new(AtomicUsize::new(0));
            let errors = Arc::new(Mutex::new(Vec::new()));
            let key = Key::Injected {
                closes: Arc::clone(&closes),
                result,
                errors: Arc::clone(&errors),
            };
            assert_eq!(closes.load(Ordering::Relaxed), 0);
            assert!(errors.lock().unwrap().is_empty());
            drop(key);
            assert_eq!(closes.load(Ordering::Relaxed), 1);
            assert_eq!(
                *errors.lock().unwrap(),
                if result == 0 { vec![] } else { vec![result.cast_signed()] }
            );
        }
    }

    #[test]
    fn november_known_folder_request_scans_exact_utf16_and_releases_ownership_on_all_statuses() {
        for path in ["", r"C:\Users\one\AppData\Local", r"C:\Δ\🚀"] {
            let units = path.encode_utf16().chain([0]).collect::<Vec<_>>();
            let text = units.as_ptr().cast_mut();
            for status in [i32::MIN, -1, 0, 1] {
                let releases = Arc::new(AtomicUsize::new(0));
                let reads = std::cell::RefCell::new(Vec::new());
                let result = local_app_data_with(
                    |flags, token, output| {
                        assert_eq!(flags, 0);
                        assert!(token.is_null());
                        *output = if status < 0 { ptr::null_mut() } else { text };
                        status
                    },
                    |pointer| {
                        assert_eq!(pointer, if status < 0 { ptr::null_mut() } else { text });
                        KnownFolder::Injected(Arc::clone(&releases))
                    },
                    |pointer, offset| {
                        assert!(status >= 0);
                        assert_eq!(pointer, text);
                        assert_eq!(offset, reads.borrow().len());
                        assert_eq!(releases.load(Ordering::Relaxed), 0);
                        reads.borrow_mut().push(offset);
                        units[offset]
                    },
                    |pointer, length| {
                        assert_eq!(pointer, text);
                        assert_eq!(length, units.len() - 1);
                        assert_eq!(releases.load(Ordering::Relaxed), 0);
                        PathBuf::from(OsString::from_wide(&units[..length]))
                    },
                );
                assert_eq!(releases.load(Ordering::Relaxed), 1);
                if status < 0 {
                    let error = result.unwrap_err();
                    assert_eq!(error.kind(), io::ErrorKind::Other);
                    assert_eq!(
                        error.to_string(),
                        if status == -1 {
                            "cannot locate local application data: HRESULT 0xffffffff"
                        } else {
                            "cannot locate local application data: HRESULT 0x80000000"
                        }
                    );
                    assert!(reads.borrow().is_empty());
                } else {
                    assert_eq!(result.unwrap(), Path::new(path));
                    assert_eq!(*reads.borrow(), (0..units.len()).collect::<Vec<_>>());
                }
            }
        }
    }

    #[test]
    fn gamma_round2_current_directory_pipeline_keeps_names_order_and_each_injected_failure() {
        let owner: MonitorOwner = "S-1-5-18".parse().unwrap();
        let parent = PathBuf::from(r"C:\profile\Seismograph");
        let child = PathBuf::from(r"C:\profile\Seismograph\monitor");
        let expected = [None, Some(parent.clone()), Some(parent), Some(child.clone()), Some(child.clone())];
        for failure in [None, Some(0), Some(1), Some(2), Some(3), Some(4)] {
            let calls = std::cell::RefCell::new(Vec::new());
            let operation = |path: Option<&Path>| {
                let index = calls.borrow().len();
                calls.borrow_mut().push(path.map(Path::to_owned));
                if failure == Some(index) {
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("scripted failure {index}")))
                } else {
                    Ok(index)
                }
            };
            let result = prepare_current_directory(
                &owner,
                || operation(None).map(|_| PathBuf::from(r"C:\profile")),
                |path, actual_owner| {
                    assert_eq!(actual_owner, &owner);
                    operation(Some(path)).map(drop)
                },
                |path, actual_owner| {
                    assert_eq!(actual_owner, &owner);
                    operation(Some(path))
                },
            );
            if let Some(index) = failure {
                let error = result.unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                assert_eq!(error.to_string(), format!("scripted failure {index}"));
                assert_eq!(calls.borrow().as_slice(), &expected[..=index]);
            } else {
                assert_eq!(result.unwrap(), (child.clone(), 4));
                assert_eq!(calls.borrow().as_slice(), expected);
            }
        }
    }

    #[test]
    fn gamma_round2_registry_enumeration_filters_invalid_names_and_preserves_distinct_owners() {
        let owner: MonitorOwner = "S-1-5-18".parse().unwrap();
        let other: MonitorOwner = "S-1-5-19".parse().unwrap();
        let base = PathBuf::from(r"C:\profile");
        let names = [
            vec![0xd800],
            "not a SID".encode_utf16().collect(),
            "S-1-5-18".encode_utf16().collect(),
            "S-1-5-19".encode_utf16().collect(),
            "S-1-5-20".encode_utf16().collect(),
        ];
        let mut queries = 0;
        let entries = registry_directories_with(
            owner.clone(),
            Ok(base.clone()),
            true,
            |name, reserved, access| {
                assert_eq!(name.last(), Some(&0));
                assert_eq!(
                    String::from_utf16(&name[..name.len() - 1]).unwrap(),
                    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList"
                );
                assert_eq!((reserved, access), (0, 0x0002_0119));
                (0, 37)
            },
            |key| {
                assert_eq!(key, 37);
                key
            },
            |key, index, name, length| {
                assert_eq!(*key, 37);
                assert_eq!(index as usize, queries);
                assert_eq!((name.len(), *length), (256, 256));
                queries += 1;
                assert!(queries <= 6, "enumeration must stop at the first no-more-items result");
                let Some(value) = names.get(index as usize) else {
                    return 259;
                };
                name[..value.len()].copy_from_slice(value);
                *length = u32::try_from(value.len()).unwrap();
                0
            },
            |sid| match sid {
                "S-1-5-18" | "S-1-5-19" => Ok(base.clone()),
                "S-1-5-20" => Err(io::Error::from_raw_os_error(5)),
                _ => panic!("invalid SID reached profile lookup"),
            },
        )
        .unwrap();
        assert_eq!(queries, 6);
        assert_eq!(entries.len(), 3);
        let first = entries[0].as_ref().unwrap();
        let second = entries[1].as_ref().unwrap();
        assert_eq!((&first.path, &first.owner), (&base.join("Seismograph").join("monitor"), &owner));
        assert_eq!((&second.path, &second.owner), (&first.path, &other));
        assert_eq!(entries[2].as_ref().err().unwrap().raw_os_error(), Some(5));
        assert_eq!(profile_indexes(), 0..u32::MAX);
    }

    #[test]
    fn gamma_round2_registry_enumeration_preserves_open_and_query_errors_without_retrying() {
        let owner: MonitorOwner = "S-1-5-18".parse().unwrap();
        let entries = registry_directories_with(
            owner.clone(),
            Err(io::ErrorKind::NotFound.into()),
            true,
            |_, _, _| (5, ()),
            |()| panic!("failed registry open must not claim ownership"),
            |(), _, _, _| panic!("failed registry open must not enumerate"),
            |_| panic!("failed registry open must not resolve profiles"),
        )
        .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].as_ref().err().unwrap().kind(), io::ErrorKind::NotFound);
        assert_eq!(entries[1].as_ref().err().unwrap().raw_os_error(), Some(5));
        let mut queries = 0;
        let entries = registry_directories_with(
            owner,
            Ok(PathBuf::from(r"C:\profile")),
            true,
            |_, _, _| (0, ()),
            |()| (),
            |(), _, _, _| {
                queries += 1;
                assert_eq!(queries, 1);
                5
            },
            |_| panic!("failed enumeration must not resolve profiles"),
        )
        .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].as_ref().err().unwrap().raw_os_error(), Some(5));
        assert_eq!(queries, 1);
    }

    #[test]
    fn gamma_round2_non_elevated_registry_discovery_preserves_base_paths_and_failures() {
        let owner: MonitorOwner = "S-1-5-18".parse().unwrap();
        let entries = registry_directories_for(owner.clone(), Ok(PathBuf::from(r"C:\profile")), false).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = entries[0].as_ref().unwrap();
        assert_eq!(entry.owner, owner);
        assert_eq!(entry.path, PathBuf::from(r"C:\profile\Seismograph\monitor"));
        let entries = registry_directories_for(owner, Err(io::ErrorKind::PermissionDenied.into()), false).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].as_ref().err().unwrap().kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn gamma_round2_profile_lookup_preserves_names_expansion_fallback_and_errors() {
        for (result, expected) in [
            (Ok(PathBuf::from(r"D:\Redirected")), Ok(PathBuf::from(r"D:\Redirected"))),
            (
                Ok(PathBuf::from(r"%USERPROFILE%\Agent")),
                Ok(PathBuf::from(r"C:\Users\Alice\Agent")),
            ),
            (Err(io::ErrorKind::NotFound), Ok(PathBuf::from(r"C:\Users\Alice\AppData\Local"))),
            (Err(io::ErrorKind::PermissionDenied), Err(io::ErrorKind::PermissionDenied)),
        ] {
            let mut calls = Vec::new();
            let mut result = Some(result);
            let actual = profile_local_app_data_with("S-1-5-21-1-2-3-1001", |root, key, value, expand| {
                calls.push((root, key.to_owned(), value.to_owned(), expand));
                if calls.len() == 1 {
                    Ok(PathBuf::from(r"C:\Users\Alice"))
                } else {
                    result.take().unwrap().map_err(io::Error::from)
                }
            });
            assert_eq!(actual.map_err(|error| error.kind()), expected);
            assert_eq!(
                calls,
                [
                    (
                        HKEY_LOCAL_MACHINE,
                        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\S-1-5-21-1-2-3-1001".into(),
                        "ProfileImagePath".into(),
                        true
                    ),
                    (
                        HKEY_USERS,
                        r"S-1-5-21-1-2-3-1001\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders".into(),
                        "Local AppData".into(),
                        false
                    ),
                ]
            );
        }
        let error =
            profile_local_app_data_with("S-1-5-21-1-2-3-1001", |_, _, _, _| Err(io::ErrorKind::PermissionDenied.into())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn gamma_round2_registry_queries_preserve_flags_lengths_and_reported_results() {
        for (expand, path, expected_flags) in [
            (true, r"C:\Profiles\Alice".to_owned(), 0x0000_ffff),
            (false, "relative".to_owned(), 0x1000_ffff),
            (false, "x".repeat(20_000), 0x1000_ffff),
        ] {
            let data = path.encode_utf16().chain([0]).collect::<Vec<_>>();
            let result = registry_string_with("round2-key", "round2-value", expand, |key, value, flags, kind, output, bytes| {
                assert_eq!(key, "round2-key".encode_utf16().chain([0]).collect::<Vec<_>>());
                assert_eq!(value, "round2-value".encode_utf16().chain([0]).collect::<Vec<_>>());
                assert_eq!(flags, expected_flags);
                if (*bytes as usize) < data.len() * 2 {
                    return 234;
                }
                output[..data.len()].copy_from_slice(&data);
                *kind = REG_SZ;
                *bytes = u32::try_from(data.len() * 2).unwrap();
                0
            })
            .unwrap();
            assert_eq!(result, Path::new(&path));
        }
        assert_eq!(
            registry_string_with("key", "value", false, |_, _, _, _, _, _| 5)
                .unwrap_err()
                .raw_os_error(),
            Some(5)
        );
        let relative = "relative".encode_utf16().chain([0]).collect::<Vec<_>>();
        assert_eq!(
            registry_string_with("key", "value", true, |_, _, _, kind, data, bytes| {
                data[..relative.len()].copy_from_slice(&relative);
                *kind = REG_SZ;
                *bytes = u32::try_from(relative.len() * 2).unwrap();
                0
            })
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn gamma_round2_registry_names_reject_nul_before_querying() {
        for (key, value) in [("bad\0key", "value"), ("key", "bad\0value")] {
            let error = registry_string_with(key, value, false, |_, _, _, _, _, _| {
                panic!("invalid names must not query the registry")
            })
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(error.to_string(), "native name contains a null character");
        }
    }

    #[test]
    fn gamma_round2_profile_placeholder_requires_the_exact_case_insensitive_prefix() {
        let profile = Path::new(r"C:\Users\Alice");
        for value in [r"%OTHERTHINGX%\AppData", r"notuserprofile\AppData", r"%USERPROFILX%\AppData"] {
            assert_eq!(
                profile_relative_path(Path::new(value), profile).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
        assert_eq!(
            profile_relative_path(Path::new(r"%uSeRpRoFiLe%\AppData"), profile).unwrap(),
            profile.join("AppData")
        );
    }

    #[test]
    fn gamma_survivor_profile_paths_keep_user_relative_resolution_and_error_text() {
        let profile = Path::new(r"C:\Users\Alice");
        assert_eq!(
            profile_relative_path(Path::new(r"D:\Absolute\Local"), profile).unwrap(),
            Path::new(r"D:\Absolute\Local")
        );
        assert_eq!(
            profile_relative_path(Path::new(r"%userprofile%\x"), profile).unwrap(),
            profile.join("x")
        );
        assert_eq!(
            profile_relative_path(Path::new(r"%USERPROFILE%/x"), profile).unwrap(),
            profile.join("x")
        );
        for path in ["%USERPROFILE%", "%USERPROFILE%xx", "relative"] {
            assert_eq!(
                profile_relative_path(Path::new(path), profile).unwrap_err().to_string(),
                "local application data must be absolute or relative to %USERPROFILE%"
            );
        }
        assert_eq!(
            profile_relative_path(Path::new(&OsString::from_wide(&[0xd800; 15])), profile)
                .unwrap_err()
                .to_string(),
            "local application data must be absolute or relative to %USERPROFILE%"
        );
        for (kind, count, units) in [(0, 2, &[0][..]), (REG_SZ, 1, &[0][..]), (REG_SZ, 4, &[0][..])] {
            assert_eq!(
                registry_value_path(kind, count, units, false).unwrap_err().to_string(),
                "user profile path is not a bounded UTF-16 string"
            );
        }
        assert_eq!(
            registry_value_path(REG_SZ, 2, &[120], false).unwrap_err().to_string(),
            "user profile path is unterminated"
        );
        assert_eq!(
            registry_value_path(REG_SZ, 4, &[120, 0], true).unwrap_err().to_string(),
            "user profile path must be absolute"
        );
        assert_eq!(registry_value_path(REG_SZ, 4, &[120, 0], false).unwrap(), Path::new("x"));
        assert_eq!(registry_value_path(REG_SZ, 4, &[120, 0, 121, 0], false).unwrap(), Path::new("x"));
        let absolute = r"C:\Users\Alice".encode_utf16().chain([0]).collect::<Vec<_>>();
        assert_eq!(registry_value_path(REG_SZ, absolute.len() * 2, &absolute, true).unwrap(), profile);
        assert_eq!(
            registry_value_path(REG_EXPAND_SZ, absolute.len() * 2, &absolute, true).unwrap(),
            profile
        );
    }

    #[test]
    fn registry_values_require_a_bounded_terminated_utf16_string() {
        let data = r"C:\Users\Alice".encode_utf16().chain([0]).collect::<Vec<_>>();
        assert_eq!(
            registry_value_path(REG_SZ, data.len() * 2, &data, true).unwrap(),
            Path::new(r"C:\Users\Alice")
        );
        let relative = "%USERPROFILE%".encode_utf16().chain([0]).collect::<Vec<_>>();
        assert_eq!(
            registry_value_path(REG_EXPAND_SZ, relative.len() * 2, &relative, false).unwrap(),
            Path::new("%USERPROFILE%")
        );
        for (kind, bytes, units, expand) in [
            (0, data.len() * 2, data.as_slice(), false),
            (REG_SZ, 3, data.as_slice(), false),
            (REG_SZ, data.len() * 2 + 2, data.as_slice(), false),
            (REG_SZ, (data.len() - 1) * 2, data.as_slice(), false),
            (REG_SZ, relative.len() * 2, relative.as_slice(), true),
        ] {
            assert_eq!(
                registry_value_path(kind, bytes, units, expand).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn conflicting_owner_claims_do_not_hide_a_registry() {
        let path = PathBuf::from(r"C:\Users\Alice\AppData\Local\Seismograph\monitor");
        let pretender = MonitorOwner::WindowsSid("S-1-5-21-1-2-3-1002".into());
        let owner = MonitorOwner::WindowsSid("S-1-5-21-1-2-3-1001".into());
        let mut entries = [pretender.clone(), owner.clone(), owner.clone()]
            .map(|owner| Ok(DirectoryEntry { path: path.clone(), owner }))
            .into_iter()
            .collect::<Vec<_>>();
        entries.push(Err(io::Error::new(io::ErrorKind::InvalidData, "invalid profile")));
        deduplicate_entries(&mut entries);
        let actual = entries
            .into_iter()
            .map(|entry| entry.map(|entry| (entry.path, entry.owner)).map_err(|error| error.kind()))
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            [Ok((path.clone(), pretender)), Ok((path, owner)), Err(io::ErrorKind::InvalidData),]
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "native known-folder and profile APIs")]
    fn current_user_discovery_does_not_require_registry_provisioning() {
        let path = local_app_data().unwrap();
        assert!(path.is_absolute());
        let entries = registry_directories().unwrap();
        let owner = current_owner().unwrap();
        assert!(
            entries
                .into_iter()
                .filter_map(Result::ok)
                .any(|entry| entry.owner == owner && entry.path == registry_path(&path))
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "native profile enumeration is exercised by native and careful tests")]
    fn profile_discovery_preserves_current_user_lookup_failures() {
        let owner = current_owner().unwrap();
        let entries = registry_directories_for(owner.clone(), Err(io::Error::other("current folder unavailable")), true).unwrap();
        assert_eq!(entries[0].as_ref().unwrap_err().to_string(), "current folder unavailable");
        let base = local_app_data().unwrap();
        let entries = registry_directories_for(owner.clone(), Ok(base.clone()), false).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = entries.into_iter().next().unwrap().unwrap();
        assert_eq!((entry.path, entry.owner), (base.join("Seismograph").join("monitor"), owner));
    }

    #[test]
    fn other_user_profile_variables_are_not_expanded_as_the_administrator() {
        assert_eq!(
            profile_relative_path(Path::new(r"%USERPROFILE%\AppData\Local"), Path::new(r"C:\Users\Alice")).unwrap(),
            Path::new(r"C:\Users\Alice\AppData\Local")
        );
        assert_eq!(
            profile_relative_path(Path::new(r"D:\Redirected\Alice"), Path::new(r"C:\Users\Alice")).unwrap(),
            Path::new(r"D:\Redirected\Alice")
        );
        profile_relative_path(Path::new("relative"), Path::new(r"C:\Users\Alice")).unwrap_err();
        assert_eq!(
            profile_relative_path(Path::new("%USERPROFILE%"), Path::new(r"C:\Users\Alice"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
