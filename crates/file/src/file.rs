// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::mem::MaybeUninit;
use std::fs::{FileTimes, Metadata, Permissions, TryLockError};
use std::io::{Error, ErrorKind, Result, SeekFrom};
use std::path::Path;

use bytesbuf::mem::{HasMemory, Memory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};

use crate::directory::Directory;
use crate::file_inner::FileInner;
use crate::open_options::OpenOptions;
use crate::shared_memory::SharedMemory;

/// A seekable read-write file handle within a capability-based filesystem.
///
/// A `File` provides both read and write access to a file. It implements
/// both [`bytesbuf_io::Read`] and [`bytesbuf_io::Write`] for streaming I/O using
/// managed buffers.
///
/// Obtain a `File` by calling [`File::open`], [`File::create`],
/// [`File::create_new`], or through [`OpenOptions`].
#[derive(Debug)]
pub struct File {
    inner: FileInner,
}

impl File {
    pub(crate) const fn new(inner: FileInner) -> Self {
        Self { inner }
    }

    pub(crate) fn into_inner(self) -> FileInner {
        self.inner
    }

    /// Returns a new [`OpenOptions`] object.
    ///
    /// This allows opening a file with specific combinations of read, write,
    /// append, truncate, and create options.
    #[must_use]
    pub const fn options() -> OpenOptions {
        OpenOptions::new()
    }

    /// Opens an existing file in read-write mode.
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

    /// Opens an existing file in read-write mode using the specified memory provider.
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

    /// Opens a file in read-write mode.
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

    /// Opens a file in read-write mode using the specified memory provider.
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

    /// Creates a new file in read-write mode; returns an error if the file exists.
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

    /// Creates a new file in read-write mode using the specified memory provider;
    /// returns an error if the file exists.
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
    /// Returns an error if the underlying I/O operation fails.
    pub async fn write(&mut self, data: BytesView) -> Result<()> {
        self.inner.write(data).await
    }

    /// Writes a byte slice to the file at the current cursor position.
    ///
    /// Convenience method for `&[u8]` callers. The data is copied internally;
    /// prefer [`write`](Self::write) with [`BytesView`] for large writes.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn write_slice(&mut self, data: impl AsRef<[u8]>) -> Result<()> {
        self.inner.write_slice(data.as_ref()).await
    }

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
    /// it extends with zeroes. The file cursor is not changed.
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

    /// Seeks to a position in the file.
    ///
    /// Returns the new position from the start of the file.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.inner.seek(pos).await
    }

    /// Returns the current seek position from the start of the file.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn stream_position(&mut self) -> Result<u64> {
        self.inner.stream_position().await
    }

    /// Rewinds to the beginning of the file.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying I/O operation fails.
    pub async fn rewind(&mut self) -> Result<()> {
        self.inner.rewind().await
    }

    /// Creates a new `File` instance that shares the same underlying file handle.
    ///
    /// Reads, writes, and seeks will affect both instances simultaneously.
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

impl HasMemory for File {
    fn memory(&self) -> impl MemoryShared {
        self.inner.memory().clone()
    }
}

impl Memory for File {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.inner.memory().reserve(min_bytes)
    }
}

impl bytesbuf_io::Read for File {
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

impl bytesbuf_io::Write for File {
    type Error = Error;

    async fn write(&mut self, data: BytesView) -> core::result::Result<(), Self::Error> {
        Self::write(self, data).await
    }
}

#[cfg(feature = "sync-compat")]
impl std::io::Read for File {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.sync_read(buf)
    }
}

#[cfg(feature = "sync-compat")]
impl std::io::Write for File {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.inner.sync_write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        self.inner.sync_flush()
    }
}

#[cfg(feature = "sync-compat")]
impl std::io::Seek for File {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.inner.sync_seek(pos)
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsRawFd for File {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsFd for File {
    fn as_fd(&self) -> std::os::unix::io::BorrowedFd<'_> {
        // SAFETY: The file descriptor is valid for the lifetime of &self
        // because self holds an Arc<RwLock<File>> that keeps the fd open.
        unsafe { std::os::unix::io::BorrowedFd::borrow_raw(std::os::unix::io::AsRawFd::as_raw_fd(self)) }
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsRawHandle for File {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        self.inner.as_raw_handle()
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsHandle for File {
    fn as_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        // SAFETY: The handle is valid for the lifetime of &self
        // because self holds an Arc<RwLock<File>> that keeps the handle open.
        unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(std::os::windows::io::AsRawHandle::as_raw_handle(self)) }
    }
}
