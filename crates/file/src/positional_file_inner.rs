// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs::{File, FileTimes, Metadata, Permissions, TryLockError};
use std::io::{Error, ErrorKind, Result};
use std::sync::Arc;

use bytesbuf::mem::Memory;
use bytesbuf::{BytesBuf, BytesView};
use sync_thunk::{Thunker, thunk};

use crate::shared_memory::SharedMemory;

#[cfg(unix)]
type FileHandle = Arc<File>;
#[cfg(windows)]
type FileHandle = Arc<std::sync::Mutex<File>>;

#[derive(Debug)]
pub struct PositionalFileInner {
    file: FileHandle,
    thunker: Thunker,
    memory: SharedMemory,
}

impl PositionalFileInner {
    /// Creates a `PositionalFileInner` from a standard `std::fs::File`.
    pub fn from_std(file: File, dir: &crate::directory::Directory, memory: SharedMemory) -> Self {
        #[cfg(unix)]
        let file = Arc::new(file);
        #[cfg(windows)]
        let file = Arc::new(std::sync::Mutex::new(file));
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
        self.file.lock().expect("file mutex poisoned").as_raw_handle()
    }

    /// Returns a borrowed handle.
    #[cfg(windows)]
    pub fn as_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        // SAFETY: The handle is valid for the lifetime of &self
        // because self holds an Arc<Mutex<File>> that keeps the handle open.
        unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(self.as_raw_handle()) }
    }

    /// Returns whether the underlying file descriptor refers to a terminal.
    pub fn is_terminal(&self) -> bool {
        use std::io::IsTerminal;
        #[cfg(unix)]
        {
            self.file.is_terminal()
        }
        #[cfg(windows)]
        {
            self.file.lock().is_ok_and(|f| f.is_terminal())
        }
    }

    /// Executes a closure with a `&File` reference, handling platform-specific locking.
    #[cfg(unix)]
    fn with_file<R>(&self, f: impl FnOnce(&File) -> R) -> R {
        f(&self.file)
    }

    /// Executes a closure with a `&File` reference, handling platform-specific locking.
    #[cfg(windows)]
    fn with_file<R>(&self, f: impl FnOnce(&File) -> R) -> R {
        f(&self.file.lock().expect("file mutex poisoned"))
    }

    #[thunk(from = self.thunker)]
    pub async fn read_max_at(&self, offset: u64, len: usize) -> Result<BytesView> {
        let mut buf = self.memory.reserve(len);
        self.with_file(|f| positional_read_into_bytesbuf(f, &mut buf, len, offset))?;
        Ok(buf.consume_all())
    }

    #[thunk(from = self.thunker)]
    pub async fn read_at(&self, offset: u64, len: usize) -> Result<BytesView> {
        let mut buf = self.memory.reserve(len);
        self.with_file(|f| {
            let mut total = 0;
            while total < len {
                let cur = offset.saturating_add(total as u64);
                let n = positional_read_into_bytesbuf(f, &mut buf, len - total, cur)?;
                if n == 0 {
                    break;
                }
                total += n;
            }
            Ok::<_, std::io::Error>(total)
        })?;
        Ok(buf.consume_all())
    }

    #[thunk(from = self.thunker)]
    pub async fn read_exact_into_bytesbuf_at(&self, offset: u64, len: usize, buf: &mut BytesBuf) -> Result<()> {
        if buf.remaining_capacity() < len {
            buf.reserve(len - buf.remaining_capacity(), &self.memory);
        }
        self.with_file(|f| {
            let mut total = 0;
            while total < len {
                let cur = offset.saturating_add(total as u64);
                let n = positional_read_into_bytesbuf(f, buf, len - total, cur)?;
                if n == 0 {
                    return Err(Error::new(ErrorKind::UnexpectedEof, "failed to fill whole buffer"));
                }
                total += n;
            }
            Ok(())
        })
    }

    #[thunk(from = self.thunker)]
    pub async fn read_max_into_bytesbuf_at(&self, offset: u64, len: usize, buf: &mut BytesBuf) -> Result<usize> {
        let needed = len.saturating_sub(buf.remaining_capacity());
        if needed > 0 {
            buf.reserve(needed, &self.memory);
        }
        self.with_file(|f| positional_read_into_bytesbuf(f, buf, len, offset))
    }

    #[thunk(from = self.thunker)]
    pub async fn read_exact_at(&self, offset: u64, len: usize) -> Result<BytesView> {
        let mut buf = self.memory.reserve(len);
        self.with_file(|f| {
            let mut total = 0;
            while total < len {
                let cur = offset.saturating_add(total as u64);
                let n = positional_read_into_bytesbuf(f, &mut buf, len - total, cur)?;
                if n == 0 {
                    return Err(Error::new(ErrorKind::UnexpectedEof, "failed to fill whole buffer"));
                }
                total += n;
            }
            Ok(())
        })?;
        Ok(buf.consume_all())
    }

    #[thunk(from = self.thunker)]
    pub async fn read_into_slice_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.with_file(|f| {
            let mut total = 0;
            while total < buf.len() {
                let cur = offset.saturating_add(total as u64);
                let n = positional_read(f, &mut buf[total..], cur)?;
                if n == 0 {
                    break;
                }
                total += n;
            }
            Ok(total)
        })
    }

    pub async fn read_exact_into_uninit_at(&self, offset: u64, buf: &mut [core::mem::MaybeUninit<u8>]) -> Result<()> {
        // SAFETY: MaybeUninit<u8> has the same layout as u8.
        // read_exact_into_slice_at writes exactly buf.len() bytes on success,
        // fully initializing the contents.
        let initialized = unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), buf.len()) };
        self.read_exact_into_slice_at(offset, initialized).await
    }

    #[thunk(from = self.thunker)]
    pub async fn read_exact_into_slice_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.with_file(|f| {
            let mut total = 0;
            while total < buf.len() {
                let cur = offset.saturating_add(total as u64);
                let n = positional_read(f, &mut buf[total..], cur)?;
                if n == 0 {
                    return Err(Error::new(ErrorKind::UnexpectedEof, "failed to fill whole buffer"));
                }
                total += n;
            }
            Ok(())
        })
    }

    #[thunk(from = self.thunker)]
    pub async fn write_at(&self, offset: u64, data: BytesView) -> Result<()> {
        self.with_file(|f| {
            let mut current_offset = offset;
            for (slice, _meta) in data.slices() {
                positional_write_all(f, slice, current_offset)?;
                current_offset += slice.len() as u64;
            }
            Ok(())
        })
    }

    #[thunk(from = self.thunker)]
    pub async fn write_slice_at(&self, offset: u64, data: &[u8]) -> Result<()> {
        self.with_file(|f| positional_write_all(f, data, offset))
    }

    #[thunk(from = self.thunker)]
    pub async fn metadata(&self) -> Result<Metadata> {
        self.with_file(File::metadata)
    }

    #[thunk(from = self.thunker)]
    pub async fn set_len(&self, size: u64) -> Result<()> {
        self.with_file(|f| f.set_len(size))
    }

    #[thunk(from = self.thunker)]
    pub async fn set_modified(&self, modified: std::time::SystemTime) -> Result<()> {
        self.with_file(|f| f.set_modified(modified))
    }

    #[thunk(from = self.thunker)]
    pub async fn set_permissions(&self, perms: Permissions) -> Result<()> {
        self.with_file(|f| f.set_permissions(perms))
    }

    #[thunk(from = self.thunker)]
    pub async fn set_times(&self, times: FileTimes) -> Result<()> {
        self.with_file(|f| f.set_times(times))
    }

    #[thunk(from = self.thunker)]
    pub async fn sync_all(&self) -> Result<()> {
        self.with_file(File::sync_all)
    }

    #[thunk(from = self.thunker)]
    pub async fn sync_data(&self) -> Result<()> {
        self.with_file(File::sync_data)
    }

    #[thunk(from = self.thunker)]
    pub async fn flush(&self) -> Result<()> {
        // std::fs::File has no internal buffer, so flush() through a shared
        // reference is effectively a no-op. Use sync_data() instead to
        // actually ensure data reaches the disk.
        self.with_file(File::sync_data)
    }

    #[thunk(from = self.thunker)]
    pub async fn lock(&self) -> Result<()> {
        self.with_file(File::lock)
    }

    #[thunk(from = self.thunker)]
    pub async fn lock_shared(&self) -> Result<()> {
        self.with_file(File::lock_shared)
    }

    #[thunk(from = self.thunker)]
    pub async fn try_lock(&self) -> core::result::Result<(), TryLockError> {
        self.with_file(File::try_lock)
    }

    #[thunk(from = self.thunker)]
    pub async fn try_lock_shared(&self) -> core::result::Result<(), TryLockError> {
        self.with_file(File::try_lock_shared)
    }

    #[thunk(from = self.thunker)]
    pub async fn unlock(&self) -> Result<()> {
        self.with_file(File::unlock)
    }

    #[expect(clippy::unused_async, reason = "async signature required to match public API contract")]
    pub async fn try_clone(&self) -> Result<Self> {
        Ok(Self {
            file: Arc::clone(&self.file),
            thunker: self.thunker.clone(),
            memory: self.memory.clone(),
        })
    }
}

