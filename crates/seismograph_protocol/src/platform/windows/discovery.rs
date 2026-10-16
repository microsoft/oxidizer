// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};
use std::{io, ptr};

use windows_sys::Win32::Foundation::ERROR_NO_MORE_ITEMS;
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

struct KnownFolder(*mut u16);

impl Drop for KnownFolder {
    fn drop(&mut self) {
        // SAFETY: SHGetKnownFolderPath allocated this string with CoTaskMemAlloc.
        unsafe { CoTaskMemFree(self.0.cast()) };
    }
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: this key was opened by RegOpenKeyExW and is owned here.
        let result = unsafe { RegCloseKey(self.0) };
        if result != 0 {
            eprintln!(
                "seismograph: failed to close profile registry key: {}",
                io::Error::from_raw_os_error(result.cast_signed())
            );
        }
    }
}

fn local_app_data() -> io::Result<PathBuf> {
    let mut path = ptr::null_mut();
    // SAFETY: the folder ID is valid and path is a writable output pointer.
    let result = unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, ptr::null_mut(), &raw mut path) };
    if result < 0 {
        return Err(io::Error::other(format!(
            "cannot locate local application data: HRESULT {result:#x}"
        )));
    }
    let path = KnownFolder(path);
    let mut length = 0;
    // SAFETY: the successful API call supplied an owned, terminated UTF-16 string.
    while unsafe { *path.0.wrapping_add(length) } != 0 {
        length += 1;
    }
    // SAFETY: length covers the readable string before its terminator.
    Ok(PathBuf::from(OsString::from_wide(unsafe {
        std::slice::from_raw_parts(path.0, length)
    })))
}

fn registry_path(base: &Path) -> PathBuf {
    base.join("Seismograph").join("monitor")
}

pub(crate) fn current_directory(owner: &MonitorOwner) -> io::Result<RegistryDirectory> {
    let base = local_app_data()?.join("Seismograph");
    create_directory(&base, owner)?;
    let _parent = open_directory(&base, owner)?;
    let path = base.join("monitor");
    create_directory(&path, owner)?;
    let file = open_directory(&path, owner)?;
    Ok(RegistryDirectory {
        path,
        file,
        temporary: None,
    })
}

pub(crate) fn registry_directories() -> io::Result<DirectoryEntries> {
    let owner = current_owner()?;
    let mut entries = vec![local_app_data().map(|base| DirectoryEntry {
        path: registry_path(&base),
        owner,
    })];
    if !is_elevated()? {
        return Ok(entries);
    }
    let name = wide(std::ffi::OsStr::new(PROFILES))?;
    let mut key = ptr::null_mut();
    // SAFETY: the predefined root and terminated name are valid; key is writable.
    let result = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, name.as_ptr(), 0, KEY_READ | KEY_WOW64_64KEY, &raw mut key) };
    if result != 0 {
        entries.push(Err(io::Error::from_raw_os_error(result.cast_signed())));
        return Ok(entries);
    }
    let key = Key(key);
    for index in 0..u32::MAX {
        let mut name = [0_u16; KEY_NAME_UNITS];
        let mut length = u32::try_from(name.len()).expect("registry key buffer fits u32");
        // SAFETY: key is open, name/length are valid buffers, and optional outputs are null.
        let result = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                name.as_mut_ptr(),
                &raw mut length,
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if result == ERROR_NO_MORE_ITEMS {
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
        entries.push(profile_local_app_data(&sid).map(|base| DirectoryEntry {
            path: registry_path(&base),
            owner,
        }));
    }
    // The current user's entry may also have been enumerated from ProfileList.
    let mut seen = std::collections::BTreeSet::new();
    entries.retain(|entry| match entry {
        Ok(entry) => seen.insert(entry.path.clone()),
        Err(_) => true,
    });
    Ok(entries)
}

fn profile_local_app_data(sid: &str) -> io::Result<PathBuf> {
    let profile = registry_string(HKEY_LOCAL_MACHINE, &format!(r"{PROFILES}\{sid}"), "ProfileImagePath", true)?;
    let settings = format!(r"{sid}\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders");
    match registry_string(HKEY_USERS, &settings, "Local AppData", false) {
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

fn registry_string(root: HKEY, key: &str, value: &str, expand: bool) -> io::Result<PathBuf> {
    let key = wide(std::ffi::OsStr::new(key))?;
    let value = wide(std::ffi::OsStr::new(value))?;
    let mut data = vec![0_u16; MAX_PATH_UNITS];
    let mut bytes = u32::try_from(data.len() * size_of::<u16>()).expect("bounded registry path buffer fits u32");
    let mut kind = 0;
    // Do not expand another user's USERPROFILE against the administrator's environment.
    let flags = RRF_RT_ANY | if expand { 0 } else { RRF_NOEXPAND };
    // SAFETY: predefined root, terminated names, and output buffers are valid.
    let result = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            value.as_ptr(),
            flags,
            &raw mut kind,
            data.as_mut_ptr().cast(),
            &raw mut bytes,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result.cast_signed()));
    }
    if !matches!(kind, REG_SZ | REG_EXPAND_SZ) || bytes % 2 != 0 || bytes as usize > data.len() * 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "user profile path is not a bounded UTF-16 string",
        ));
    }
    let units = &data[..bytes as usize / 2];
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
    use super::*;

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
    }
}
