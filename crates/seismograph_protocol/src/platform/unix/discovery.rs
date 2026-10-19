// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::io::{BufRead as _, BufReader};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::{fs, io};

use super::{create_directory, current_owner, directory_file, open_directory};
use crate::monitor::{InstanceId, MonitorEndpoint, MonitorOwner};
use crate::platform::{DirectoryEntries, DirectoryEntry, RegistryDirectory};

pub(crate) fn current_directory(owner: &MonitorOwner) -> io::Result<RegistryDirectory> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let directory = current_directory_in(runtime.as_deref(), Path::new("/tmp"), owner)?;
    if directory.temporary.is_some() {
        eprintln!(
            "seismograph: XDG_RUNTIME_DIR unavailable; using private temporary registry {}",
            directory.path.display()
        );
    }
    Ok(directory)
}

fn current_directory_in(runtime: Option<&Path>, temporary_root: &Path, owner: &MonitorOwner) -> io::Result<RegistryDirectory> {
    if let Some(runtime) = runtime.filter(|path| path.is_absolute()) {
        match runtime_directory(runtime, owner) {
            Ok(directory) => return Ok(directory),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied | io::ErrorKind::NotADirectory | io::ErrorKind::InvalidInput
                ) || error.raw_os_error() == Some(libc::ELOOP) =>
            {
                eprintln!(
                    "seismograph: ignoring unusable XDG runtime directory {}: {error}",
                    runtime.display()
                );
            }
            Err(error) => return Err(error),
        }
    }
    let MonitorOwner::UnixUser(uid) = owner else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Unix registry requires a Unix user ID"));
    };
    // /tmp can be an OS-managed symlink (notably on macOS).
    let root = fs::canonicalize(temporary_root)?;
    let guard = directory_file(&root)?;
    let metadata = guard.metadata()?;
    if metadata.uid() != 0 || metadata.mode() & 0o1000 == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "shared temporary directory must be root-owned and sticky",
        ));
    }
    let temporary = tempfile::Builder::new()
        .prefix(&format!("seismograph-{uid}-"))
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(&root)?;
    let path = temporary.path().to_owned();
    let file = open_directory(&path, owner)?;
    Ok(RegistryDirectory {
        path,
        file,
        temporary: Some(temporary),
    })
}

fn runtime_directory(runtime: &Path, owner: &MonitorOwner) -> io::Result<RegistryDirectory> {
    if runtime.to_str().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "runtime directory cannot be represented in a monitor descriptor",
        ));
    }
    let _runtime = open_directory(runtime, owner)?;
    let path = runtime.join("seismograph");
    let endpoint = MonitorEndpoint::for_instance(&path, owner, std::process::id(), InstanceId::from_bytes([0; 16]));
    std::os::unix::net::SocketAddr::from_pathname(endpoint.path())?;
    create_directory(&path, owner)?;
    let file = open_directory(&path, owner)?;
    Ok(RegistryDirectory {
        path,
        file,
        temporary: None,
    })
}

pub(crate) fn registry_directories() -> io::Result<DirectoryEntries> {
    let MonitorOwner::UnixUser(caller) = current_owner()? else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unix discovery requires a Unix user ID",
        ));
    };
    let mut paths = BTreeMap::new();
    let mut errors = Vec::new();
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        add_directory(runtime.join("seismograph"), caller, &mut paths, &mut errors);
    }
    if caller == 0 {
        scan_runtime_users(Path::new("/run/user"), caller, &mut paths, &mut errors);
    } else {
        add_directory(
            Path::new("/run/user").join(caller.to_string()).join("seismograph"),
            caller,
            &mut paths,
            &mut errors,
        );
    }
    scan_temporary(Path::new("/tmp"), caller, &mut paths, &mut errors);
    #[cfg(target_os = "linux")]
    scan_socket_table(Path::new("/proc/net/unix"), caller, &mut paths, &mut errors);
    errors.extend(paths.into_iter().map(|(path, owner)| Ok(DirectoryEntry { path, owner })));
    Ok(errors)
}

