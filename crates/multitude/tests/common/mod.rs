// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared test helpers. Each integration test file includes this via
//! `mod common;` and uses items as `common::*`.

#![allow(dead_code, reason = "shared between multiple test binaries; some helpers may be unused per-file")]

use core::alloc::Layout;
use core::cell::Cell;
use core::hash::{Hash, Hasher};
use core::ptr::NonNull;
use std::collections::hash_map::DefaultHasher;

use allocator_api2::alloc::{AllocError, Allocator, Global};

pub fn hash_of<T: Hash>(v: &T) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// Allocator that fails its `allocate` calls after the first `n`
/// successes. Used to drive the `try_alloc*` / `panic_alloc` Err
/// branches that aren't reachable with the global allocator under
/// normal test conditions.
///
/// Cloned references share the same underlying counter (interior
/// mutability via `Rc<Cell<usize>>` on the test side; here we wrap
/// the count in a heap pointer for `Clone` to copy).
#[derive(Clone)]
pub struct FailingAllocator {
    remaining: std::rc::Rc<Cell<usize>>,
}

impl FailingAllocator {
    pub fn new(allow_n_allocs: usize) -> Self {
        Self {
            remaining: std::rc::Rc::new(Cell::new(allow_n_allocs)),
        }
    }
}

// SAFETY: forwards to Global on success; fails atomically on exhaustion.
unsafe impl Allocator for FailingAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let r = self.remaining.get();
        if r == 0 {
            return Err(AllocError);
        }
        self.remaining.set(r - 1);
        Global.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded — caller's contract.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

/// Allocator that tracks live allocations (count and bytes) so tests
/// can detect leaks across an `Arena`'s lifetime. Tracks `allocate`
/// vs. `deallocate` and `grow`/`shrink` deltas.
#[derive(Clone)]
pub struct TrackingAllocator {
    live_chunks: std::rc::Rc<Cell<isize>>,
    live_bytes: std::rc::Rc<Cell<isize>>,
}

impl TrackingAllocator {
    pub fn new() -> Self {
        Self {
            live_chunks: std::rc::Rc::new(Cell::new(0)),
            live_bytes: std::rc::Rc::new(Cell::new(0)),
        }
    }

    pub fn live_chunks(&self) -> isize {
        self.live_chunks.get()
    }

    pub fn live_bytes(&self) -> isize {
        self.live_bytes.get()
    }
}

// SAFETY: forwards to Global; counters are interior-mutable bookkeeping only.
unsafe impl Allocator for TrackingAllocator {
    #[expect(clippy::cast_possible_wrap, reason = "test allocator: chunk sizes fit in isize")]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let p = Global.allocate(layout)?;
        self.live_chunks.set(self.live_chunks.get() + 1);
        self.live_bytes.set(self.live_bytes.get() + layout.size() as isize);
        Ok(p)
    }

    #[expect(clippy::cast_possible_wrap, reason = "test allocator: chunk sizes fit in isize")]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded — caller's contract.
        unsafe { Global.deallocate(ptr, layout) };
        self.live_chunks.set(self.live_chunks.get() - 1);
        self.live_bytes.set(self.live_bytes.get() - layout.size() as isize);
    }
}
/// Send + Sync variant of [`TrackingAllocator`] for tests that need
/// to allocate `Arc` (whose constructor requires `A: Send + Sync`).
#[derive(Clone)]
pub struct SendTrackingAllocator {
    live_chunks: std::sync::Arc<core::sync::atomic::AtomicIsize>,
    live_bytes: std::sync::Arc<core::sync::atomic::AtomicIsize>,
}

impl SendTrackingAllocator {
    pub fn new() -> Self {
        Self {
            live_chunks: std::sync::Arc::new(core::sync::atomic::AtomicIsize::new(0)),
            live_bytes: std::sync::Arc::new(core::sync::atomic::AtomicIsize::new(0)),
        }
    }

    pub fn live_chunks(&self) -> isize {
        self.live_chunks.load(core::sync::atomic::Ordering::Relaxed)
    }

    pub fn live_bytes(&self) -> isize {
        self.live_bytes.load(core::sync::atomic::Ordering::Relaxed)
    }
}

