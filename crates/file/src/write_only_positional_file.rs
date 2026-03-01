// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs::{File, FileTimes, Metadata, Permissions, TryLockError};
use std::io::Result;
use std::path::Path;

use bytesbuf::mem::{HasMemory, Memory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};

use crate::directory::Directory;
use crate::file_inner::FileInner;
use crate::shared_memory::SharedMemory;

/// A positional write-only file handle within a capability-based filesystem.
///
/// All I/O methods take `&self` and operate at explicit byte offsets, enabling
/// concurrent access from multiple tasks without cursor management.
///
/// Obtain a `WriteOnlyPositionalFile` by calling [`WriteOnlyPositionalFile::create`]
/// or [`WriteOnlyPositionalFile::create_new`], or by narrowing a
/// [`PositionalFile`](crate::PositionalFile) via [`From`].
#[derive(Debug)]
pub struct WriteOnlyPositionalFile {
    inner: FileInner,
}

impl WriteOnlyPositionalFile {
    /// Opens a file in write-only mode.
    ///
    /// This function will create the file if it does not exist, and will truncate
    /// it if it does. The path is relative to the given directory capability.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be created or opened.
    pub async fn create(dir: &Directory, path: impl AsRef<Path>) -> Result<Self> {
        let full_path = crate::path_utils::safe_join(dir.base_path(), path)?;
        let file = dir.dispatcher().dispatch(move || File::create(&full_path)).await?;
        Ok(Self {
            inner: FileInner::from_std(file, dir, SharedMemory::global()),
        })
    }

    /// Opens a file in write-only mode using the specified memory provider.
    ///
    /// This function will create the file if it does not exist, and will truncate
    /// it if it does. The custom memory provider allows the caller to control
    /// buffer allocation, enabling zero-copy transfers from other subsystems that
    /// share the same memory provider.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be created or opened.
    pub async fn create_with_memory(dir: &Directory, path: impl AsRef<Path>, memory: impl MemoryShared) -> Result<Self> {
        let full_path = crate::path_utils::safe_join(dir.base_path(), path)?;
        let file = dir.dispatcher().dispatch(move || File::create(&full_path)).await?;
        Ok(Self {
            inner: FileInner::from_std(file, dir, SharedMemory::new(memory)),
        })
    }

    /// Creates a new file in write-only mode; returns an error if the file exists.
    ///
    /// If the call succeeds, the file returned is guaranteed to be new. This option
    /// is useful because it is atomic. Otherwise, between checking whether a file
    /// exists and creating a new one, the file may have been created by another
    /// process (a TOCTOU race condition).
    ///
    /// # Errors
    ///
    /// Returns an error if the file already exists or cannot be created.
    pub async fn create_new(dir: &Directory, path: impl AsRef<Path>) -> Result<Self> {
        let full_path = crate::path_utils::safe_join(dir.base_path(), path)?;
        let file = dir.dispatcher().dispatch(move || File::create_new(&full_path)).await?;
        Ok(Self {
            inner: FileInner::from_std(file, dir, SharedMemory::global()),
        })
    }

    /// Creates a new file in write-only mode using the specified memory provider;
    /// returns an error if the file exists.
    ///
    /// Combines the atomicity guarantees of [`WriteOnlyPositionalFile::create_new`]
    /// with a custom memory provider for zero-copy transfers.
    ///
    /// # Errors
    ///
    /// Returns an error if the file already exists or cannot be created.
    pub async fn create_new_with_memory(dir: &Directory, path: impl AsRef<Path>, memory: impl MemoryShared) -> Result<Self> {
        let full_path = crate::path_utils::safe_join(dir.base_path(), path)?;
        let file = dir.dispatcher().dispatch(move || File::create_new(&full_path)).await?;
        Ok(Self {
            inner: FileInner::from_std(file, dir, SharedMemory::new(memory)),
        })
    }

    /// Writes the provided byte sequence to the file at `offset`.
    ///
    /// The method completes when all bytes have been written. Partial writes are
    /// retried automatically.
    ///
    /// # Errors
    ///
    /// Returns an error if the write operation fails.
    pub async fn write_at(&self, offset: u64, data: BytesView) -> Result<()> {
        self.inner.write_at(offset, data).await
    }

    /// Writes a byte slice to the file at `offset`.
    ///
    /// This is a convenience method for callers working with `&[u8]` rather than
    /// managed buffers. The data is copied internally to transfer it to the
    /// worker thread; prefer [`write_at`](Self::write_at) with [`BytesView`] for
    /// large or performance-sensitive writes.
    ///
    /// # Errors
    ///
    /// Returns an error if the write operation fails.
    pub async fn write_slice_at(&self, offset: u64, data: impl AsRef<[u8]>) -> Result<()> {
        self.inner.write_slice_at(offset, data.as_ref()).await
    }

    /// Queries metadata about the underlying file.
    ///
    /// # Errors
    ///
    /// Returns an error if the metadata cannot be retrieved.
    pub async fn metadata(&self) -> Result<Metadata> {
        self.inner.metadata().await
    }