fn add_directory(path: PathBuf, caller: u32, paths: &mut BTreeMap<PathBuf, MonitorOwner>, errors: &mut DirectoryEntries) {
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() && (caller == 0 || metadata.uid() == caller) => {
            paths.insert(path, MonitorOwner::UnixUser(metadata.uid()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound || (caller != 0 && error.kind() == io::ErrorKind::PermissionDenied) => {}
        Err(error) => errors.push(Err(io::Error::new(
            error.kind(),
            format!("monitor discovery {}: {error}", path.display()),
        ))),
    }
}

fn scan_runtime_users(root: &Path, caller: u32, paths: &mut BTreeMap<PathBuf, MonitorOwner>, errors: &mut DirectoryEntries) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            errors.push(Err(error));
            return;
        }
    };
    for entry in entries {
        match entry {
            Ok(entry)
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| matches!(name.parse::<MonitorOwner>(), Ok(MonitorOwner::UnixUser(_)))) =>
            {
                add_directory(entry.path().join("seismograph"), caller, paths, errors);
            }
            Ok(_) => {}
            Err(error) => errors.push(Err(error)),
        }
    }
}

fn scan_temporary(root: &Path, caller: u32, paths: &mut BTreeMap<PathBuf, MonitorOwner>, errors: &mut DirectoryEntries) {
    let entries = match fs::canonicalize(root).and_then(fs::read_dir) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(Err(error));
            return;
        }
    };
    for entry in entries {
        match entry {
            Ok(entry) if temporary_name(&entry.file_name().to_string_lossy()) => add_directory(entry.path(), caller, paths, errors),
            Ok(_) => {}
            Err(error) => errors.push(Err(error)),
        }
    }
}

fn temporary_name(name: &str) -> bool {
    name.strip_prefix("seismograph-")
        .and_then(|name| name.split_once('-'))
        .is_some_and(|(uid, random)| !random.is_empty() && matches!(uid.parse::<MonitorOwner>(), Ok(MonitorOwner::UnixUser(_))))
}

#[cfg(target_os = "linux")]
fn scan_socket_table(table: &Path, caller: u32, paths: &mut BTreeMap<PathBuf, MonitorOwner>, errors: &mut DirectoryEntries) {
    let file = match File::open(table) {
        Ok(file) => file,
        Err(error) => {
            errors.push(Err(error));
            return;
        }
    };
    for entry in socket_table_directories(BufReader::new(file).lines()) {
        match entry {
            Ok(directory) => add_directory(directory, caller, paths, errors),
            Err(error) => errors.push(Err(error)),
        }
    }
}

#[cfg(target_os = "linux")]
fn socket_table_directories(mut lines: impl Iterator<Item = io::Result<String>>) -> impl Iterator<Item = io::Result<PathBuf>> {
    let mut stopped = false;
    std::iter::from_fn(move || {
        while !stopped {
            match lines.next()? {
                Ok(line) => {
                    if let Some(directory) = socket_directory(&line) {
                        return Some(Ok(directory.to_owned()));
                    }
                }
                Err(error) => {
                    stopped = error.kind() != io::ErrorKind::InvalidData;
                    return Some(Err(error));
                }
            }
        }
        None
    })
}

