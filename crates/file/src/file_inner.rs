// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs::{File, FileTimes, Metadata, Permissions, TryLockError};
use std::io::{ErrorKind, Read, Result, Seek as _, SeekFrom, Write as _};

use bytesbuf::mem::Memory;
use bytesbuf::{BytesBuf, BytesView};
use sync_thunk::{Thunker, thunk};

use crate::io_helpers::read_into_bytesbuf;
use crate::shared_memory::SharedMemory;

const DEFAULT_READ_SIZE: usize = 8192;

#[derive(Debug)]
pub struct FileInner {
    file: File,
    thunker: Thunker,
    memory: SharedMemory,
}

impl FileInner {
    /// Creates a `SeekableFileInner` from a standard `std::fs::File`.
    pub fn from_std(file: File, dir: &crate::directory::Directory, memory: SharedMemory) -> Self {
        Self {
            file,
            thunker: dir.thunker().clone(),
            memory,
        }
    }

    /// Opens a file with the given options, dispatching the blocking open to a worker thread.
    pub async fn open_file(
        dir: &crate::directory::Directory,
        path: impl AsRef<std::path::Path>,
        memory: SharedMemory,
        opts: std::fs::OpenOptions,
    ) -> Result<Self> {
        let file = dir.open_std_file(path.as_ref(), opts).await?;
        Ok(Self::from_std(file, dir, memory))
    }

