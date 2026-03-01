// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Result;
use std::path::Path;
use std::sync::{Arc, RwLock};

use bytesbuf::mem::MemoryShared;

use crate::directory::Directory;
use crate::file::File;
use crate::file_inner::FileInner;
use crate::path_utils::safe_join;
use crate::shared_memory::SharedMemory;

/// Options and flags which can be used to configure how a file is opened.
///
/// This builder exposes the ability to configure how a [`File`] is opened
/// and what operations are permitted on the open file. The [`File::open`],
/// [`File::create`], and [`File::create_new`] methods are aliases
/// for commonly used options using this builder.
///
/// Generally speaking, when using `OpenOptions`, you'll first call [`OpenOptions::new`],
/// then chain calls to methods to set each option, then call [`OpenOptions::open`],
/// passing a directory capability and a relative path.
#[derive(Clone, Copy, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "mirrors std::fs::OpenOptions API")]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
}

impl OpenOptions {
    /// Creates a blank new set of options ready for configuration.
    ///
    /// All options are initially set to `false`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
        }
    }

    /// Sets the option for read access.
    ///
    /// This option, when true, will indicate that the file should be readable
    /// if opened.
    pub const fn read(&mut self, read: bool) -> &mut Self {
        self.read = read;
        self
    }

    /// Sets the option for write access.
    ///
    /// This option, when true, will indicate that the file should be writable
    /// if opened. If the file already exists, any write calls on it will
    /// overwrite its contents, without truncating it.
    pub const fn write(&mut self, write: bool) -> &mut Self {
        self.write = write;
        self
    }

    /// Sets the option for the append mode.
    ///
    /// This option, when true, means that writes will append to a file instead
    /// of overwriting previous contents. Note that setting `.write(true).append(true)`
    /// has the same effect as setting only `.append(true)`.
    ///
    /// This function doesn't create the file if it doesn't exist. Use the
    /// [`create`](OpenOptions::create) method to do so.
    pub const fn append(&mut self, append: bool) -> &mut Self {
        self.append = append;
        self
    }

    /// Sets the option for truncating a previous file.
    ///
    /// If a file is successfully opened with this option set to true, it will
    /// truncate the file to 0 length if it already exists. The file must be
    /// opened with write access for truncate to work.
    pub const fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.truncate = truncate;
        self
    }

    /// Sets the option to create a new file, or open it if it already exists.
    ///
    /// In order for the file to be created, write or append access must be used.
    pub const fn create(&mut self, create: bool) -> &mut Self {
        self.create = create;
        self
    }

    /// Sets the option to always create a new file, failing if it already exists.
    ///
    /// No file is allowed to exist at the target location. This option is useful
    /// because it is atomic, avoiding TOCTOU race conditions. If
    /// `.create_new(true)` is set, `.create()` and `.truncate()` are ignored.
    pub const fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.create_new = create_new;
        self
    }

    /// Opens a file at the given path relative to the directory capability with
    /// the options specified by `self`.
    ///
    /// # Errors
    ///
    /// This function will return an error under a number of different
    /// circumstances, including but not limited to:
    ///
    /// * [`NotFound`](std::io::ErrorKind::NotFound): the specified file does
    ///   not exist and neither `create` nor `create_new` is set.
    /// * [`PermissionDenied`](std::io::ErrorKind::PermissionDenied): the user
    ///   lacks permission to get the specified access rights for the file, or
    ///   the file or one of its parent directories does not allow access.
    /// * [`AlreadyExists`](std::io::ErrorKind::AlreadyExists): `create_new`
    ///   was specified and the file already exists.
    /// * [`InvalidInput`](std::io::ErrorKind::InvalidInput): invalid
    ///   combinations of open options were used.
    pub async fn open(&self, dir: &Directory, path: impl AsRef<Path>) -> Result<File> {
        self.open_inner(dir, path, SharedMemory::global()).await
    }

    /// Opens a file with the specified options using a custom memory provider.
    ///
    /// The memory provider controls buffer allocation for subsequent I/O
    /// operations on the returned file.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`open`](OpenOptions::open).
    pub async fn open_with_memory(&self, dir: &Directory, path: impl AsRef<Path>, memory: impl MemoryShared) -> Result<File> {
        self.open_inner(dir, path, SharedMemory::new(memory)).await
    }

    async fn open_inner(&self, dir: &Directory, path: impl AsRef<Path>, memory: SharedMemory) -> Result<File> {
        let full_path = safe_join(dir.base_path(), path)?;
        let opts = *self;
        let file = dir
            .dispatcher()
            .dispatch(move || {
                std::fs::OpenOptions::new()
                    .read(opts.read)
                    .write(opts.write)
                    .append(opts.append)
                    .truncate(opts.truncate)
                    .create(opts.create)
                    .create_new(opts.create_new)
                    .open(&full_path)
            })
            .await?;
        Ok(File::new(FileInner::new(
            Arc::new(RwLock::new(file)),
            dir.dispatcher().clone(),
            memory,
        )))
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}