#[cfg(target_os = "linux")]
fn socket_directory(mut line: &str) -> Option<&Path> {
    for _ in 0..7 {
        line = line.trim_ascii_start();
        let end = line.find(|character: char| character.is_ascii_whitespace())?;
        line = &line[end..];
    }
    let path = Path::new(line.trim_ascii_start());
    let (pid, instance) = path.file_name()?.to_str()?.strip_suffix(".sock")?.split_once('-')?;
    if !path.is_absolute()
        || pid.parse::<u32>().ok()? == 0
        || instance.len() != 32
        || !instance.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let directory = path.parent()?;
    let name = directory.file_name()?.to_str()?;
    (name == "seismograph" || temporary_name(name)).then_some(directory)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::os::unix::ffi::OsStringExt as _;
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "native permissions are exercised by native tests")]
    fn runtime_registry_is_created_without_root_and_unsafe_parents_are_not_used() {
        let runtime = tempfile::tempdir().unwrap();
        fs::set_permissions(runtime.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let owner = current_owner().unwrap();
        let directory = current_directory_in(Some(runtime.path()), Path::new("/tmp"), &owner).unwrap();
        assert_eq!(directory.path, runtime.path().join("seismograph"));
        assert!(directory.temporary.is_none());
        assert_eq!(directory.file.metadata().unwrap().mode() & 0o777, 0o700);
        fs::set_permissions(runtime.path(), fs::Permissions::from_mode(0o777)).unwrap();
        let fallback = current_directory_in(Some(runtime.path()), Path::new("/tmp"), &owner).unwrap();
        assert!(fallback.temporary.is_some());
        assert_ne!(fallback.path, directory.path);
        assert_eq!(runtime.path().metadata().unwrap().mode() & 0o777, 0o777);
    }

    #[test]
    #[cfg_attr(miri, ignore = "native temporary files are exercised by native tests")]
    fn missing_runtime_gets_distinct_private_temporary_registries() {
        let owner = current_owner().unwrap();
        let first = current_directory_in(None, Path::new("/tmp"), &owner).unwrap();
        let second = current_directory_in(Some(Path::new("relative")), Path::new("/tmp"), &owner).unwrap();
        assert_ne!(first.path, second.path);
        assert_eq!(first.file.metadata().unwrap().mode() & 0o777, 0o700);
    }

    #[test]
    #[cfg_attr(miri, ignore = "native temporary files and socket path limits")]
    fn unusable_runtime_paths_fall_back_without_weakening_permissions() {
        let parent = tempfile::tempdir().unwrap();
        let runtime = parent.path().join("long-runtime-name-".repeat(8));
        fs::create_dir(&runtime).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        let owner = current_owner().unwrap();
        let directory = current_directory_in(Some(&runtime), Path::new("/tmp"), &owner).unwrap();
        assert!(directory.temporary.is_some());
        let endpoint = MonitorEndpoint::for_instance(&directory.path, &owner, std::process::id(), InstanceId::from_bytes([0; 16]));
        std::os::unix::net::SocketAddr::from_pathname(endpoint.path()).unwrap();
        assert_eq!(runtime.metadata().unwrap().mode() & 0o777, 0o700);
        assert!(!runtime.join("seismograph").exists());

        let runtime = parent.path().join(std::ffi::OsString::from_vec(b"runtime-\xff".to_vec()));
        fs::create_dir(&runtime).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        let directory = current_directory_in(Some(&runtime), Path::new("/tmp"), &owner).unwrap();
        assert!(directory.temporary.is_some());
        assert!(!runtime.join("seismograph").exists());
        assert_eq!(runtime.metadata().unwrap().mode() & 0o777, 0o700);
    }

    #[test]
    #[cfg_attr(miri, ignore = "native temporary directory ownership")]
    fn untrusted_temporary_roots_are_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            current_directory_in(None, temporary.path(), &current_owner().unwrap())
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn invalid_socket_table_text_does_not_hide_later_monitors() {
        let registry = PathBuf::from("/custom runtime/seismograph");
        let mut rows = b"Num RefCount Protocol Flags Type St Inode Path\n".to_vec();
        rows.extend_from_slice(b"0000: 2 0 10000 1 01 1234 /unrelated-\xff\n");
        rows.extend_from_slice(
            format!(
                "0000: 2 0 10000 1 01 1235 {}/42-01010101010101010101010101010101.sock\n",
                registry.display()
            )
            .as_bytes(),
        );
        let entries = socket_table_directories(io::Cursor::new(rows).lines())
            .map(|entry| entry.map_err(|error| error.kind()))
            .collect::<Vec<_>>();
        assert_eq!(entries, [Err(io::ErrorKind::InvalidData), Ok(registry)]);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn socket_table_read_failures_are_reported_without_looping() {
        let mut reads = 0;
        let lines = std::iter::from_fn(|| {
            reads += 1;
            Some(Err(io::Error::from(io::ErrorKind::IsADirectory)))
        });
        let entries = socket_table_directories(lines)
            .map(|entry| entry.map_err(|error| error.kind()))
            .collect::<Vec<_>>();
        assert_eq!((reads, entries), (1, vec![Err(io::ErrorKind::IsADirectory)]));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn socket_table_preserves_spaces_and_filters_unrelated_sockets() {
        let name = "42-01010101010101010101010101010101.sock";
        let line = format!("0000: 00000002 00000000 00010000 0001 01 1234 /custom runtime/seismograph/{name}");
        assert_eq!(socket_directory(&line), Some(Path::new("/custom runtime/seismograph")));
        assert_eq!(socket_directory("0000: 2 0 10000 1 01 1234 @abstract"), None);
        assert_eq!(socket_directory("truncated"), None);
        assert!(!temporary_name("seismograph-not-a-uid-random"));
    }
}