    /// Opens a file in read-only mode.
    pub async fn open_readonly(dir: &crate::directory::Directory, path: impl AsRef<std::path::Path>, memory: SharedMemory) -> Result<Self> {
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true);
        Self::open_file(dir, path, memory, opts).await
    }

    /// Creates (or truncates) a file in write-only mode.
    pub async fn create_writeonly(
        dir: &crate::directory::Directory,
        path: impl AsRef<std::path::Path>,
        memory: SharedMemory,
    ) -> Result<Self> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        Self::open_file(dir, path, memory, opts).await
    }

    /// Atomically creates a new file in write-only mode; fails if it exists.
    pub async fn create_new_writeonly(
        dir: &crate::directory::Directory,
        path: impl AsRef<std::path::Path>,
        memory: SharedMemory,
    ) -> Result<Self> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        Self::open_file(dir, path, memory, opts).await
    }

    /// Opens an existing file in read-write mode.
    pub async fn open_readwrite(
        dir: &crate::directory::Directory,
        path: impl AsRef<std::path::Path>,
        memory: SharedMemory,
    ) -> Result<Self> {
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true).write(true);
        Self::open_file(dir, path, memory, opts).await
    }

    /// Creates (or truncates) a file in read-write mode.
    pub async fn create_readwrite(
        dir: &crate::directory::Directory,
        path: impl AsRef<std::path::Path>,
        memory: SharedMemory,
    ) -> Result<Self> {
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(true);
        Self::open_file(dir, path, memory, opts).await
    }

    /// Atomically creates a new file in read-write mode; fails if it exists.
    pub async fn create_new_readwrite(
        dir: &crate::directory::Directory,
        path: impl AsRef<std::path::Path>,
        memory: SharedMemory,
    ) -> Result<Self> {
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true).write(true).create_new(true);
        Self::open_file(dir, path, memory, opts).await
    }

    pub const fn memory(&self) -> &SharedMemory {
        &self.memory
    }

    /// Returns the raw file descriptor (Unix).
    #[cfg(unix)]
    pub fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        use std::os::unix::io::AsRawFd;
        self.file.as_raw_fd()
    }

    /// Returns a borrowed file descriptor.
    #[cfg(unix)]
    pub fn as_fd(&self) -> std::os::unix::io::BorrowedFd<'_> {
        use std::os::unix::io::AsFd;
        self.file.as_fd()
    }

    /// Returns the raw handle (Windows).
    #[cfg(windows)]
    pub fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        use std::os::windows::io::AsRawHandle;
        self.file.as_raw_handle()
    }

    /// Returns a borrowed handle.
    #[cfg(windows)]
    pub fn as_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        use std::os::windows::io::AsHandle;
        self.file.as_handle()
    }

    /// Returns whether the underlying file descriptor refers to a terminal.
    pub fn is_terminal(&self) -> bool {
        use std::io::IsTerminal;
        self.file.is_terminal()
    }

    /// Synchronous read, bypassing the worker pool.
    #[cfg(feature = "sync-compat")]
    pub fn sync_read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.file.read(buf)
    }

    /// Synchronous write, bypassing the worker pool.
    #[cfg(feature = "sync-compat")]
    pub fn sync_write(&mut self, buf: &[u8]) -> Result<usize> {
        use std::io::Write;
        self.file.write(buf)
    }

    /// Synchronous flush, bypassing the worker pool.
    #[cfg(feature = "sync-compat")]
    pub fn sync_flush(&mut self) -> Result<()> {
        use std::io::Write;
        self.file.flush()
    }

    /// Synchronous seek, bypassing the worker pool.
    #[cfg(feature = "sync-compat")]
    pub fn sync_seek(&mut self, pos: SeekFrom) -> Result<u64> {
        use std::io::Seek;
        self.file.seek(pos)
    }

    #[thunk(from = self.thunker)]
    pub async fn read_into_bytesview(&mut self, len: usize) -> Result<BytesView> {
        let mut buf = self.memory.reserve(len);
        read_bytesbuf_best_effort(&mut self.file, &mut buf, len)?;
        Ok(buf.consume_all())
    }

    #[thunk(from = self.thunker)]
    pub async fn read_exact_into_bytesview(&mut self, len: usize) -> Result<BytesView> {
        let mut buf = self.memory.reserve(len);
        read_bytesbuf_exact(&mut self.file, &mut buf, len)?;
        Ok(buf.consume_all())
    }

    #[thunk(from = self.thunker)]
    pub async fn read_max_into_bytesview(&mut self, len: usize) -> Result<BytesView> {
        let mut buf = self.memory.reserve(len);
        read_into_bytesbuf(&mut self.file, &mut buf, len)?;
        Ok(buf.consume_all())
    }

    #[thunk(from = self.thunker)]
    pub async fn read_exact_into_bytesbuf(&mut self, len: usize, buf: &mut BytesBuf) -> Result<()> {
        if buf.remaining_capacity() < len {
            buf.reserve(len - buf.remaining_capacity(), &self.memory);
        }
        read_bytesbuf_exact(&mut self.file, buf, len)?;
        Ok(())
    }

    #[thunk(from = self.thunker)]
    pub async fn read_max_into_bytesbuf(&mut self, len: usize, into: &mut BytesBuf) -> Result<usize> {
        let needed = len.saturating_sub(into.remaining_capacity());
        if needed > 0 {
            into.reserve(needed, &self.memory);
        }
        read_into_bytesbuf(&mut self.file, into, len)
    }

    pub async fn read_into_bytesbuf(&mut self, into: &mut BytesBuf) -> Result<usize> {
        self.read_max_into_bytesbuf(DEFAULT_READ_SIZE, into).await
    }

    #[thunk(from = self.thunker)]
    pub async fn read_into_slice(&mut self, buf: &mut [u8]) -> Result<usize> {
        read_slice_best_effort(&mut self.file, buf)
    }

    pub async fn read_exact_into_uninit(&mut self, buf: &mut [core::mem::MaybeUninit<u8>]) -> Result<()> {
        // SAFETY: MaybeUninit<u8> has the same layout as u8.
        // read_exact_into_slice writes exactly buf.len() bytes on success,
        // fully initializing the contents.
        let initialized = unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), buf.len()) };
        self.read_exact_into_slice(initialized).await
    }

    #[thunk(from = self.thunker)]
    pub async fn read_exact_into_slice(&mut self, buf: &mut [u8]) -> Result<()> {
        self.file.read_exact(buf)
    }

    #[thunk(from = self.thunker)]
    pub async fn write(&mut self, data: BytesView) -> Result<()> {
        write_all_bytesview(&mut self.file, &data)
    }

    #[thunk(from = self.thunker)]
    pub async fn write_slice(&mut self, data: &[u8]) -> Result<()> {
        self.file.write_all(data)
    }

    #[thunk(from = self.thunker)]
    pub async fn metadata(&mut self) -> Result<Metadata> {
        self.file.metadata()
    }

    #[thunk(from = self.thunker)]
    pub async fn set_len(&mut self, size: u64) -> Result<()> {
        self.file.set_len(size)
    }

    #[thunk(from = self.thunker)]
    pub async fn set_modified(&mut self, modified: std::time::SystemTime) -> Result<()> {
        self.file.set_modified(modified)
    }

    #[thunk(from = self.thunker)]
    pub async fn set_permissions(&mut self, perms: Permissions) -> Result<()> {
        self.file.set_permissions(perms)
    }

    #[thunk(from = self.thunker)]
    pub async fn set_times(&mut self, times: FileTimes) -> Result<()> {
        self.file.set_times(times)
    }

    #[thunk(from = self.thunker)]
    pub async fn sync_all(&mut self) -> Result<()> {
        self.file.sync_all()
    }

    #[thunk(from = self.thunker)]
    pub async fn sync_data(&mut self) -> Result<()> {
        self.file.sync_data()
    }

    #[thunk(from = self.thunker)]
    pub async fn flush(&mut self) -> Result<()> {
        self.file.flush()
    }

    #[thunk(from = self.thunker)]
    pub async fn lock(&mut self) -> Result<()> {
        self.file.lock()
    }

    #[thunk(from = self.thunker)]
    pub async fn lock_shared(&mut self) -> Result<()> {
        self.file.lock_shared()
    }

    #[thunk(from = self.thunker)]
    pub async fn try_lock(&mut self) -> core::result::Result<(), TryLockError> {
        self.file.try_lock()
    }

    #[thunk(from = self.thunker)]
    pub async fn try_lock_shared(&mut self) -> core::result::Result<(), TryLockError> {
        self.file.try_lock_shared()
    }

    #[thunk(from = self.thunker)]
    pub async fn unlock(&mut self) -> Result<()> {
        self.file.unlock()
    }

    #[thunk(from = self.thunker)]
    pub async fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.file.seek(pos)
    }

    #[thunk(from = self.thunker)]
    pub async fn stream_position(&mut self) -> Result<u64> {
        self.file.stream_position()
    }

    #[thunk(from = self.thunker)]
    pub async fn rewind(&mut self) -> Result<()> {
        self.file.rewind()
    }

    #[thunk(from = self.thunker)]
    pub async fn try_clone(&mut self) -> Result<Self> {
        let new_file = self.file.try_clone()?;
        Ok(Self {
            file: new_file,
            thunker: self.thunker.clone(),
            memory: self.memory.clone(),
        })
    }
}

