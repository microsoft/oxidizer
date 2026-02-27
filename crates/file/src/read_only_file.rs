// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::mem::MaybeUninit;
use std::fs::{Metadata, TryLockError};
use std::io::{Error, Result, SeekFrom};
use std::path::Path;

use bytesbuf::mem::{HasMemory, Memory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};

use crate::directory::Directory;
use crate::file_inner::FileInner;
use crate::shared_memory::SharedMemory;

/// A seekable read-only file handle within a capability-based filesystem.
///
/// A `ReadOnlyFile` provides read access to a file. It implements
/// [`bytesbuf_io::Read`] for streaming reads using managed buffers.
///
/// Obtain a `ReadOnlyFile` by calling [`ReadOnlyFile::open`].
#[derive(Debug)]
pub struct ReadOnlyFile {
    inner: FileInner,
}

impl ReadOnlyFile {
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
            inner: FileInner::open_readonly(dir, path, SharedMemory::global()).await?,
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
            inner: FileInner::open_readonly(dir, path, SharedMemory::new(memory)).await?,
        })
    }

    /// Reads up to `len` bytes from the current position, making a best effort
    /// to return the full amount.
    ///
    /// Performs multiple reads as necessary. May return fewer bytes only when
    /// EOF is reached before `len` bytes are available.
    ///
    /// # Errors
    ///
    /// Returns an error if a read operation fails due to an I/O error.
    #[inline]
    pub async fn read(&mut self, len: usize) -> Result<BytesView> {
        self.inner.read_into_bytesview(len).await
    }

    /// Reads at most `len` bytes from the current position in a single
    /// operation.
    ///
    /// May return fewer bytes than requested. A return of zero bytes indicates
    /// EOF.
    ///
    /// # Errors
    ///
    /// Returns an error if the read operation fails due to an I/O error.
    #[inline]
    pub async fn read_max(&mut self, len: usize) -> Result<BytesView> {
        self.inner.read_max_into_bytesview(len).await
    }

    /// Reads exactly `len` bytes from the current position.
    ///
    /// Performs multiple reads as necessary. Returns an error if EOF is
    /// reached before `len` bytes are read.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::ErrorKind::UnexpectedEof`] if the file ends before `len`
    /// bytes are read, or another error on I/O failure.
    #[inline]
    pub async fn read_exact(&mut self, len: usize) -> Result<BytesView> {
        self.inner.read_exact_into_bytesview(len).await
    }

    /// Reads an implementation-chosen number of bytes into the provided buffer.
    ///
    /// Returns the number of bytes read and the updated buffer. A return of
    /// 0 bytes indicates EOF.
    ///
    /// # Errors
    ///
    /// Returns an error if the read operation fails due to an I/O error.
    #[inline]
    pub async fn read_into_bytesbuf(&mut self, buf: &mut BytesBuf) -> Result<usize> {
        self.inner.read_into_bytesbuf(buf).await
    }

    /// Reads at most `len` bytes into the provided buffer in a single
    /// operation.
    ///
    /// Returns the number of bytes read and the updated buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the read operation fails due to an I/O error.
    #[inline]
    pub async fn read_max_into_bytesbuf(&mut self, len: usize, buf: &mut BytesBuf) -> Result<usize> {
        self.inner.read_max_into_bytesbuf(len, buf).await
    }

    /// Reads exactly `len` bytes into the provided buffer.
    ///
    /// Performs multiple reads as necessary. Returns an error if EOF is
    /// reached before `len` bytes are appended.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::ErrorKind::UnexpectedEof`] if the file ends before `len`
    /// bytes are read, or another error on I/O failure.
    #[inline]
    pub async fn read_exact_into_bytesbuf(&mut self, len: usize, buf: &mut BytesBuf) -> Result<()> {
        self.inner.read_exact_into_bytesbuf(len, buf).await
    }

    /// Reads into the provided slice, making a best effort to fill it
    /// completely.
    ///
    /// Returns the total number of bytes read. May return fewer than
    /// `buf.len()` only when EOF is reached.
    ///
    /// # Errors
    ///
    /// Returns an error if a read operation fails due to an I/O error.
    #[inline]
    pub async fn read_into_slice(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.read_into_slice(buf).await
    }

    /// Fills the provided slice with exactly `buf.len()` bytes.
    ///
    /// Performs multiple reads as necessary. Returns an error if EOF is
    /// reached before the slice is fully filled.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::ErrorKind::UnexpectedEof`] if the file ends before the
    /// slice is filled, or another error on I/O failure.
    #[inline]
    pub async fn read_exact_into_slice(&mut self, buf: &mut [u8]) -> Result<()> {
        self.inner.read_exact_into_slice(buf).await
    }

    /// Fills the provided uninitialized slice with exactly `buf.len()` bytes.
    ///
    /// On success every element in `buf` is initialized.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::ErrorKind::UnexpectedEof`] if the file ends before the slice
    /// is filled, or another error on I/O failure.
    #[inline]
    pub async fn read_exact_into_uninit(&mut self, buf: &mut [MaybeUninit<u8>]) -> Result<()> {
        self.inner.read_exact_into_uninit(buf).await
    }

    /// Queries metadata about the underlying file.
    ///
    /// # Errors
    ///
    /// Returns an error if the metadata cannot be retrieved due to an I/O error.
    #[inline]
    pub async fn metadata(&mut self) -> Result<Metadata> {
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
    pub async fn lock(&mut self) -> Result<()> {
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
    pub async fn lock_shared(&mut self) -> Result<()> {
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
    pub async fn try_lock(&mut self) -> core::result::Result<(), TryLockError> {
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
    pub async fn try_lock_shared(&mut self) -> core::result::Result<(), TryLockError> {
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
    pub async fn unlock(&mut self) -> Result<()> {
        self.inner.unlock().await
    }

    /// Seeks to a position in the file.
    ///
    /// The new position, measured in bytes from the start of the file, is
    /// returned.
    ///
    /// # Errors
    ///
    /// Returns an error if the seek operation fails due to an I/O error.
    #[inline]
    pub async fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.inner.seek(pos).await
    }

    /// Returns the current seek position from the start of the file.
    ///
    /// This is equivalent to `self.seek(SeekFrom::Current(0))`.
    ///
    /// # Errors
    ///
    /// Returns an error if the seek operation fails due to an I/O error.
    #[inline]
    pub async fn stream_position(&mut self) -> Result<u64> {
        self.inner.stream_position().await
    }

    /// Rewinds to the beginning of the file.
    ///
    /// This is equivalent to `self.seek(SeekFrom::Start(0))` but does not
    /// return the previous position.
    ///
    /// # Errors
    ///
    /// Returns an error if the seek operation fails due to an I/O error.
    #[inline]
    pub async fn rewind(&mut self) -> Result<()> {
        self.inner.rewind().await
    }

    /// Creates a new `ReadOnlyFile` instance that shares the same underlying
    /// file handle.
    ///
    /// Reads and seeks will affect both instances simultaneously.
    ///
    /// # Errors
    ///
    /// Returns an error if the clone operation fails due to an I/O error.
    #[inline]
    pub async fn try_clone(&mut self) -> Result<Self> {
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

impl HasMemory for ReadOnlyFile {
    fn memory(&self) -> impl MemoryShared {
        self.inner.memory().clone()
    }
}

impl Memory for ReadOnlyFile {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.inner.memory().reserve(min_bytes)
    }
}

impl bytesbuf_io::Read for ReadOnlyFile {
    type Error = Error;

    async fn read_at_most_into(&mut self, len: usize, mut into: BytesBuf) -> core::result::Result<(usize, BytesBuf), Self::Error> {
        let n = self.inner.read_max_into_bytesbuf(len, &mut into).await?;
        Ok((n, into))
    }

    async fn read_more_into(&mut self, mut into: BytesBuf) -> core::result::Result<(usize, BytesBuf), Self::Error> {
        let n = self.inner.read_into_bytesbuf(&mut into).await?;
        Ok((n, into))
    }

    async fn read_any(&mut self) -> core::result::Result<BytesBuf, Self::Error> {
        let mut buf = self.inner.memory().reserve(8192);
        let _ = self.inner.read_into_bytesbuf(&mut buf).await?;
        Ok(buf)
    }
}

#[cfg(feature = "sync-compat")]
impl std::io::Read for ReadOnlyFile {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.sync_read(buf)
    }
}

#[cfg(feature = "sync-compat")]
impl std::io::Seek for ReadOnlyFile {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.inner.sync_seek(pos)
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsRawFd for ReadOnlyFile {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsFd for ReadOnlyFile {
    fn as_fd(&self) -> std::os::unix::io::BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsRawHandle for ReadOnlyFile {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        self.inner.as_raw_handle()
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsHandle for ReadOnlyFile {
    fn as_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        self.inner.as_handle()
    }
}

impl From<crate::file::File> for ReadOnlyFile {
    /// Converts a [`File`](crate::File) into a `ReadOnlyFile`,
    /// narrowing the capability to read-only access.
    fn from(file: crate::file::File) -> Self {
        Self { inner: file.into_inner() }
    }
}
