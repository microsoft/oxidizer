// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Shared implementation for file types.
///
/// Contains the common fields and methods used by [`ReadOnlyFile`],
/// [`WriteOnlyFile`], and [`File`].
///
/// Several methods take `&mut self` even though they don't mutate `self`
/// directly. The `&mut` borrow prevents concurrent cursor-affecting
/// operations at the Rust level — the actual mutation happens on the
/// worker thread through `Arc<RwLock<File>>`.
use std::fs::{File, FileTimes, Metadata, Permissions, TryLockError};
use std::io::{Error, ErrorKind, Read, Result, Seek as _, SeekFrom, Write as _};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use bytesbuf::mem::Memory;
use bytesbuf::{BytesBuf, BytesView};

use crate::dispatcher::Dispatcher;
use crate::shared_memory::SharedMemory;

const DEFAULT_READ_SIZE: usize = 8192;

#[derive(Debug)]
pub struct FileInner {
    file: Arc<RwLock<File>>,
    dispatcher: Dispatcher,
    memory: SharedMemory,
}

#[expect(
    clippy::significant_drop_tightening,
    reason = "RwLock guard must be held across I/O operations in dispatch closures"
)]
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "&mut self enforces sequential cursor access across dispatch boundary"
)]
impl FileInner {
    pub(crate) const fn new(file: Arc<RwLock<File>>, dispatcher: Dispatcher, memory: SharedMemory) -> Self {
        Self { file, dispatcher, memory }
    }

    /// Creates a `FileInner` from a standard `std::fs::File`.
    ///
    /// Wraps the file in an `Arc<RwLock<>>` and extracts the dispatcher from the directory.
    pub(crate) fn from_std(file: File, dir: &crate::directory::Directory, memory: SharedMemory) -> Self {
        Self::new(Arc::new(RwLock::new(file)), dir.dispatcher().clone(), memory)
    }

    pub(crate) const fn memory(&self) -> &SharedMemory {
        &self.memory
    }