    /// Truncates or extends the underlying file, updating the size of this file
    /// to become `size`.
    ///
    /// If `size` is less than the current file size, the file will be shrunk.
    /// If it is greater, the file will be extended with zeroes.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub async fn set_len(&self, size: u64) -> Result<()> {
        self.inner.set_len(size).await
    }

    /// Changes the modification time of the underlying file.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub async fn set_modified(&self, modified: std::time::SystemTime) -> Result<()> {
        self.inner.set_modified(modified).await
    }

    /// Changes the permissions on the underlying file.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub async fn set_permissions(&self, perms: Permissions) -> Result<()> {
        self.inner.set_permissions(perms).await
    }

    /// Changes the timestamps of the underlying file.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub async fn set_times(&self, times: FileTimes) -> Result<()> {
        self.inner.set_times(times).await
    }

    /// Attempts to sync all OS-internal file content and metadata to disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the sync operation fails.
    pub async fn sync_all(&self) -> Result<()> {
        self.inner.sync_all().await
    }

    /// Similar to [`WriteOnlyPositionalFile::sync_all`], except that it might not
    /// synchronize file metadata to the filesystem.
    ///
    /// # Errors
    ///
    /// Returns an error if the sync operation fails.
    pub async fn sync_data(&self) -> Result<()> {
        self.inner.sync_data().await
    }

    /// Flushes any buffered data to the underlying file.
    ///
    /// Call this before dropping to ensure all data is written.
    ///
    /// # Errors
    ///
    /// Returns an error if the flush operation fails.
    pub async fn flush(&self) -> Result<()> {
        self.inner.flush().await
    }

    /// Acquires an exclusive lock on the file.
    ///
    /// Blocks until the lock can be acquired.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock cannot be acquired.
    pub async fn lock(&self) -> Result<()> {
        self.inner.lock().await
    }

    /// Acquires a shared (non-exclusive) lock on the file.
    ///
    /// Blocks until the lock can be acquired.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock cannot be acquired.
    pub async fn lock_shared(&self) -> Result<()> {
        self.inner.lock_shared().await
    }

    /// Tries to acquire an exclusive lock on the file.
    ///
    /// Returns `Err(TryLockError::WouldBlock)` if a different lock is already held.
    ///
    /// # Errors
    ///
    /// Returns [`std::fs::TryLockError::WouldBlock`] if the lock is held by another
    /// process, or [`std::fs::TryLockError::Error`] if the operation fails.
    pub async fn try_lock(&self) -> core::result::Result<(), TryLockError> {
        self.inner.try_lock().await
    }

    /// Tries to acquire a shared lock on the file.
    ///
    /// Returns `Err(TryLockError::WouldBlock)` if an exclusive lock is already held.
    ///
    /// # Errors
    ///
    /// Returns [`std::fs::TryLockError::WouldBlock`] if an exclusive lock is held
    /// by another process, or [`std::fs::TryLockError::Error`] if the operation fails.
    pub async fn try_lock_shared(&self) -> core::result::Result<(), TryLockError> {
        self.inner.try_lock_shared().await
    }

    /// Releases all locks on the file.
    ///
    /// # Errors
    ///
    /// Returns an error if the unlock operation fails.
    pub async fn unlock(&self) -> Result<()> {
        self.inner.unlock().await
    }

    /// Creates a new `WriteOnlyPositionalFile` instance that shares the same
    /// underlying file handle.
    ///
    /// # Errors
    ///
    /// Returns an error if the file handle cannot be cloned.
    pub async fn try_clone(&self) -> Result<Self> {
        Ok(Self {
            inner: self.inner.try_clone().await?,
        })
    }

    /// Returns `true` if the underlying file descriptor refers to a terminal.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.inner.is_terminal()
    }
}

impl HasMemory for WriteOnlyPositionalFile {
    fn memory(&self) -> impl MemoryShared {
        self.inner.memory().clone()
    }
}

impl Memory for WriteOnlyPositionalFile {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.inner.memory().reserve(min_bytes)
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsRawFd for WriteOnlyPositionalFile {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsFd for WriteOnlyPositionalFile {
    fn as_fd(&self) -> std::os::unix::io::BorrowedFd<'_> {
        // SAFETY: The file descriptor is valid for the lifetime of &self
        // because self holds an Arc<RwLock<File>> that keeps the fd open.
        unsafe { std::os::unix::io::BorrowedFd::borrow_raw(std::os::unix::io::AsRawFd::as_raw_fd(self)) }
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsRawHandle for WriteOnlyPositionalFile {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        self.inner.as_raw_handle()
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsHandle for WriteOnlyPositionalFile {
    fn as_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        // SAFETY: The handle is valid for the lifetime of &self
        // because self holds an Arc<RwLock<File>> that keeps the handle open.
        unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(std::os::windows::io::AsRawHandle::as_raw_handle(self)) }
    }
}

impl From<crate::positional_file::PositionalFile> for WriteOnlyPositionalFile {
    /// Converts a [`PositionalFile`](crate::PositionalFile) into a
    /// `WriteOnlyPositionalFile`, narrowing the capability to write-only
    /// positional access.
    fn from(file: crate::positional_file::PositionalFile) -> Self {
        Self { inner: file.into_inner() }
    }
}
