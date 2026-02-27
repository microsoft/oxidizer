// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs::{File, FileTimes, Metadata, Permissions, TryLockError};
use std::io::{Error, Result, SeekFrom};
use std::path::Path;

use bytesbuf::mem::{HasMemory, Memory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};

use crate::directory::Directory;
use crate::file_inner::FileInner;
use crate::shared_memory::SharedMemory;

/// A seekable write-only file handle within a capability-based filesystem.
///
/// A `WriteOnlyFile` provides write access to a file. It implements
/// [`bytesbuf_io::Write`] for streaming writes using managed buffers.
///
/// Obtain a `WriteOnlyFile` by calling [`WriteOnlyFile::create`] or
/// [`WriteOnlyFile::create_new`].
#[derive(Debug)]
pub struct WriteOnlyFile {
    inner: FileInner,
}

impl WriteOnlyFile {
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
    /// Combines the atomicity guarantees of [`WriteOnlyFile::create_new`] with a
    /// custom memory provider for zero-copy transfers.
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

    /// Writes the provided byte sequence to the file.
    ///
    /// The method completes when all bytes have been written. Partial writes are
    /// considered a failure.
    ///
    /// For optimal efficiency, the data should originate from buffers allocated via
    /// this file's memory provider (see [`Memory::reserve`] or [`HasMemory::memory`]).
    ///
    /// # Errors
    ///
    /// Returns an error if the write operation fails.
    pub async fn write(&mut self, data: BytesView) -> Result<()> {
        self.inner.write(data).await
    }

    /// Writes a byte slice to the file at the current cursor position.
    ///
    /// This is a convenience method for callers working with `&[u8]` rather than
    /// managed buffers. The data is copied internally to transfer it to the
    /// worker thread; prefer [`write`](Self::write) with [`BytesView`] for
    /// large or performance-sensitive writes.
    ///
    /// # Errors
    ///
    /// Returns an error if the write operation fails.
    pub async fn write_slice(&mut self, data: impl AsRef<[u8]>) -> Result<()> {
        self.inner.write_slice(data.as_ref()).await
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
    /// If it is greater, the file will be extended with zeroes. The file's cursor
    /// isn't changed.
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

    /// Similar to [`WriteOnlyFile::sync_all`], except that it might not synchronize
    /// file metadata to the filesystem.
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

    /// Seeks to a position in the file.
    ///
    /// The new position, measured in bytes from the start of the file, is returned.
    ///
    /// # Errors
    ///
    /// Returns an error if the seek operation fails.
    pub async fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.inner.seek(pos).await
    }

    /// Returns the current seek position from the start of the file.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub async fn stream_position(&mut self) -> Result<u64> {
        self.inner.stream_position().await
    }

    /// Rewinds to the beginning of the file.
    ///
    /// # Errors
    ///
    /// Returns an error if the seek operation fails.
    pub async fn rewind(&mut self) -> Result<()> {
        self.inner.rewind().await
    }

    /// Creates a new `WriteOnlyFile` instance that shares the same underlying
    /// file handle.
    ///
    /// Writes and seeks will affect both instances simultaneously.
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

impl HasMemory for WriteOnlyFile {
    fn memory(&self) -> impl MemoryShared {
        self.inner.memory().clone()
    }
}

impl Memory for WriteOnlyFile {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.inner.memory().reserve(min_bytes)
    }
}

impl bytesbuf_io::Write for WriteOnlyFile {
    type Error = Error;

    async fn write(&mut self, data: BytesView) -> core::result::Result<(), Self::Error> {
        Self::write(self, data).await
    }
}

#[cfg(feature = "sync-compat")]
impl std::io::Write for WriteOnlyFile {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.inner.sync_write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        self.inner.sync_flush()
    }
}

#[cfg(feature = "sync-compat")]
impl std::io::Seek for WriteOnlyFile {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.inner.sync_seek(pos)
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsRawFd for WriteOnlyFile {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsFd for WriteOnlyFile {
    fn as_fd(&self) -> std::os::unix::io::BorrowedFd<'_> {
        // SAFETY: The file descriptor is valid for the lifetime of &self
        // because self holds an Arc<RwLock<File>> that keeps the fd open.
        unsafe { std::os::unix::io::BorrowedFd::borrow_raw(std::os::unix::io::AsRawFd::as_raw_fd(self)) }
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsRawHandle for WriteOnlyFile {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        self.inner.as_raw_handle()
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsHandle for WriteOnlyFile {
    fn as_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        // SAFETY: The handle is valid for the lifetime of &self
        // because self holds an Arc<RwLock<File>> that keeps the handle open.
        unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(std::os::windows::io::AsRawHandle::as_raw_handle(self)) }
    }
}

impl From<crate::file::File> for WriteOnlyFile {
    /// Converts a [`File`](crate::File) into a `WriteOnlyFile`,
    /// narrowing the capability to write-only access.
    fn from(file: crate::file::File) -> Self {
        Self { inner: file.into_inner() }
    }
}
