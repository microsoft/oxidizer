// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::{OsStr, OsString};
use std::fs::{FileType, Metadata};
use std::io::{Error, Result};

/// An entry within a directory.
///
/// This type is returned by [`ReadDir::next_entry`](crate::read_dir::ReadDir::next_entry).
/// Unlike `std::fs::DirEntry`, this type does not expose the full path to the
/// entry, preserving the capability-based access model.
///
/// Metadata and file type are fetched eagerly during directory iteration,
/// so accessing them is allocation-free and instant.
#[derive(Debug)]
pub struct DirEntry {
    file_name: OsString,
    file_type: Result<FileType>,
    metadata: Result<Metadata>,
}

impl DirEntry {
    /// Creates a `DirEntry` by eagerly capturing all data from a `std::fs::DirEntry`.
    pub(crate) fn from_std(entry: &std::fs::DirEntry) -> Self {
        let file_name = entry.file_name();
        let metadata = entry.metadata();
        // Extract file_type from metadata when available, avoiding a
        // separate syscall on platforms where file_type() would stat again.
        let file_type = metadata.as_ref().map_or_else(|_| entry.file_type(), |m| Ok(m.file_type()));
        Self {
            file_name,
            file_type,
            metadata,
        }
    }

    /// Returns the bare file name of this directory entry without any other
    /// leading path component.
    #[must_use]
    pub fn file_name(&self) -> &OsStr {
        &self.file_name
    }

    /// Returns the metadata for the file that this entry points at.
    ///
    /// This function will not traverse symlinks if this entry points at a
    /// symlink.
    ///
    /// # Errors
    ///
    /// Returns an error if the metadata could not be read when the directory
    /// was iterated.
    pub const fn metadata(&self) -> core::result::Result<&Metadata, &Error> {
        self.metadata.as_ref()
    }

    /// Returns the file type for the file that this entry points at.
    ///
    /// This function will not traverse symlinks if this entry points at a
    /// symlink.
    ///
    /// # Errors
    ///
    /// Returns an error if the file type could not be read when the directory
    /// was iterated.
    pub const fn file_type(&self) -> core::result::Result<FileType, &Error> {
        match &self.file_type {
            Ok(ft) => Ok(*ft),
            Err(e) => Err(e),
        }
    }
}
