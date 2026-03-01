// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::mem::MaybeUninit;
use std::fs::{FileTimes, Metadata, Permissions, TryLockError};
use std::io::{Error, ErrorKind, Result};
use std::path::Path;

use bytesbuf::mem::{HasMemory, Memory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};

use crate::directory::Directory;
use crate::file_inner::FileInner;
use crate::positional_open_options::PositionalOpenOptions;
use crate::shared_memory::SharedMemory;

/// A positional read-write file handle within a capability-based filesystem.
///
/// All I/O methods take `&self` and operate at explicit byte offsets, enabling
/// concurrent access from multiple tasks without cursor management.
///
/// Obtain a `PositionalFile` by calling [`PositionalFile::open`],
/// [`PositionalFile::create`], [`PositionalFile::create_new`], or through
/// [`PositionalOpenOptions`].
#[derive(Debug)]
pub struct PositionalFile {
    inner: FileInner,
}

impl PositionalFile {
    pub(crate) const fn new(inner: FileInner) -> Self {
        Self { inner }
    }

    pub(crate) fn into_inner(self) -> FileInner {
        self.inner
    }

    /// Returns a new [`PositionalOpenOptions`] object.
    ///
    /// This allows opening a file with specific combinations of read, write,
    /// append, truncate, and create options for positional access.
    #[must_use]
    pub const fn options() -> PositionalOpenOptions {
        PositionalOpenOptions::new()
    }

    /// Opens an existing file in read-write mode for positional access.
    ///
    /// The path is relative to the given directory capability.
    ///
    /// # Errors
    ///
    /// Returns an error if the file does not exist, if the path escapes the
    /// directory capability, or on other I/O errors.
    pub async fn open(dir: &Directory, path: impl AsRef<Path>) -> Result<Self> {
        let full_path = crate::path_utils::safe_join(dir.base_path(), path)?;
        let file = dir
            .dispatcher()
            .dispatch(move || std::fs::OpenOptions::new().read(true).write(true).open(&full_path))
            .await?;
        Ok(Self {
            inner: FileInner::from_std(file, dir, SharedMemory::global()),
        })
    }

    /// Opens an existing file in read-write mode for positional access using the
    /// specified memory provider.
    ///
    /// The custom memory provider allows the caller to control buffer allocation,
    /// enabling zero-copy transfers with other subsystems sharing the same memory
    /// provider.
    ///
    /// # Errors
    ///
    /// Returns an error if the file does not exist, if the path escapes the
    /// directory capability, or on other I/O errors.
    pub async fn open_with_memory(dir: &Directory, path: impl AsRef<Path>, memory: impl MemoryShared) -> Result<Self> {
        let full_path = crate::path_utils::safe_join(dir.base_path(), path)?;
        let file = dir
            .dispatcher()
            .dispatch(move || std::fs::OpenOptions::new().read(true).write(true).open(&full_path))
            .await?;
        Ok(Self {
            inner: FileInner::from_std(file, dir, SharedMemory::new(memory)),
        })
    }

