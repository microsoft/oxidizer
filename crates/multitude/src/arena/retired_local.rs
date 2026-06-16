// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Intrusive singly-linked list of retired local chunks.
//!
//! The arena keeps a LIFO list of [`LocalChunk`]s it has rotated out of
//! `current_local` but cannot yet recycle, because some outstanding
//! `&mut T` borrow (or [`crate::vec::Vec`] / [`crate::strings::String`]
//! handle) still references payload bytes in them. Each chunk on the
//! list logically owns one strong reference (refcount = 1) — the same
//! `+1` that the originally-retiring [`ChunkMutator`] held.
//!
//! Linkage is **intrusive**: each `LocalChunk` carries a
//! [`next`](LocalChunk) field used to thread chunks together
//! without per-retirement heap allocation. The list head holds a
//! thin `*mut u8` to the topmost chunk's header (`null` for empty);
//! the fat DST pointer is reconstructed via
//! [`LocalChunk::header_to_fat`] when the list is drained.
//!
//! Only two writers ever touch this structure:
//! * [`Arena::refill_local`](crate::Arena) pushes the rotated-out
//!   mutator's chunk.
//! * [`Arena::reset`](crate::Arena), `Arena::drop`, and the
//!   oversized-Vec-grow path drain (or splice from) the list.
//!
//! The drain is iterative *and* re-checks `head` on each pass so that
//! reentrant pushes performed by user-supplied chunk-teardown
//! destructors (drop shims invoked from `LocalChunk::teardown_and_release`)
//! never leak.

use core::cell::Cell;
use core::marker::PhantomData;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::Allocator;

use crate::internal::chunk::Chunk;
use crate::internal::chunk_mutator::ChunkMutator;
use crate::internal::local_chunk::LocalChunk;

/// LIFO list of retired local chunks linked through each chunk's
/// own [`next`](LocalChunk) field.
///
/// `head` is a thin `*mut u8` header pointer (or `null`). Carrying
/// the metadata-less form keeps the head field 8 bytes and matches
/// the encoding the cache freelist uses elsewhere in the chunk
/// provider; the fat pointer is rebuilt via `header_to_fat` when
/// chunks are popped.
pub(crate) struct RetiredLocalChunks<A: Allocator + Clone> {
    head: Cell<*mut u8>,
    /// `LocalChunk<A>` is only referenced via raw pointers from this
    /// list; this marker propagates `A`'s `Send` / auto-traits to the
    /// list type.
    _marker: PhantomData<ChunkMutator<LocalChunk<A>>>,
}

// SAFETY: when `ChunkMutator<LocalChunk<A>>` is `Send`, so is this
// list — every node logically holds the same `+1` a mutator does and
// is single-owner; moving the arena between threads moves every node
// with it. `!Sync` is inherited from `Cell`, matching the bound
// `Arena` needs.
unsafe impl<A: Allocator + Clone + Send> Send for RetiredLocalChunks<A> {}

impl<A: Allocator + Clone> RetiredLocalChunks<A> {
    #[inline]
    pub(crate) const fn new() -> Self {
        Self {
            head: Cell::new(ptr::null_mut()),
            _marker: PhantomData,
        }
    }

    /// Retire `mutator`'s chunk by linking its header onto the list
    /// head. Empty mutators are a no-op (nothing to retire).
    ///
    /// The mutator's `+1` becomes the list's; its `Drop` is bypassed
    /// (via [`ChunkMutator::forget_into_chunk`]) so the chunk is *not*
    /// torn down here. Teardown happens when the list is drained.
    #[inline]
    pub(crate) fn push(&self, mutator: ChunkMutator<LocalChunk<A>>) {
        let Some(chunk) = mutator.forget_into_chunk() else {
            return;
        };
        // SAFETY: we just took ownership of the +1 on `chunk`, so it
        // is live and uniquely owned. `set_next` is a single
        // `Cell::replace` on the chunk's own header field.
        unsafe {
            let prev_head = self.head.replace(chunk.cast::<u8>().as_ptr());
            LocalChunk::set_next(chunk, prev_head);
        }
    }

    /// Drop every retained chunk. Iterative on two levels:
    /// * The inner `while let` walks the chain one node at a time
    ///   (re-using the chunk's own `next` link to advance), so a
    ///   long list never overflows the stack via recursive `Drop`.
    /// * The outer `loop` re-checks `self.head` after each chain
    ///   drain so reentrant pushes performed by chunk-teardown drop
    ///   shims are captured rather than leaked.
    pub(crate) fn clear(&self) {
        loop {
            let mut cur = self.head.replace(ptr::null_mut());
            if cur.is_null() {
                return;
            }
            while !cur.is_null() {
                // SAFETY: while a node sits in this list we own its
                // `+1`, so the chunk allocation is live. `header_to_fat`
                // reads the header's `capacity` field through the
                // live allocation.
                unsafe {
                    let fat = LocalChunk::<A>::header_to_fat(cur);
                    let chunk = NonNull::new_unchecked(fat);
                    // Detach the node *before* releasing its
                    // refcount: a panicking drop shim invoked from
                    // `teardown_and_release` must not see this chunk
                    // re-entrantly through the retired list. We also
                    // clear the field so the chunk, if recycled into
                    // the provider cache, starts in a clean state.
                    let next = LocalChunk::set_next(chunk, ptr::null_mut());
                    Self::release_retired_chunk(chunk);
                    cur = next;
                }
            }
        }
    }

    /// Release one strong reference on a chunk that has been
    /// unlinked from the list. The list logically holds exactly one
    /// `+1` per node, so `dec_ref` always returns `true` here; we
    /// fall through to `teardown_and_release` to run any drop shims
    /// and route the chunk back to the cache (or the system
    /// allocator).
    ///
    /// # Safety
    ///
    /// Caller must have just removed `chunk` from this list and must
    /// not retain any further references to it.
    #[inline]
    unsafe fn release_retired_chunk(chunk: NonNull<LocalChunk<A>>) {
        use crate::internal::chunk_ops::ChunkOps;
        // SAFETY: caller hands over the list's `+1`; LocalChunk
        // refcounts are bounded to 0/1, so this decrement always
        // hits zero.
        unsafe {
            let chunk_ref = chunk.as_ref();
            let last = chunk_ref.dec_ref();
            debug_assert!(last, "retired LocalChunk refcount must be 1; dec_ref must hit zero");
            if last {
                <LocalChunk<A> as ChunkOps>::teardown_and_release(chunk);
            }
        }
    }
}

impl<A: Allocator + Clone> Drop for RetiredLocalChunks<A> {
    fn drop(&mut self) {
        self.clear();
    }
}
