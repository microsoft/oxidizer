// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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

use alloc::boxed::Box as StdBox;
use core::fmt;
use core::mem::MaybeUninit;
use core::num::NonZero;
use core::ptr::NonNull;
use core::sync::atomic::{self, AtomicUsize};

use allocator_api2::alloc::Allocator;
use bytesbuf::BytesBuf;
use bytesbuf::mem::{Block, BlockRef, BlockRefDynamic, BlockRefVTable, BlockSize, Memory};

use crate::{Arc, Arena};

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
        let Some(min_bytes_nz) = NonZero::new(min_bytes) else {
            return BytesBuf::new();
        };

        let len_u32 = BlockSize::try_from(min_bytes_nz.get())
            .expect("multitude::Arena::reserve: min_bytes exceeds u32::MAX, which is the bytesbuf block size limit");
        // SAFETY: `min_bytes_nz` is non-zero, and `len_u32` came from a successful
        // conversion of that non-zero `usize`, so it is non-zero as well.
        let len_nz = unsafe { NonZero::new_unchecked(len_u32) };

        let arc: Arc<[MaybeUninit<u8>], A> = self.alloc_uninit_slice_arc::<u8>(min_bytes_nz.get());

        // The pointer is to the first element of the arena-resident slice. The
        // `Arc` is moved into the state below, keeping the slice (and its hosting
        // chunk) alive until the last `BlockRef` is dropped. We obtain the raw
        // pointer via `Arc::as_ptr` rather than `&*arc` to avoid creating a
        // `SharedReadOnly` retag on the slice — bytesbuf later writes into the
        // returned buffer, which requires a `Unique`-compatible tag.
        let ptr = {
            let slice_ptr: *const [MaybeUninit<u8>] = Arc::as_ptr(&arc);
            // SAFETY: slice_ptr is non-null (returned by a valid Arc).
            unsafe { NonNull::new_unchecked(slice_ptr.cast::<MaybeUninit<u8>>().cast_mut()) }
        };

        let state = StdBox::new(ArenaBlockState::<A> {
            _arc: arc,
            ref_count: AtomicUsize::new(1),
        });
        let state_ptr = NonNull::from(StdBox::leak(state));

        // SAFETY: `state_ptr` was just produced by `Box::leak` and remains valid for
        // reads (and ultimately for the `Box::from_raw` reclamation in
        // `BlockRefDynamic::drop`) until that final drop runs.
        let block_ref = unsafe { BlockRef::new(state_ptr, vtable::<A>()) };

        // SAFETY: We hold the only `BlockRef` (refcount initialized to 1) and the
        // `Arc` stored inside the state keeps the backing memory alive for as long
        // as any clone of `block_ref` exists.
        let block = unsafe { Block::new(ptr, len_nz, block_ref) };

        BytesBuf::from_blocks([block])
    }
}

/// Per-block state holding the arena `Arc` that keeps the backing memory alive,
/// plus an atomic refcount shared by all clones of the issued [`BlockRef`].
struct ArenaBlockState<A: Allocator + Clone + Send + Sync + 'static> {
    /// The `Arc` is kept alive by the state so that the underlying arena chunk
    /// remains live for the entire lifetime of any [`BlockRef`] clone.
    _arc: Arc<[MaybeUninit<u8>], A>,
    ref_count: AtomicUsize,
}

impl<A: Allocator + Clone + Send + Sync + 'static> fmt::Debug for ArenaBlockState<A> {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArenaBlockState")
            .field("ref_count", &self.ref_count.load(atomic::Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

// SAFETY: All shared state mutation goes through atomic operations on `ref_count`,
// and the `Arc<[MaybeUninit<u8>], A>` is itself `Send + Sync` under the trait bounds
// on `A`, so dropping the state on any thread is sound.
unsafe impl<A: Allocator + Clone + Send + Sync + 'static> BlockRefDynamic for ArenaBlockState<A> {
    type State = Self;

    fn clone(state_ptr: NonNull<Self::State>) -> NonNull<Self::State> {
        #[cfg_attr(coverage_nightly, coverage(off))]
        #[inline(never)]
        #[cold]
        fn refcount_overflow() -> ! {
            crate::internal::constants::refcount_overflow_abort()
        }
        // SAFETY: `state_ptr` is valid for reads for as long as any `BlockRef`
        // referencing this state is alive, and we only access it via shared refs.
        let state = unsafe { state_ptr.as_ref() };
        let prev = state.ref_count.fetch_add(1, atomic::Ordering::Relaxed);
        // A refcount that wraps back to zero would let a live `BlockRef` race
        // with the teardown in `drop`, causing a use-after-free. Mirror the
        // crate's chunk refcounts (see `SharedChunk::inc_ref`) and abort.
        if prev == usize::MAX {
            refcount_overflow();
        }
        state_ptr
    }

    fn drop(state_ptr: NonNull<Self::State>) {
        // SAFETY: `state_ptr` is valid for reads while this drop runs.
        let state = unsafe { state_ptr.as_ref() };
        if state.ref_count.fetch_sub(1, atomic::Ordering::Release) != 1 {
            return;
        }
        // Ensure we observe all writes from other threads before tearing down.
        atomic::fence(atomic::Ordering::Acquire);
        // SAFETY: The state was created via `Box::leak`, this is the last
        // outstanding reference (refcount just dropped to zero), and the
        // pointer has not been freed yet.
        drop(unsafe { StdBox::from_raw(state_ptr.as_ptr()) });
    }
}

fn vtable<A: Allocator + Clone + Send + Sync + 'static>() -> &'static BlockRefVTable<ArenaBlockState<A>> {
    &const { BlockRefVTable::<ArenaBlockState<A>>::from_trait() }
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    use super::*;

    /// Drives `ArenaBlockState::clone` with the refcount pre-set to
    /// `usize::MAX` so the next increment wraps to zero and the overflow guard
    /// fires (covering the `refcount_overflow()` call site). Under `cfg(test)`
    /// `refcount_overflow_abort` panics instead of aborting, letting
    /// `#[should_panic]` observe this otherwise process-terminating path.
    #[test]
    #[should_panic(expected = "refcount overflow")]
    fn clone_aborts_on_refcount_overflow() {
        let arena = Arena::new();
        let arc: Arc<[MaybeUninit<u8>], Global> = arena.alloc_uninit_slice_arc::<u8>(4);
        let mut state = StdBox::new(ArenaBlockState::<Global> {
            _arc: arc,
            ref_count: AtomicUsize::new(usize::MAX),
        });
        let state_ptr = NonNull::from(&mut *state);
        let _ = <ArenaBlockState<Global> as BlockRefDynamic>::clone(state_ptr);
    }
}
