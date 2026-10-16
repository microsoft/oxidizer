// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::*;
#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(not(any(unix, windows)))]
compile_error!("Seismograph local monitors require Unix or Windows");

#[derive(Debug)]
pub(crate) struct RegistryDirectory {
    pub(crate) path: std::path::PathBuf,
    pub(crate) file: std::fs::File,
    pub(crate) temporary: Option<tempfile::TempDir>,
}

#[derive(Debug)]
pub(crate) struct DirectoryEntry {
    pub(crate) path: std::path::PathBuf,
    pub(crate) owner: crate::monitor::MonitorOwner,
}

pub(crate) type DirectoryEntries = Vec<std::io::Result<DirectoryEntry>>;
