// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::undocumented_unsafe_blocks,
    reason = "the unsafe block is governed by the precondition documented on the surrounding function — the caller has a fresh allocation from a shared chunk under refcount discipline"
)]

//! `bytesbuf::mem::Memory` support backed by arena shared chunks.
//!
//! # Usage
//!
//! ```
//! # #[cfg(feature = "bytesbuf")] {
//! use bytesbuf::mem::Memory;
//! use multitude::Arena;
//!
//! let arena = Arena::new();
//!
//! let mut buf = arena.reserve(64);
//! buf.put_slice(*b"hello, arena!");
//! let view = buf.consume_all();
//! assert_eq!(view.len(), 13);
//! # }
//! ```

use alloc::alloc::{alloc, dealloc, handle_alloc_error};
use core::alloc::Layout;
use core::mem::MaybeUninit;
use core::num::NonZero;
use core::ptr::{NonNull, drop_in_place, write};
// Use the real `AtomicUsize` here, not the loom shim: `reserve`
// writes a fresh `ArenaBlockState` into heap memory with `ptr::write`,
// and loom atomics cannot be moved that way. The orderings match `Arc`.
use core::sync::atomic::{AtomicUsize, Ordering, fence};

use allocator_api2::alloc::Allocator;
use bytesbuf::BytesBuf;
use bytesbuf::mem::{Block, BlockRef, BlockRefDynamic, BlockRefVTable, Memory};

use crate::Arena;
use crate::internal::in_chunk::InSharedChunk;
use crate::internal::shared_chunk::SharedChunk;

impl<A: Allocator + Clone + Send + Sync + 'static> Memory for Arena<A> {
    /// Reserve `min_bytes` of arena-backed buffer space as a [`BytesBuf`].
    ///
    /// # Panics
    ///
    /// Panics if `min_bytes > u32::MAX`: the `bytesbuf` crate's block
    /// size is a `u32`, so individual reservations are capped at
    /// `u32::MAX` bytes (just under 4 GiB) regardless of the host
    /// pointer width. Callers needing larger buffers must chunk the
    /// request themselves. Also panics if the arena's underlying
    /// allocator fails.
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        if min_bytes == 0 {
            return BytesBuf::new();
        }

        let block_len = u32::try_from(min_bytes)
            .expect("min_bytes exceeds u32::MAX bytes (just under 4 GiB), the per-reservation cap imposed by bytesbuf's BlockSize");
        let block_len_nz = NonZero::new(block_len).expect("min_bytes is non-zero (handled above)");

        // Reserve `min_bytes` and take the block's single chunk hold.
        let layout = Layout::from_size_align(min_bytes, 1).expect("byte layout with align 1 is always valid");
        let data = self.allocate_shared_layout(layout).expect("arena allocation failed");

        // Heap-allocate per-block state for `BlockRef` clones.
        let state_layout = Layout::new::<ArenaBlockState>();
        let raw_state = unsafe { alloc(state_layout) };
        let state_nn = NonNull::new(raw_state).unwrap_or_else(|| abort_oom(state_layout));
        let state_ptr: NonNull<ArenaBlockState> = state_nn.cast();
        // SAFETY: state_ptr is freshly allocated, exclusive, properly aligned.
        unsafe {
            write(
                state_ptr.as_ptr(),
                ArenaBlockState {
                    data_ptr: data,
                    ref_count: AtomicUsize::new(1),
                    release_fn: release_chunk_ref_shared::<A>,
                },
            );
        };

        // SAFETY: `state_ptr` stays valid until the last `BlockRef` drops.
        let block_ref = unsafe { BlockRef::new(state_ptr, &ARENA_BLOCK_VTABLE) };

        // SAFETY: `data` covers `min_bytes` bytes of exclusive capacity,
        // and the block's chunk hold keeps them alive.
        let block = unsafe { Block::new(data.cast::<MaybeUninit<u8>>(), block_len_nz, block_ref) };
        BytesBuf::from_blocks([block])
    }
}

#[cold]
#[inline(never)]
#[cfg_attr(coverage_nightly, coverage(off))]
// OOM tests abort before llvm-cov can flush counters.
fn abort_oom(layout: Layout) -> ! {
    handle_alloc_error(layout);
}

/// Type-erased release function for the arena chunk refcount.
type ReleaseFn = unsafe fn(NonNull<u8>);