/// Reads into a `BytesBuf` in a loop until `len` bytes are read or EOF.
fn read_bytesbuf_best_effort(reader: &mut impl Read, buf: &mut BytesBuf, len: usize) -> Result<usize> {
    let mut total = 0;
    while total < len {
        let n = read_into_bytesbuf(reader, buf, len - total)?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}

/// Reads into a `BytesBuf` in a loop until exactly `len` bytes are read; returns
/// `UnexpectedEof` on premature EOF.
fn read_bytesbuf_exact(reader: &mut impl Read, buf: &mut BytesBuf, len: usize) -> Result<usize> {
    let start = buf.len();
    while buf.len() - start < len {
        let remaining = len - (buf.len() - start);
        let n = read_into_bytesbuf(reader, buf, remaining)?;
        if n == 0 {
            return Err(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                "failed to read exact number of bytes",
            ));
        }
    }
    Ok(len)
}

/// Reads into a slice in a loop until the buffer is full or EOF.
fn read_slice_best_effort(reader: &mut impl Read, buf: &mut [u8]) -> Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        let n = reader.read(&mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}

/// Writes all slices of a [`BytesView`] to a writer.
fn write_all_bytesview(writer: &mut impl std::io::Write, data: &BytesView) -> Result<()> {
    for (slice, _meta) in data.slices() {
        writer.write_all(slice)?;
    }
    Ok(())
}

impl Memory for FileInner {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.memory.reserve(min_bytes)
    }
}