    /// Returns the raw file descriptor (Unix) or handle (Windows).
    ///
    /// Briefly acquires a read lock on the inner `RwLock`.
    #[cfg(unix)]
    pub(crate) fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        use std::os::unix::io::AsRawFd;
        self.file.read().expect("file RwLock poisoned").as_raw_fd()
    }

    /// Returns the raw file descriptor (Unix) or handle (Windows).
    ///
    /// Briefly acquires a read lock on the inner `RwLock`.
    #[cfg(windows)]
    pub(crate) fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        use std::os::windows::io::AsRawHandle;
        self.file.read().expect("file RwLock poisoned").as_raw_handle()
    }

    /// Returns whether the underlying file descriptor refers to a terminal.
    pub(crate) fn is_terminal(&self) -> bool {
        use std::io::IsTerminal;
        self.file.read().is_ok_and(|f| f.is_terminal())
    }

    /// Synchronous read, bypassing the dispatcher.
    ///
    /// Acquires a write lock (for cursor advancement) and reads directly
    /// on the calling thread.
    #[cfg(feature = "sync-compat")]
    pub(crate) fn sync_read(&mut self, buf: &mut [u8]) -> Result<usize> {
        write_lock(&self.file)?.read(buf)
    }

    /// Synchronous write, bypassing the dispatcher.
    #[cfg(feature = "sync-compat")]
    pub(crate) fn sync_write(&mut self, buf: &[u8]) -> Result<usize> {
        write_lock(&self.file)?.write(buf)
    }

    /// Synchronous flush, bypassing the dispatcher.
    #[cfg(feature = "sync-compat")]
    pub(crate) fn sync_flush(&mut self) -> Result<()> {
        write_lock(&self.file)?.flush()
    }

    /// Synchronous seek, bypassing the dispatcher.
    #[cfg(feature = "sync-compat")]
    pub(crate) fn sync_seek(&mut self, pos: SeekFrom) -> Result<u64> {
        write_lock(&self.file)?.seek(pos)
    }

    pub async fn read_at_most_into(&mut self, len: usize, mut into: BytesBuf) -> Result<(usize, BytesBuf)> {
        // Pre-reserve on the caller side to avoid cloning SharedMemory
        // into the dispatch closure.
        if into.remaining_capacity() < len {
            into.reserve(len - into.remaining_capacity(), &self.memory);
        }
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let mut f = write_lock(&file)?;
                let n = read_into_buf(&mut *f, &mut into, len)?;
                Ok((n, into))
            })
            .await
    }

    pub async fn read_more_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf)> {
        self.read_at_most_into(DEFAULT_READ_SIZE, into).await
    }

    pub async fn read_at(&self, offset: u64, len: usize) -> Result<BytesView> {
        let file = Arc::clone(&self.file);
        let mut buf = self.memory.reserve(len);
        self.dispatcher
            .dispatch(move || {
                let f = positional_lock(&file)?;
                let _n = positional_read_into_buf(&f, &mut buf, len, offset)?;
                Ok(buf.consume_all())
            })
            .await
    }

    pub async fn read_best_effort_at(&self, offset: u64, len: usize) -> Result<BytesView> {
        let file = Arc::clone(&self.file);
        let mut buf = self.memory.reserve(len);
        self.dispatcher
            .dispatch(move || {
                let f = positional_lock(&file)?;
                let mut total = 0;
                while total < len {
                    let current_offset = offset.saturating_add(total as u64);
                    let n = positional_read_into_buf(&f, &mut buf, len - total, current_offset)?;
                    if n == 0 {
                        break;
                    }
                    total += n;
                }
                Ok(buf.consume_all())
            })
            .await
    }

    pub async fn read_at_into(&self, offset: u64, len: usize, mut buf: BytesBuf) -> Result<(usize, BytesBuf)> {
        // Pre-reserve on the caller side.
        if buf.remaining_capacity() < len {
            buf.reserve(len - buf.remaining_capacity(), &self.memory);
        }
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = positional_lock(&file)?;
                let n = positional_read_into_buf(&f, &mut buf, len, offset)?;
                Ok((n, buf))
            })
            .await
    }

    pub async fn read_exact_at(&self, offset: u64, len: usize) -> Result<BytesView> {
        let file = Arc::clone(&self.file);
        let mut buf = self.memory.reserve(len);
        self.dispatcher
            .dispatch(move || {
                let f = positional_lock(&file)?;
                let mut total = 0;
                while total < len {
                    let current_offset = offset.saturating_add(total as u64);
                    let n = positional_read_into_buf(&f, &mut buf, len - total, current_offset)?;
                    if n == 0 {
                        return Err(Error::new(ErrorKind::UnexpectedEof, "failed to fill whole buffer"));
                    }
                    total += n;
                }
                Ok(buf.consume_all())
            })
            .await
    }

    pub async fn read_slice(&mut self, buf: &mut [u8]) -> Result<usize> {
        let file = Arc::clone(&self.file);
        let raw = SendSliceMut::new(buf);
        self.dispatcher
            .dispatch_scoped(move || {
                // SAFETY: ScopedDispatchFuture guarantees the closure completes
                // (or never starts) before the caller regains access to `buf`.
                let buf = unsafe { raw.into_mut_slice() };
                let mut f = write_lock(&file)?;
                f.read(buf)
            })
            .await
    }

    pub async fn read_slice_best_effort(&mut self, buf: &mut [u8]) -> Result<usize> {
        let file = Arc::clone(&self.file);
        let raw = SendSliceMut::new(buf);
        self.dispatcher
            .dispatch_scoped(move || {
                // SAFETY: ScopedDispatchFuture guarantees the closure completes
                // (or never starts) before the caller regains access to `buf`.
                let buf = unsafe { raw.into_mut_slice() };
                let mut f = write_lock(&file)?;
                let mut total = 0;
                while total < buf.len() {
                    let n = f.read(&mut buf[total..])?;
                    if n == 0 {
                        break;
                    }
                    total += n;
                }
                Ok(total)
            })
            .await
    }

    pub async fn read_slice_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        let file = Arc::clone(&self.file);
        let raw = SendSliceMut::new(buf);
        self.dispatcher
            .dispatch_scoped(move || {
                // SAFETY: ScopedDispatchFuture guarantees the closure completes
                // (or never starts) before the caller regains access to `buf`.
                let buf = unsafe { raw.into_mut_slice() };
                let mut f = write_lock(&file)?;
                f.read_exact(buf)
            })
            .await
    }

    pub async fn read_slice_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let file = Arc::clone(&self.file);
        let raw = SendSliceMut::new(buf);
        self.dispatcher
            .dispatch_scoped(move || {
                // SAFETY: ScopedDispatchFuture guarantees the closure completes
                // (or never starts) before the caller regains access to `buf`.
                let buf = unsafe { raw.into_mut_slice() };
                let f = positional_lock(&file)?;
                positional_read(&f, buf, offset)
            })
            .await
    }

    pub async fn read_slice_at_best_effort(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let file = Arc::clone(&self.file);
        let raw = SendSliceMut::new(buf);
        self.dispatcher
            .dispatch_scoped(move || {
                // SAFETY: ScopedDispatchFuture guarantees the closure completes
                // (or never starts) before the caller regains access to `buf`.
                let buf = unsafe { raw.into_mut_slice() };
                let f = positional_lock(&file)?;
                let mut total = 0;
                while total < buf.len() {
                    let current_offset = offset.saturating_add(total as u64);
                    let n = positional_read(&f, &mut buf[total..], current_offset)?;
                    if n == 0 {
                        break;
                    }
                    total += n;
                }
                Ok(total)
            })
            .await
    }

    pub async fn read_slice_at_exact(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let file = Arc::clone(&self.file);
        let raw = SendSliceMut::new(buf);
        self.dispatcher
            .dispatch_scoped(move || {
                // SAFETY: ScopedDispatchFuture guarantees the closure completes
                // (or never starts) before the caller regains access to `buf`.
                let buf = unsafe { raw.into_mut_slice() };
                let f = positional_lock(&file)?;
                let mut total = 0;
                while total < buf.len() {
                    let current_offset = offset.saturating_add(total as u64);
                    let n = positional_read(&f, &mut buf[total..], current_offset)?;
                    if n == 0 {
                        return Err(Error::new(ErrorKind::UnexpectedEof, "failed to fill whole buffer"));
                    }
                    total += n;
                }
                Ok(())
            })
            .await
    }

    pub async fn write(&mut self, mut data: BytesView) -> Result<()> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let mut f = write_lock(&file)?;
                while !data.is_empty() {
                    let slice = data.first_slice();
                    let len = slice.len();
                    f.write_all(slice)?;
                    data.advance(len);
                }
                Ok(())
            })
            .await
    }

    pub async fn write_all_at(&self, offset: u64, mut data: BytesView) -> Result<()> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = positional_lock(&file)?;
                let mut current_offset = offset;
                while !data.is_empty() {
                    let slice = data.first_slice();
                    positional_write_all(&f, slice, current_offset)?;
                    current_offset += slice.len() as u64;
                    let len = slice.len();
                    data.advance(len);
                }
                Ok(())
            })
            .await
    }

    pub async fn write_slice(&mut self, data: &[u8]) -> Result<()> {
        let file = Arc::clone(&self.file);
        let raw = SendSlice::new(data);
        self.dispatcher
            .dispatch_scoped(move || {
                // SAFETY: ScopedDispatchFuture guarantees the closure completes
                // (or never starts) before the caller regains access to `data`.
                let data = unsafe { raw.into_slice() };
                let mut f = write_lock(&file)?;
                f.write_all(data)
            })
            .await
    }

    pub async fn write_slice_at(&self, offset: u64, data: &[u8]) -> Result<()> {
        let file = Arc::clone(&self.file);
        let raw = SendSlice::new(data);
        self.dispatcher
            .dispatch_scoped(move || {
                // SAFETY: ScopedDispatchFuture guarantees the closure completes
                // (or never starts) before the caller regains access to `data`.
                let data = unsafe { raw.into_slice() };
                let f = positional_lock(&file)?;
                positional_write_all(&f, data, offset)
            })
            .await
    }

    pub async fn metadata(&self) -> Result<Metadata> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = read_lock(&file)?;
                f.metadata()
            })
            .await
    }

    pub async fn set_len(&self, size: u64) -> Result<()> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = read_lock(&file)?;
                f.set_len(size)
            })
            .await
    }

    pub async fn set_modified(&self, modified: std::time::SystemTime) -> Result<()> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = read_lock(&file)?;
                f.set_modified(modified)
            })
            .await
    }

    pub async fn set_permissions(&self, perms: Permissions) -> Result<()> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = read_lock(&file)?;
                f.set_permissions(perms)
            })
            .await
    }

    pub async fn set_times(&self, times: FileTimes) -> Result<()> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = read_lock(&file)?;
                f.set_times(times)
            })
            .await
    }

    pub async fn sync_all(&self) -> Result<()> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = read_lock(&file)?;
                f.sync_all()
            })
            .await
    }

    pub async fn sync_data(&self) -> Result<()> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = read_lock(&file)?;
                f.sync_data()
            })
            .await
    }

    pub async fn flush(&self) -> Result<()> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let mut f = write_lock(&file)?;
                f.flush()
            })
            .await
    }

    pub async fn lock(&self) -> Result<()> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = read_lock(&file)?;
                f.lock()
            })
            .await
    }

    pub async fn lock_shared(&self) -> Result<()> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = read_lock(&file)?;
                f.lock_shared()
            })
            .await
    }

    pub async fn try_lock(&self) -> core::result::Result<(), TryLockError> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = file
                    .read()
                    .map_err(|e| TryLockError::Error(Error::other(format!("file lock poisoned: {e}"))))?;
                f.try_lock()
            })
            .await
    }

    pub async fn try_lock_shared(&self) -> core::result::Result<(), TryLockError> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = file
                    .read()
                    .map_err(|e| TryLockError::Error(Error::other(format!("file lock poisoned: {e}"))))?;
                f.try_lock_shared()
            })
            .await
    }

    pub async fn unlock(&self) -> Result<()> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let f = read_lock(&file)?;
                f.unlock()
            })
            .await
    }

    pub async fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let mut f = write_lock(&file)?;
                f.seek(pos)
            })
            .await
    }

    pub async fn stream_position(&mut self) -> Result<u64> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let mut f = write_lock(&file)?;
                f.stream_position()
            })
            .await
    }

    pub async fn rewind(&mut self) -> Result<()> {
        let file = Arc::clone(&self.file);
        self.dispatcher
            .dispatch(move || {
                let mut f = write_lock(&file)?;
                f.rewind()
            })
            .await
    }

    pub async fn try_clone(&self) -> Result<Self> {
        let file = Arc::clone(&self.file);
        let new_file = self
            .dispatcher
            .dispatch(move || {
                let f = read_lock(&file)?;
                f.try_clone()
            })
            .await?;
        Ok(Self {
            file: Arc::new(RwLock::new(new_file)),
            dispatcher: self.dispatcher.clone(),
            memory: self.memory.clone(),
        })
    }
}

