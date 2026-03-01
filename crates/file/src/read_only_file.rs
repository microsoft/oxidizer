// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::mem::MaybeUninit;
use std::fs::{File, Metadata, TryLockError};
use std::io::{Error, ErrorKind, Result, SeekFrom};
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
    pub async fn open(dir: &Directory, path: impl AsRef<Path>) -> Result<Self> {
        let full_path = crate::path_utils::safe_join(dir.base_path(), path)?;
        let file = dir.dispatcher().dispatch(move || File::open(&full_path)).await?;
        Ok(Self {
            inner: FileInner::from_std(file, dir, SharedMemory::global()),
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
    pub async fn open_with_memory(dir: &Directory, path: impl AsRef<Path>, memory: impl MemoryShared) -> Result<Self> {
        let full_path = crate::path_utils::safe_join(dir.base_path(), path)?;
        let file = dir.dispatcher().dispatch(move || File::open(&full_path)).await?;
        Ok(Self {
            inner: FileInner::from_std(file, dir, SharedMemory::new(memory)),
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
    pub async fn read(&mut self, len: usize) -> Result<BytesView> {
        let mut buf = self.inner.memory().reserve(len);
        while buf.len() < len {
            let remaining = len - buf.len();
            let n = self.inner.read_max_into_bytebuf(remaining, &mut buf).await?;
            if n == 0 {
                break;
            }
        }
        Ok(buf.consume_all())
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
    pub async fn read_max(&mut self, len: usize) -> Result<BytesView> {
        let mut buf = self.inner.memory().reserve(len);
        let _ = self.inner.read_max_into_bytebuf(len, &mut buf).await?;
        Ok(buf.consume_all())
    }

    /// Reads exactly `len` bytes from the current position.
    ///
    /// Performs multiple reads as necessary. Returns an error if EOF is
    /// reached before `len` bytes are read.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::UnexpectedEof`] if the file ends before `len`
    /// bytes are read, or another error on I/O failure.
    pub async fn read_exact(&mut self, len: usize) -> Result<BytesView> {
        let mut buf = self.inner.memory().reserve(len);
        while buf.len() < len {
            let remaining = len - buf.len();
            let n = self.inner.read_max_into_bytebuf(remaining, &mut buf).await?;
            if n == 0 {
                return Err(Error::new(ErrorKind::UnexpectedEof, "failed to read exact number of bytes"));
            }
        }
        Ok(buf.consume_all())
    }

    /// Reads an implementation-chosen number of bytes into the provided buffer.
    ///
    /// Returns the number of bytes read and the updated buffer. A return of
    /// 0 bytes indicates EOF.
    ///
    /// # Errors
    ///
    /// Returns an error if the read operation fails due to an I/O error.
    pub async fn read_into_bytebuf(&mut self, buf: &mut BytesBuf) -> Result<usize> {
        self.inner.read_into_bytebuf(buf).await
    }

    /// Reads at most `len` bytes into the provided buffer in a single
    /// operation.
    ///
    /// Returns the number of bytes read and the updated buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the read operation fails due to an I/O error.
    pub async fn read_max_into_bytebuf(&mut self, len: usize, buf: &mut BytesBuf) -> Result<usize> {
        self.inner.read_max_into_bytebuf(len, buf).await
    }

    /// Reads exactly `len` bytes into the provided buffer.
    ///
    /// Performs multiple reads as necessary. Returns an error if EOF is
    /// reached before `len` bytes are appended.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::UnexpectedEof`] if the file ends before `len`
    /// bytes are read, or another error on I/O failure.
    pub async fn read_exact_into_bytebuf(&mut self, len: usize, buf: &mut BytesBuf) -> Result<()> {
        let start_len = buf.len();
        while buf.len() - start_len < len {
            let remaining = len - (buf.len() - start_len);
            let n = self.inner.read_max_into_bytebuf(remaining, buf).await?;
            if n == 0 {
                return Err(Error::new(ErrorKind::UnexpectedEof, "failed to read exact number of bytes"));
            }
        }
        Ok(())
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
    pub async fn read_into_slice(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.read_into_slice(buf).await
    }

    /// Reads at most `len` bytes into the provided slice in a single
    /// operation.
    ///
    /// # Panics
    ///
    /// Panics if `len > buf.len()`.
    ///
    /// # Errors
    ///
    /// Returns an error if the read operation fails due to an I/O error.
    pub async fn read_max_into_slice(&mut self, len: usize, buf: &mut [u8]) -> Result<usize> {
        assert!(len <= buf.len(), "len must not exceed buf.len()");
        self.inner.read_max_into_slice(&mut buf[..len]).await
    }

    /// Fills the provided slice with exactly `buf.len()` bytes.
    ///
    /// Performs multiple reads as necessary. Returns an error if EOF is
    /// reached before the slice is fully filled.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::UnexpectedEof`] if the file ends before the
    /// slice is filled, or another error on I/O failure.
    pub async fn read_exact_into_slice(&mut self, buf: &mut [u8]) -> Result<()> {
        self.inner.read_exact_into_slice(buf).await
    }

    /// Fills the provided uninitialized slice with exactly `buf.len()` bytes.
    ///
    /// On success every element in `buf` is initialized.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::UnexpectedEof`] if the file ends before the slice
    /// is filled, or another error on I/O failure.
    pub async fn read_exact_into_uninit(&mut self, buf: &mut [MaybeUninit<u8>]) -> Result<()> {
        // SAFETY: MaybeUninit<u8> has the same layout as u8.
        // read_exact_into_slice writes exactly buf.len() bytes on success,
        // fully initializing the contents.
        let initialized = unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), buf.len()) };
        self.read_exact_into_slice(initialized).await
    }

    /// Queries metadata about the underlying file.
    ///
    /// # Errors
    ///
    /// Returns an error if the metadata cannot be retrieved due to an I/O error.
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
    pub async fn unlock(&self) -> Result<()> {
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
        let n = self.inner.read_max_into_bytebuf(len, &mut into).await?;
        Ok((n, into))
    }

    async fn read_more_into(&mut self, mut into: BytesBuf) -> core::result::Result<(usize, BytesBuf), Self::Error> {
        let n = self.inner.read_into_bytebuf(&mut into).await?;
        Ok((n, into))
    }

    async fn read_any(&mut self) -> core::result::Result<BytesBuf, Self::Error> {
        let mut buf = self.inner.memory().reserve(8192);
        let _ = self.inner.read_into_bytebuf(&mut buf).await?;
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
        // SAFETY: The file descriptor is valid for the lifetime of &self
        // because self holds an Arc<RwLock<File>> that keeps the fd open.
        unsafe { std::os::unix::io::BorrowedFd::borrow_raw(std::os::unix::io::AsRawFd::as_raw_fd(self)) }
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
        // SAFETY: The handle is valid for the lifetime of &self
        // because self holds an Arc<RwLock<File>> that keeps the handle open.
        unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(std::os::windows::io::AsRawHandle::as_raw_handle(self)) }
    }
}

impl From<crate::file::File> for ReadOnlyFile {
    /// Converts a [`File`](crate::File) into a `ReadOnlyFile`,
    /// narrowing the capability to read-only access.
    fn from(file: crate::file::File) -> Self {
        Self { inner: file.into_inner() }
    }
}
