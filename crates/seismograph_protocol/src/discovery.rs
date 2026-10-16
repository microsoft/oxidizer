// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Automatically initialized per-user discovery registries.
//!
//! Unix prefers `$XDG_RUNTIME_DIR/seismograph`. Without a usable runtime
//! directory, each monitor owns a private randomized directory directly under
//! `/tmp`. Windows uses the user's local application-data directory.
//! No administrator setup is required. Discovery aggregates accessible
//! registries while retaining owner and administrator access checks.
//! Windows registries require direct local-disk paths; network locations,
//! substituted drives, and reparse-point ancestors are rejected before probing.

use std::io;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::io::{DirectoryGuard, MonitorIo};
use crate::monitor::{MonitorDescriptor, MonitorOwner};

/// A private monitor directory retained open with verified OS access control.
#[derive(Debug)]
pub struct Directory {
    path: PathBuf,
    owner: MonitorOwner,
    io: MonitorIo,
    _guard: DirectoryGuard,
}

impl Directory {
    /// Creates or opens the current user's registry without administrator setup.
    ///
    /// A Unix temporary fallback is private to this handle and removed on drop.
    /// Retain the directory until its listener and descriptor have been removed.
    ///
    /// # Errors
    ///
    /// Fails for unsafe ownership/permissions, unavailable user storage, or I/O errors.
    pub fn current(io: &MonitorIo) -> Result<Self, Error> {
        let owner = io.current_owner().map_err(Error::Io)?;
        let (path, guard) = io.current_directory(&owner).map_err(Error::Io)?;
        Ok(Self {
            path,
            owner,
            io: io.clone(),
            _guard: guard,
        })
    }

    /// Opens a directory and verifies owner-only and administrator access.
    ///
    /// # Errors
    ///
    /// Fails for wrong ownership, redirection, unsafe permissions, or I/O errors.
    pub fn open(io: &MonitorIo, path: &Path, owner: &MonitorOwner) -> Result<Self, Error> {
        let guard = io.open_directory(path, owner).map_err(|source| failure(path, source))?;
        Ok(Self {
            path: path.to_owned(),
            owner: owner.clone(),
            io: io.clone(),
            _guard: guard,
        })
    }

    /// Returns this directory's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the verified directory owner.
    #[must_use]
    pub const fn owner(&self) -> &MonitorOwner {
        &self.owner
    }

    pub(crate) const fn io(&self) -> &MonitorIo {
        &self.io
    }

    /// Iterates over descriptors, reporting malformed or unreadable entries.
    ///
    /// # Errors
    ///
    /// Fails when enumeration cannot start. Iterator errors identify invalid
    /// entries without hiding valid peers.
    pub fn descriptors(&self) -> Result<impl Iterator<Item = Result<MonitorDescriptor, Error>> + '_, Error> {
        let entries = self.io.entries(&self.path).map_err(|source| failure(&self.path, source))?;
        Ok(entries.into_iter().filter_map(|entry| {
            let path = match entry {
                Ok(path) => path,
                Err(source) => return Some(Err(failure(&self.path, source))),
            };
            if path.extension().is_none_or(|extension| extension != "monitor") {
                return None;
            }
            let result = match MonitorDescriptor::read_file(&self.io, &path) {
                Ok(descriptor) if descriptor.owner == self.owner => Ok(descriptor),
                Ok(_) => Err(failure(
                    &path,
                    io::Error::new(io::ErrorKind::InvalidData, "descriptor owner differs from its directory"),
                )),
                Err(Error::Registry { source, .. }) if source.kind() == io::ErrorKind::NotFound => return None,
                Err(error) => Err(failure(&path, io::Error::new(io::ErrorKind::InvalidData, error))),
            };
            Some(result)
        }))
    }
}

/// Opens accessible per-user registries without creating any directories.
///
/// Linux also uses the kernel's Unix-socket table to discover nonstandard XDG
/// locations in the current network and mount namespace. Other Unix systems
/// scan conventional runtime and private temporary registries. Windows
/// administrators aggregate registered profiles' local application data.
///
/// # Errors
///
/// Fails when discovery cannot start. Iterator errors identify unsafe or
/// unreadable registries without hiding other users. Missing directories and
/// ordinary permission denials for other users are skipped.
pub fn directories(io: &MonitorIo) -> Result<impl Iterator<Item = Result<Directory, Error>>, Error> {
    let entries = io.registry_directories().map_err(Error::Io)?;
    let caller = io.current_owner().map_err(Error::Io)?;
    let io = io.clone();
    Ok(entries.into_iter().filter_map(move |entry| {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => return Some(Err(Error::Io(error))),
        };
        match Directory::open(&io, &entry.path, &entry.owner) {
            Ok(directory) => Some(Ok(directory)),
            Err(Error::Registry { source, .. }) if source.kind() == io::ErrorKind::NotFound => None,
            Err(Error::Registry { source, .. })
                if source.kind() == io::ErrorKind::PermissionDenied && source.raw_os_error().is_some() && entry.owner != caller =>
            {
                None
            }
            Err(error) => Some(Err(error)),
        }
    }))
}

fn failure(path: &Path, source: io::Error) -> Error {
    Error::Registry {
        path: path.to_owned(),
        source,
    }
}
