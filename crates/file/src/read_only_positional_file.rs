// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::mem::MaybeUninit;
use std::fs::{Metadata, TryLockError};
use std::io::Result;
use std::path::Path;

use bytesbuf::mem::{HasMemory, Memory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};

use crate::directory::Directory;
use crate::positional_file_inner::PositionalFileInner;
use crate::shared_memory::SharedMemory;

/// A positional read-only file handle within a capability-based filesystem.
///
/// All I/O methods take `&self` and operate at explicit byte offsets, enabling
/// concurrent access from multiple tasks without cursor management.
///
/// Obtain a `ReadOnlyPositionalFile` by calling [`ReadOnlyPositionalFile::open`],
/// or by narrowing a [`PositionalFile`](crate::PositionalFile) via [`From`].
#[derive(Debug)]
pub struct ReadOnlyPositionalFile {
    inner: PositionalFileInner,
}

impl ReadOnlyPositionalFile {
    /// Attempts to open a file in read-only mode.
    ///
    /// The path is relative to the given directory capability.
    ///
    /// # Errors
    ///
    /// Returns an error if the path does not exist, if the path escapes the
    /// directory capability, or due to other I/O errors.
    #[inline]
    pub async fn open(dir: &Directory, path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            inner: PositionalFileInner::open_readonly(dir, path, SharedMemory::global()).await?,
        })
    }

    /// Attempts to open a file in read-only mode using the specified memory provider.
    ///
    /// This allows the caller to control buffer allocation, enabling zero-copy
    /// transfers to other subsystems that share the same memory provider.
    ///
    /// # Errors
    ///
    /// Returns an error if the path does not exist, if the path escapes the
    /// directory capability, or due to other I/O errors.
    #[inline]
    pub async fn open_with_memory(dir: &Directory, path: impl AsRef<Path>, memory: impl MemoryShared) -> Result<Self> {
        Ok(Self {
            inner: PositionalFileInner::open_readonly(dir, path, SharedMemory::new(memory)).await?,
        })
    }

    /// Reads up to `len` bytes at `offset`, making a best effort to return
    /// the full amount.
    ///
    /// Performs multiple reads as necessary. May return fewer bytes only when
    /// EOF is reached before `len` bytes are available.
    ///
    /// # Errors
    ///
    /// Returns an error if a read operation fails due to an I/O error.
    #[inline]
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
    #[inline]
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
    /// Returns [`std::io::ErrorKind::UnexpectedEof`] if the file ends before `len`
    /// bytes are read, or another error on I/O failure.
    #[inline]
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
    #[inline]
    pub async fn read_into_bytesbuf_at(&self, offset: u64, buf: &mut BytesBuf) -> Result<usize> {
        let len = if buf.remaining_capacity() > 0 {
            buf.remaining_capacity()
        } else {
            8192
        };
        self.inner.read_max_into_bytesbuf_at(offset, len, buf).await
    }

    /// Reads at most `len` bytes at `offset` into the provided buffer in a
    /// single operation.
    ///
    /// Returns the number of bytes read and the updated buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the read operation fails due to an I/O error.
    #[inline]
    pub async fn read_max_into_bytesbuf_at(&self, offset: u64, len: usize, buf: &mut BytesBuf) -> Result<usize> {
        self.inner.read_max_into_bytesbuf_at(offset, len, buf).await
    }

    /// Reads exactly `len` bytes at `offset` into the provided buffer.
    ///
    /// Performs multiple reads as necessary. Returns an error if EOF is
    /// reached before `len` bytes are appended.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::ErrorKind::UnexpectedEof`] if the file ends before `len`
    /// bytes are read, or another error on I/O failure.
    #[inline]
    pub async fn read_exact_into_bytesbuf_at(&self, offset: u64, len: usize, buf: &mut BytesBuf) -> Result<()> {
        self.inner.read_exact_into_bytesbuf_at(offset, len, buf).await
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
    #[inline]
    pub async fn read_into_slice_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.inner.read_into_slice_at(offset, buf).await
    }

    /// Fills the provided slice with exactly `buf.len()` bytes starting at
    /// `offset`.
    ///
    /// Performs multiple reads as necessary. Returns an error if EOF is
    /// reached before the slice is fully filled.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::ErrorKind::UnexpectedEof`] if the file ends before the
    /// slice is filled, or another error on I/O failure.
    #[inline]
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
    /// Returns [`std::io::ErrorKind::UnexpectedEof`] if the file ends before the slice
    /// is filled, or another error on I/O failure.
    #[inline]
    pub async fn read_exact_into_uninit_at(&self, offset: u64, buf: &mut [MaybeUninit<u8>]) -> Result<()> {
        self.inner.read_exact_into_uninit_at(offset, buf).await
    }

    /// Queries metadata about the underlying file.
    ///
    /// # Errors
    ///
    /// Returns an error if the metadata cannot be retrieved due to an I/O error.
    #[inline]
    pub async fn metadata(&self) -> Result<Metadata> {
        self.inner.metadata().await
    }

    /// Acquires an exclusive lock on the file.
    ///
    /// Blocks until the lock can be acquired. No other file handle to this file
    /// may acquire another lock while this lock is held.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock cannot be acquired due to an I/O error.
    #[inline]
    pub async fn lock(&self) -> Result<()> {
        self.inner.lock().await
    }

    /// Acquires a shared (non-exclusive) lock on the file.
    ///
    /// Blocks until the lock can be acquired. More than one handle may hold a
    /// shared lock, but none may hold an exclusive lock at the same time.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock cannot be acquired due to an I/O error.
    #[inline]
    pub async fn lock_shared(&self) -> Result<()> {
        self.inner.lock_shared().await
    }

    /// Tries to acquire an exclusive lock on the file.
    ///
    /// Returns `Err(TryLockError::WouldBlock)` if a different lock is already
    /// held.
    ///
    /// # Errors
    ///
    /// Returns [`std::fs::TryLockError::WouldBlock`] if the lock is already
    /// held, or [`std::fs::TryLockError::Error`] for other I/O errors.
    #[inline]
    pub async fn try_lock(&self) -> core::result::Result<(), TryLockError> {
        self.inner.try_lock().await
    }

    /// Tries to acquire a shared lock on the file.
    ///
    /// Returns `Err(TryLockError::WouldBlock)` if an exclusive lock is already
    /// held.
    ///
    /// # Errors
    ///
    /// Returns [`std::fs::TryLockError::WouldBlock`] if an exclusive lock is
    /// already held, or [`std::fs::TryLockError::Error`] for other I/O errors.
    #[inline]
    pub async fn try_lock_shared(&self) -> core::result::Result<(), TryLockError> {
        self.inner.try_lock_shared().await
    }

    /// Releases all locks on the file.
    ///
    /// All locks are also released when the file is closed.
    ///
    /// # Errors
    ///
    /// Returns an error if the unlock operation fails due to an I/O error.
    #[inline]
    pub async fn unlock(&self) -> Result<()> {
        self.inner.unlock().await
    }

    /// Creates a new `ReadOnlyPositionalFile` instance that shares the same
    /// underlying file handle.
    ///
    /// # Errors
    ///
    /// Returns an error if the clone operation fails due to an I/O error.
    #[inline]
    pub async fn try_clone(&self) -> Result<Self> {
        Ok(Self {
            inner: self.inner.try_clone().await?,
        })
    }

    /// Returns `true` if the underlying file descriptor refers to a terminal.
    #[must_use]
    #[inline]
    pub fn is_terminal(&self) -> bool {
        self.inner.is_terminal()
    }
}

impl HasMemory for ReadOnlyPositionalFile {
    fn memory(&self) -> impl MemoryShared {
        self.inner.memory().clone()
    }
}

impl Memory for ReadOnlyPositionalFile {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.inner.memory().reserve(min_bytes)
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsRawFd for ReadOnlyPositionalFile {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsFd for ReadOnlyPositionalFile {
    fn as_fd(&self) -> std::os::unix::io::BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsRawHandle for ReadOnlyPositionalFile {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        self.inner.as_raw_handle()
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsHandle for ReadOnlyPositionalFile {
    fn as_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        self.inner.as_handle()
    }
}

impl From<crate::positional_file::PositionalFile> for ReadOnlyPositionalFile {
    /// Converts a [`PositionalFile`](crate::PositionalFile) into a
    /// `ReadOnlyPositionalFile`, narrowing the capability to read-only
    /// positional access.
    fn from(file: crate::positional_file::PositionalFile) -> Self {
        Self { inner: file.into_inner() }
    }
}