impl Memory for FileInner {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.memory.reserve(min_bytes)
    }
}

fn read_lock(file: &RwLock<File>) -> Result<RwLockReadGuard<'_, File>> {
    file.read().map_err(|e| Error::other(format!("file lock poisoned: {e}")))
}

fn write_lock(file: &RwLock<File>) -> Result<RwLockWriteGuard<'_, File>> {
    file.write().map_err(|e| Error::other(format!("file lock poisoned: {e}")))
}

/// Lock for positional I/O operations.
///
/// On Unix, `pread`/`pwrite` are truly cursor-independent, so a shared lock
/// suffices and allows concurrent positional operations.
///
/// On Windows, `seek_read`/`seek_write` move the file cursor as a side effect,
/// so an exclusive lock is required to prevent concurrent positional operations
/// from corrupting the seek position of a concurrent operation.
#[cfg(unix)]
fn positional_lock(file: &RwLock<File>) -> Result<RwLockReadGuard<'_, File>> {
    read_lock(file)
}

#[cfg(windows)]
fn positional_lock(file: &RwLock<File>) -> Result<RwLockWriteGuard<'_, File>> {
    write_lock(file)
}

/// Reads up to `len` bytes from `reader` directly into `buf`'s unfilled capacity,
/// avoiding a temporary `Vec` allocation.
pub fn read_into_buf(reader: &mut impl Read, buf: &mut BytesBuf, len: usize) -> Result<usize> {
    let unfilled = buf.first_unfilled_slice();
    let read_len = len.min(unfilled.len());

    // Initialize the buffer to zeros before passing to read().
    // This is necessary because std::io::Read's contract allows implementations
    // to read the buffer contents before writing (though most don't).
    // Passing uninitialized memory would be Undefined Behavior.
    //
    // OPTIMIZATION: We are skipping initialization because we know we are backed by
    // std::fs::File (or similar OS resources) which only write to the buffer.
    // This avoids a memset/zeroing loop.
    // for slot in unfilled.iter_mut().take(read_len) {
    //     *slot = core::mem::MaybeUninit::new(0);
    // }

    // SAFETY: MaybeUninit<u8> has the same layout as u8.
    // We are passing uninitialized memory to the reader.
    // Since we know the reader is a file, this is safe in practice as the OS
    // writes to the buffer without reading it.
    // The read call writes `n` bytes; we only advance by `n` below.
    let dst = unsafe { core::slice::from_raw_parts_mut(unfilled.as_mut_ptr().cast::<u8>(), read_len) };
    let n = reader.read(dst)?;
    if n > 0 {
        // SAFETY: `n` bytes were just written by the read call.
        unsafe {
            buf.advance(n);
        }
    }
    Ok(n)
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
fn positional_read_into_buf(file: &File, buf: &mut BytesBuf, len: usize, offset: u64) -> Result<usize> {
    let unfilled = buf.first_unfilled_slice();
    let read_len = len.min(unfilled.len());

    // Initialize the buffer to zeros before reading (see read_into_buf for explanation).
    // OPTIMIZATION: Skipped initialization as we are using OS file primitives.
    // for slot in unfilled.iter_mut().take(read_len) {
    //     *slot = core::mem::MaybeUninit::new(0);
    // }

    // SAFETY: same as read_into_buf — MaybeUninit<u8> has identical layout to u8.
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

/// An immutable raw-pointer slice that is [`Send`].
///
/// # Safety
///
/// The caller must guarantee the pointed-to data is alive and not mutably
/// aliased for the duration of any cross-thread access. In this crate,
/// [`ScopedDispatchFuture`](crate::dispatcher::ScopedDispatchFuture) provides
/// that guarantee by blocking on drop.
#[derive(Clone, Copy)]
struct SendSlice {
    ptr: *const u8,
    len: usize,
}

impl SendSlice {
    fn new(slice: &[u8]) -> Self {
        Self {
            ptr: slice.as_ptr(),
            len: slice.len(),
        }
    }

    /// Reconstructs the original `&[u8]`.
    ///
    /// # Safety
    ///
    /// The original slice must still be alive and not mutably aliased.
    unsafe fn into_slice(self) -> &'static [u8] {
        // SAFETY: caller guarantees the slice is alive and unaliased.
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }
}

// SAFETY: The safety contract on SendSlice::new and ScopedDispatchFuture
// together ensure the data is alive and unaliased during cross-thread access.
unsafe impl Send for SendSlice {}

/// A mutable raw-pointer slice that is [`Send`].
///
/// Same safety contract as [`SendSlice`], but for mutable access.
#[derive(Clone, Copy)]
struct SendSliceMut {
    ptr: *mut u8,
    len: usize,
}

impl SendSliceMut {
    fn new(slice: &mut [u8]) -> Self {
        Self {
            ptr: slice.as_mut_ptr(),
            len: slice.len(),
        }
    }

    /// Reconstructs the original `&mut [u8]`.
    ///
    /// # Safety
    ///
    /// The original slice must still be alive and exclusively owned by the caller.
    unsafe fn into_mut_slice(self) -> &'static mut [u8] {
        // SAFETY: caller guarantees the slice is alive and exclusively owned.
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

// SAFETY: Same as SendSlice — ScopedDispatchFuture guarantees the data
// outlives the cross-thread access.
unsafe impl Send for SendSliceMut {}
