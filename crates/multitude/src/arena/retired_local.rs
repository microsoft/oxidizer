// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Intrusive singly-linked list of retired chunks held alive for `&Arena`.
//!
//! The arena keeps a LIFO list of [`Chunk`]s it has rotated out of
//! `current` but cannot recycle yet, because they handed out arena-lifetime
//! [`Alloc`](crate::Alloc) handles (or growable-collection buffers — see
//! [`crate::vec::Vec`] / [`crate::strings::String`]) that borrow the chunk
//! storage for the whole `&Arena` lifetime. Each chunk on the list holds one
//! strong reference — the same `+1` that the originally-retiring
//! [`ChunkMutator`] held — plus, possibly, additional references owned by
//! escaped `Arc`/`Box`/`Rc` handles living in the same chunk.
//!
//! Linkage is **intrusive**: each `Chunk` carries a [`next`](Chunk::next)
//! field used to thread chunks together without per-retirement heap
//! allocation. The list head holds a thin `*mut u8` to the topmost chunk's
//! header (`null` for empty); the fat DST pointer is reconstructed via
//! [`Chunk::header_to_fat`] when the list is drained.
//!
//! Only two writers ever touch this structure:
//! * [`Arena::refill`](crate::Arena) pushes a rotated-out chunk that handed
//!   out an arena-lifetime reference.
//! * [`Arena::reset`](crate::Arena), `Arena::drop`, and the
//!   oversized-Vec-grow path drain (or splice from) the list.
//!
//! The drain is iterative *and* re-checks `head` on each pass so that
//! reentrant pushes performed by user-supplied chunk-teardown destructors
//! (eager `Arc`/`Box`/`Rc` drops invoked from `Chunk::teardown_and_release`)
//! never leak.

use core::cell::Cell;
use core::marker::PhantomData;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::Allocator;

use crate::internal::chunk::Chunk;
use crate::internal::chunk_mutator::ChunkMutator;

/// LIFO list of retired chunks linked through each chunk's own
/// [`next`](Chunk::next) field.
///
/// `head` is a thin `*mut u8` header pointer (or `null`). Carrying the
/// metadata-less form keeps the head field 8 bytes and matches the encoding
/// the cache freelist uses elsewhere in the chunk provider; the fat pointer
/// is rebuilt via `header_to_fat` when chunks are popped.
pub(in crate::arena) struct RetiredLocalChunks<A: Allocator + Clone> {
    head: Cell<*mut u8>,
    /// `Chunk<A>` is only referenced via raw pointers from this list; this
    /// marker propagates `A`'s `Send` / auto-traits to the list type.
    _marker: PhantomData<ChunkMutator<A>>,
}

// SAFETY: when `ChunkMutator<A>` is `Send`, so is this list — every node
// logically holds the same `+1` a mutator does and is single-owner; moving
// the arena between threads moves every node with it. `!Sync` is inherited
// from `Cell`, matching the bound `Arena` needs.
unsafe impl<A: Allocator + Clone + Send> Send for RetiredLocalChunks<A> {}

impl<A: Allocator + Clone> RetiredLocalChunks<A> {
    #[inline]
    pub(in crate::arena) const fn new() -> Self {
        Self {
            head: Cell::new(ptr::null_mut()),
            _marker: PhantomData,
        }
    }

    /// Retire `mutator`'s chunk by linking its header onto the list head.
    /// Empty mutators are a no-op (nothing to retire).
    ///
    /// The mutator's `+1` becomes the list's; its `Drop` is bypassed (via
    /// [`ChunkMutator::forget_into_chunk`]) so the chunk stays pinned for the
    /// `&Arena` borrow (its `Alloc` handles already run their own destructors).
    #[inline]
    pub(in crate::arena) fn push(&self, mutator: ChunkMutator<A>) {
        let Some(chunk) = mutator.forget_into_chunk() else {
            return;
        };
        // SAFETY: we just took ownership of the +1 on `chunk`, so it is live
        // and the owning thread has exclusive access to its `next` link.
        unsafe {
            let prev_head = self.head.replace(chunk.cast::<u8>().as_ptr());
            Chunk::set_next(chunk, prev_head);
        }
    }

    /// Drop every retained chunk. Iterative on two levels:
    /// * The inner `while` walks the chain one node at a time (re-using the
    ///   chunk's own `next` link to advance), so a long list never overflows
    ///   the stack via recursive `Drop`.
    /// * The outer `loop` re-checks `self.head` after each chain drain so
    ///   reentrant pushes performed by chunk-teardown drop shims are
    ///   captured rather than leaked.
    pub(in crate::arena) fn clear(&self) {
        loop {
            let mut cur = self.head.replace(ptr::null_mut());
            if cur.is_null() {
                return;
            }
            while !cur.is_null() {
                // SAFETY: while a node sits in this list we own its `+1`, so
                // the chunk allocation is live. `header_to_fat` reads the
                // header's `capacity` field through the live allocation.
                unsafe {
                    let fat = Chunk::<A>::header_to_fat(cur);
                    let chunk = NonNull::new_unchecked(fat);
                    // Detach the node *before* releasing its refcount: a
                    // panicking drop shim invoked from `teardown_and_release`
                    // must not see this chunk re-entrantly through the list.
                    // We also clear the field so the chunk, if recycled into
                    // the provider cache, starts in a clean state.
                    let next = Chunk::next(chunk);
                    Chunk::set_next(chunk, ptr::null_mut());
                    Self::release_retired_chunk(chunk);
                    cur = next;
                }
            }
        }
    }

    /// Release the list's `+1` on a chunk that has been unlinked.
    ///
    /// # Safety
    ///
    /// Caller must have just removed `chunk` from this list and must not
    /// retain any further references to it.
    #[inline]
    unsafe fn release_retired_chunk(chunk: NonNull<Chunk<A>>) {
        // SAFETY: caller hands over the list's `+1`. Release the `+1`; if it
        // was the last reference, route through teardown to recycle.
        unsafe {
            if chunk.as_ref().dec_ref() {
                Chunk::teardown_and_release(chunk);
            }
        }
    }
}

impl<A: Allocator + Clone> Drop for RetiredLocalChunks<A> {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    use super::*;

    /// Pushing a chunkless (empty) mutator is a no-op: `forget_into_chunk`
    /// yields `None`, so `push` early-returns without retaining anything.
    #[test]
    fn push_chunkless_mutator_is_noop() {
        let retired = RetiredLocalChunks::<Global>::new();
        retired.push(ChunkMutator::<Global>::empty());
        // Nothing was retained; the list drops cleanly.
    }
}