// SAFETY: forwards to Global; counters are atomic bookkeeping only.
unsafe impl Allocator for SendTrackingAllocator {
    #[expect(clippy::cast_possible_wrap, reason = "test allocator: chunk sizes fit in isize")]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let p = Global.allocate(layout)?;
        self.live_chunks.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        self.live_bytes
            .fetch_add(layout.size() as isize, core::sync::atomic::Ordering::Relaxed);
        Ok(p)
    }

    #[expect(clippy::cast_possible_wrap, reason = "test allocator: chunk sizes fit in isize")]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded — caller's contract.
        unsafe { Global.deallocate(ptr, layout) };
        self.live_chunks.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
        self.live_bytes
            .fetch_sub(layout.size() as isize, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Pathological allocator that returns a non-null pointer whose end
/// address lies in the upper half of the address space (above
/// `isize::MAX`). Backing memory is **not** real — the chunk-allocation
/// path must reject the pointer at its bounds check before reading or
/// writing through it. `deallocate` is therefore a no-op.
///
/// Used to drive `chunk_end_addr_fits_in_isize`-style regression tests.
#[derive(Clone, Copy, Default)]
pub struct BadAddressAllocator;

// SAFETY: returns synthetic pointers never read or written; `deallocate`
// is a no-op so no foreign allocator is asked to free a fake pointer.
unsafe impl Allocator for BadAddressAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let size = layout.size();
        let align = layout.align();
        // Skip zero-sized or unsupportable alignments.
        if size == 0 || align == 0 {
            return Err(AllocError);
        }
        // Aim for `end_addr > isize::MAX as usize` with the start address
        // aligned to `align`. Choosing the next aligned address at or
        // above `(isize::MAX as usize + 1) - size` satisfies both.
        let target_end_floor = 1usize << (usize::BITS - 1); // isize::MAX + 1
        let unaligned_start = target_end_floor.checked_sub(size).ok_or(AllocError)?;
        let mask = align - 1;
        let start_addr = unaligned_start.checked_add(mask).ok_or(AllocError)? & !mask;
        // The pointer is synthetic and never dereferenced — only its
        // address is observed by `chunk_end_addr_fits_in_isize`. Use
        // `without_provenance_mut` so Miri doesn't conflate the
        // integer cast with a real exposed-provenance pointer.
        // SAFETY: `start_addr` is non-zero by construction (target_end_floor
        // is the high bit and start lives near it).
        let nn = unsafe { NonNull::new_unchecked(core::ptr::without_provenance_mut::<u8>(start_addr)) };
        Ok(NonNull::slice_from_raw_parts(nn, size))
    }

    unsafe fn deallocate(&self, _ptr: NonNull<u8>, _layout: Layout) {
        // No-op: the pointer is synthetic, never backed by real memory.
    }
}

/// constructor families (which require `A: Send + Sync`).
#[derive(Clone)]
pub struct SendFailingAllocator {
    remaining: std::sync::Arc<core::sync::atomic::AtomicUsize>,
}

impl SendFailingAllocator {
    pub fn new(allow_n_allocs: usize) -> Self {
        Self {
            remaining: std::sync::Arc::new(core::sync::atomic::AtomicUsize::new(allow_n_allocs)),
        }
    }
}

// SAFETY: forwards to Global on success; fails atomically on exhaustion.
unsafe impl Allocator for SendFailingAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        use core::sync::atomic::Ordering;
        loop {
            let r = self.remaining.load(Ordering::Relaxed);
            if r == 0 {
                return Err(AllocError);
            }
            if self
                .remaining
                .compare_exchange(r, r - 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return Global.allocate(layout);
            }
        }
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded — caller's contract.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

/// Drop-tracking payload. Pushes its label onto a thread-local
/// vector when dropped. Tests use `Droppy::take_log()` to inspect
/// the order in which payloads were destroyed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Droppy(pub &'static str);

std::thread_local! {
    static DROPPY_LOG: Cell<Option<Vec<&'static str>>> = const { Cell::new(None) };
}

impl Drop for Droppy {
    fn drop(&mut self) {
        DROPPY_LOG.with(|c| {
            let mut v = c.take().unwrap_or_default();
            v.push(self.0);
            c.set(Some(v));
        });
    }
}

impl Droppy {
    /// Drain the thread-local drop log and return the captured labels.
    pub fn take_log() -> Vec<&'static str> {
        DROPPY_LOG.with(|c| c.take().unwrap_or_default())
    }
}

/// Reference-counted drop counter. Increments its `AtomicUsize` once
/// when dropped. Clones share the same counter via the inner `Arc`.
#[derive(Clone, Debug, Default)]
pub struct DropCounter(pub std::sync::Arc<core::sync::atomic::AtomicUsize>);

impl DropCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn count(&self) -> usize {
        self.0.load(core::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}