impl Memory for PositionalFileInner {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.memory.reserve(min_bytes)
    }
}

/// Reads bytes at `offset` without affecting the cursor.
#[cfg(unix)]
pub fn positional_read(file: &File, buf: &mut [u8], offset: u64) -> Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buf, offset)
}

/// Reads bytes at `offset` without affecting the cursor.
#[cfg(windows)]
pub fn positional_read(file: &File, buf: &mut [u8], offset: u64) -> Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buf, offset)
}

/// Writes bytes at `offset` without affecting the cursor.
#[cfg(unix)]
pub fn positional_write(file: &File, buf: &[u8], offset: u64) -> Result<usize> {
    use std::os::unix::fs::FileExt;
    file.write_at(buf, offset)
}

/// Writes bytes at `offset` without affecting the cursor.
#[cfg(windows)]
pub fn positional_write(file: &File, buf: &[u8], offset: u64) -> Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_write(buf, offset)
}

/// Writes all of `buf` at `offset`, retrying on short writes.
pub fn positional_write_all(file: &File, mut buf: &[u8], mut offset: u64) -> Result<()> {
    while !buf.is_empty() {
        let n = positional_write(file, buf, offset)?;
        if n == 0 {
            return Err(Error::new(ErrorKind::WriteZero, "failed to write whole buffer"));
        }
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

/// Reads up to `len` bytes at `offset` directly into `buf`'s unfilled capacity,
/// without affecting the file cursor.
pub fn positional_read_into_bytesbuf(file: &File, buf: &mut BytesBuf, len: usize, offset: u64) -> Result<usize> {
    let unfilled = buf.first_unfilled_slice();
    let read_len = len.min(unfilled.len());

    // SAFETY: same as read_into_bytesbuf in io_helpers.rs — MaybeUninit<u8> has identical layout to u8.
    // We skip zero-initialization because the OS file primitives only write to the buffer.
    let dst = unsafe { core::slice::from_raw_parts_mut(unfilled.as_mut_ptr().cast::<u8>(), read_len) };
    let n = positional_read(file, dst, offset)?;
    if n > 0 {
        // SAFETY: `n` bytes were just written by the positional read.
        unsafe {
            buf.advance(n);
        }
    }
    Ok(n)
}