/// Per-block state allocated on the heap. Manages a reference count for
/// `BlockRef` cloning and releases the arena chunk when the last ref drops.
#[repr(C)]
struct ArenaBlockState {
    /// Pointer into the arena chunk's data region. Used to locate the
    /// chunk header for refcount release.
    data_ptr: NonNull<u8>,
    /// `BlockRef` clone count.
    ref_count: AtomicUsize,
    /// Type-erased function to release the arena chunk refcount.
    release_fn: ReleaseFn,
}

// SAFETY: `data_ptr` points into an atomically refcounted shared chunk,
// `ref_count` is atomic, and `release_fn` is a plain function pointer.
unsafe impl Send for ArenaBlockState {}
// SAFETY: Same rationale as Send — all fields are either atomic or plain data.
unsafe impl Sync for ArenaBlockState {}

/// Static vtable for arena-backed blocks.
static ARENA_BLOCK_VTABLE: BlockRefVTable<ArenaBlockState> = BlockRefVTable::from_trait();

// SAFETY: state refcounts and backing chunk refcounts are both atomic.
unsafe impl BlockRefDynamic for ArenaBlockState {
    type State = Self;

    fn clone(state_ptr: NonNull<Self::State>) -> NonNull<Self::State> {
        // SAFETY: `state_ptr` is live while we hold this clone, and
        // `ref_count` is atomic.
        let prev = unsafe { (*state_ptr.as_ptr()).ref_count.fetch_add(1, Ordering::Relaxed) };
        check_arena_block_state_refcount(prev);
        debug_assert!(prev > 0, "BlockRef::clone on a dead state");
        state_ptr
    }

    fn drop(state_ptr: NonNull<Self::State>) {
        // SAFETY: `state_ptr` is live while we drop this clone.
        let prev = unsafe { (*state_ptr.as_ptr()).ref_count.fetch_sub(1, Ordering::Release) };
        debug_assert!(prev > 0, "BlockRef::drop on a dead state");
        if prev == 1 {
            // Synchronize with prior `Release` decrements before the final read.
            fence(Ordering::Acquire);

            // SAFETY: refcount zero gives exclusive access to the state.
            let (data_ptr, release_fn) = unsafe {
                let s = &*state_ptr.as_ptr();
                (s.data_ptr, s.release_fn)
            };

            // SAFETY: each `ArenaBlockState` owns exactly one chunk hold,
            // released here before freeing the state.
            unsafe { release_fn(data_ptr) };

            // SAFETY: refcount zero gives exclusive access for drop and free.
            unsafe { drop_in_place(state_ptr.as_ptr()) };
            let layout = Layout::new::<Self>();
            // SAFETY: matches the original allocation layout.
            unsafe { dealloc(state_ptr.as_ptr().cast::<u8>(), layout) };
        }
    }
}

/// Cold-path refcount overflow guard for `ArenaBlockState::clone`.
///
/// Mirrors `check_local_refcount` / `check_shared_refcount`: if more
/// than `usize::MAX / 2` concurrent clones were ever observed (which
/// would require an absurd number of live `BlockRef` wrappers — one
/// per byte of address space), abort to prevent the `fetch_add` from
/// wrapping through zero and triggering an unintended drop. This path
/// is physically unreachable in tested configurations, so it is
/// excluded from coverage and mutation testing per crate convention.
#[inline(always)]
#[allow(
    clippy::inline_always,
    reason = "must inline at every clone site to avoid a per-call function-call overhead on the BlockRef::clone hot path"
)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)] // Refcount overflow requires physically unreachable outstanding refs.
fn check_arena_block_state_refcount(prev: usize) {
    use crate::internal::constants::{LARGE, refcount_overflow_abort};
    if prev >= LARGE.saturating_add(LARGE) {
        refcount_overflow_abort();
    }
}

/// Releases one shared-chunk refcount given a pointer into that chunk's
/// data region.
///
/// # Safety
///
/// `data_ptr` must point into a live shared-flavor arena chunk for which
/// the caller holds one outstanding refcount that this call consumes.
unsafe fn release_chunk_ref_shared<A: Allocator + Clone>(data_ptr: NonNull<u8>) {
    // SAFETY: caller's invariant — `data_ptr` is in a live shared chunk.
    let chunk = unsafe { InSharedChunk::<_, A>::new(data_ptr) }.chunk_ptr();
    // SAFETY: caller guarantees we own one refcount on `chunk`.
    unsafe { SharedChunk::dec_ref(chunk) };
}