    /// Opens a file in read-write mode for positional access.
    ///
    /// This function will create the file if it does not exist, and will truncate
    /// it if it does.
    ///
    /// # Errors
    ///
    /// Returns an error if the path escapes the directory capability or on other
    /// I/O errors.
    pub async fn create(dir: &Directory, path: impl AsRef<Path>) -> Result<Self> {
        let full_path = crate::path_utils::safe_join(dir.base_path(), path)?;
        let file = dir
            .dispatcher()
            .dispatch(move || {
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&full_path)
            })
            .await?;
        Ok(Self {
            inner: FileInner::from_std(file, dir, SharedMemory::global()),
        })
    }

    /// Opens a file in read-write mode for positional access using the specified
    /// memory provider.
    ///
    /// Creates the file if it does not exist, truncates it if it does.
    ///
    /// # Errors
    ///
    /// Returns an error if the path escapes the directory capability or on other
    /// I/O errors.
    pub async fn create_with_memory(dir: &Directory, path: impl AsRef<Path>, memory: impl MemoryShared) -> Result<Self> {
        let full_path = crate::path_utils::safe_join(dir.base_path(), path)?;
        let file = dir
            .dispatcher()
            .dispatch(move || {
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&full_path)
            })
            .await?;
        Ok(Self {
            inner: FileInner::from_std(file, dir, SharedMemory::new(memory)),
        })
    }

    /// Creates a new file in read-write mode for positional access; returns an
    /// error if the file exists.
    ///
    /// If the call succeeds, the file is guaranteed to be new. This is atomic,
    /// avoiding TOCTOU race conditions.
    ///
    /// # Errors
    ///
    /// Returns an error if the file already exists, if the path escapes the
    /// directory capability, or on other I/O errors.
    pub async fn create_new(dir: &Directory, path: impl AsRef<Path>) -> Result<Self> {
        let full_path = crate::path_utils::safe_join(dir.base_path(), path)?;
        let file = dir
            .dispatcher()
            .dispatch(move || std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&full_path))
            .await?;
        Ok(Self {
            inner: FileInner::from_std(file, dir, SharedMemory::global()),
        })
    }

    /// Creates a new file in read-write mode for positional access using the
    /// specified memory provider; returns an error if the file exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the file already exists, if the path escapes the
    /// directory capability, or on other I/O errors.
    pub async fn create_new_with_memory(dir: &Directory, path: impl AsRef<Path>, memory: impl MemoryShared) -> Result<Self> {
        let full_path = crate::path_utils::safe_join(dir.base_path(), path)?;
        let file = dir
            .dispatcher()
            .dispatch(move || std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&full_path))
            .await?;
        Ok(Self {
            inner: FileInner::from_std(file, dir, SharedMemory::new(memory)),
        })
    }

    // ── Positional read methods ─────────────────────────────────────────

    /// Reads up to `len` bytes at `offset`, making a best effort to return
    /// the full amount.
    ///
    /// Performs multiple reads as necessary. May return fewer bytes only when
    /// EOF is reached before `len` bytes are available.
    ///
    /// # Errors
    ///
    /// Returns an error if a read operation fails due to an I/O error.
    pub async fn read_at(&self, offset: u64, len: usize) -> Result<BytesView> {
        self.inner.read_at(offset, len).await
    }

    /// Reads at most `len` bytes at `offset` in a single operation.
    ///
    /// May return fewer bytes than requested. A return of zero bytes indicates
    /// EOF.
    ///
    /// # Errors
    ///
    /// Returns an error if the read operation fails due to an I/O error.
    pub async fn read_max_at(&self, offset: u64, len: usize) -> Result<BytesView> {
        self.inner.read_max_at(offset, len).await
    }

    /// Reads exactly `len` bytes at `offset`.
    ///
    /// Performs multiple reads as necessary. Returns an error if EOF is
    /// reached before `len` bytes are read.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::UnexpectedEof`] if the file ends before `len`
    /// bytes are read, or another error on I/O failure.
    pub async fn read_exact_at(&self, offset: u64, len: usize) -> Result<BytesView> {
        self.inner.read_exact_at(offset, len).await
    }

    /// Reads an implementation-chosen number of bytes at `offset` into the
    /// provided buffer.
    ///
    /// Uses the buffer's remaining capacity (or 8192 if empty) to determine
    /// the read size. Returns the number of bytes read and the updated buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the read operation fails due to an I/O error.
    pub async fn read_into_bytebuf_at(&self, offset: u64, buf: &mut BytesBuf) -> Result<usize> {
        let len = if buf.remaining_capacity() > 0 {
            buf.remaining_capacity()
        } else {
            8192
        };
        self.inner.read_max_into_bytebuf_at(offset, len, buf).await
    }

    /// Reads at most `len` bytes at `offset` into the provided buffer in a
    /// single operation.
    ///
    /// Returns the number of bytes read and the updated buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the read operation fails due to an I/O error.
    pub async fn read_max_into_bytebuf_at(&self, offset: u64, len: usize, buf: &mut BytesBuf) -> Result<usize> {
        self.inner.read_max_into_bytebuf_at(offset, len, buf).await
    }

    /// Reads exactly `len` bytes at `offset` into the provided buffer.
    ///
    /// Performs multiple reads as necessary. Returns an error if EOF is
    /// reached before `len` bytes are appended.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::UnexpectedEof`] if the file ends before `len`
    /// bytes are read, or another error on I/O failure.
    pub async fn read_exact_into_bytebuf_at(&self, offset: u64, len: usize, buf: &mut BytesBuf) -> Result<()> {
        let start_len = buf.len();
        let mut current_offset = offset;
        while buf.len() - start_len < len {
            let remaining = len - (buf.len() - start_len);
            let n = self.inner.read_max_into_bytebuf_at(current_offset, remaining, buf).await?;
            if n == 0 {
                return Err(Error::new(ErrorKind::UnexpectedEof, "failed to read exact number of bytes"));
            }
            current_offset = offset.saturating_add((buf.len() - start_len) as u64);
        }
        Ok(())
    }

    /// Reads into the provided slice at `offset`, making a best effort to
    /// fill it completely.
    ///
    /// Returns the total number of bytes read. May return fewer than
    /// `buf.len()` only when EOF is reached.
    ///
    /// # Errors
    ///
    /// Returns an error if a read operation fails due to an I/O error.
    pub async fn read_into_slice_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.inner.read_into_slice_at(offset, buf).await
    }

    /// Reads at most `len` bytes at `offset` into the provided slice in a
    /// single operation.
    ///
    /// # Panics
    ///
    /// Panics if `len > buf.len()`.
    ///
    /// # Errors
    ///
    /// Returns an error if the read operation fails due to an I/O error.
    pub async fn read_max_into_slice_at(&self, offset: u64, len: usize, buf: &mut [u8]) -> Result<usize> {
        assert!(len <= buf.len(), "len must not exceed buf.len()");
        self.inner.read_max_into_slice_at(offset, &mut buf[..len]).await
    }

    /// Fills the provided slice with exactly `buf.len()` bytes starting at
    /// `offset`.
    ///
    /// Performs multiple reads as necessary. Returns an error if EOF is
    /// reached before the slice is fully filled.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::UnexpectedEof`] if the file ends before the
    /// slice is filled, or another error on I/O failure.
    pub async fn read_exact_into_slice_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.inner.read_exact_into_slice_at(offset, buf).await
    }

    /// Fills the provided uninitialized slice with exactly `buf.len()` bytes
    /// starting at `offset`.
    ///
    /// On success every element in `buf` is initialized.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::UnexpectedEof`] if the file ends before the slice
    /// is filled, or another error on I/O failure.
    pub async fn read_exact_into_uninit_at(&self, offset: u64, buf: &mut [MaybeUninit<u8>]) -> Result<()> {
        // SAFETY: MaybeUninit<u8> has the same layout as u8.
        // read_exact_into_slice_at writes exactly buf.len() bytes on success,
        // fully initializing the contents.
        let initialized = unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), buf.len()) };
        self.read_exact_into_slice_at(offset, initialized).await
    }

    // ── Positional write methods ────────────────────────────────────────

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

    // ── Metadata / control ──────────────────────────────────────────────

    /// Queries metadata about the underlying file.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn metadata(&self) -> Result<Metadata> {
        self.inner.metadata().await
    }

    /// Truncates or extends the underlying file, updating the size to become `size`.
    ///
    /// If `size` is less than the current file size, the file shrinks. If greater,
    /// it extends with zeroes.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn set_len(&self, size: u64) -> Result<()> {
        self.inner.set_len(size).await
    }

    /// Changes the modification time of the underlying file.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn set_modified(&self, modified: std::time::SystemTime) -> Result<()> {
        self.inner.set_modified(modified).await
    }

    /// Changes the permissions on the underlying file.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn set_permissions(&self, perms: Permissions) -> Result<()> {
        self.inner.set_permissions(perms).await
    }

    /// Changes the timestamps of the underlying file.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn set_times(&self, times: FileTimes) -> Result<()> {
        self.inner.set_times(times).await
    }

    /// Attempts to sync all OS-internal file content and metadata to disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn sync_all(&self) -> Result<()> {
        self.inner.sync_all().await
    }

    /// Similar to [`sync_all`](Self::sync_all), except that it might not synchronize
    /// file metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn sync_data(&self) -> Result<()> {
        self.inner.sync_data().await
    }

    /// Flushes any buffered data to the underlying file.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn flush(&self) -> Result<()> {
        self.inner.flush().await
    }

    /// Acquires an exclusive lock on the file.
    ///
    /// Blocks until the lock is acquired.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn lock(&self) -> Result<()> {
        self.inner.lock().await
    }

    /// Acquires a shared (non-exclusive) lock on the file.
    ///
    /// Blocks until the lock is acquired.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn lock_shared(&self) -> Result<()> {
        self.inner.lock_shared().await
    }

    /// Tries to acquire an exclusive lock on the file.
    ///
    /// Returns `Err(TryLockError::WouldBlock)` if another lock is held.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock cannot be acquired or on I/O failure.
    pub async fn try_lock(&self) -> core::result::Result<(), TryLockError> {
        self.inner.try_lock().await
    }

    /// Tries to acquire a shared lock on the file.
    ///
    /// Returns `Err(TryLockError::WouldBlock)` if an exclusive lock is held.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock cannot be acquired or on I/O failure.
    pub async fn try_lock_shared(&self) -> core::result::Result<(), TryLockError> {
        self.inner.try_lock_shared().await
    }

    /// Releases all locks on the file.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn unlock(&self) -> Result<()> {
        self.inner.unlock().await
    }

    /// Creates a new `PositionalFile` instance that shares the same underlying
    /// file handle.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
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

impl HasMemory for PositionalFile {
    fn memory(&self) -> impl MemoryShared {
        self.inner.memory().clone()
    }
}

impl Memory for PositionalFile {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.inner.memory().reserve(min_bytes)
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsRawFd for PositionalFile {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsFd for PositionalFile {
    fn as_fd(&self) -> std::os::unix::io::BorrowedFd<'_> {
        // SAFETY: The file descriptor is valid for the lifetime of &self
        // because self holds an Arc<RwLock<File>> that keeps the fd open.
        unsafe { std::os::unix::io::BorrowedFd::borrow_raw(std::os::unix::io::AsRawFd::as_raw_fd(self)) }
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsRawHandle for PositionalFile {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        self.inner.as_raw_handle()
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsHandle for PositionalFile {
    fn as_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        // SAFETY: The handle is valid for the lifetime of &self
        // because self holds an Arc<RwLock<File>> that keeps the handle open.
        unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(std::os::windows::io::AsRawHandle::as_raw_handle(self)) }
    }
}
