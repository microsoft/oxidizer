// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Single-slot interior-mutable holder for a [`ChunkMutator`].
//!
//! [`CurrentChunk`] wraps `UnsafeCell<ChunkMutator<C>>` for
//! [`Arena`](crate::Arena)'s hot path.
//!
//! # Soundness contract
//!
//! `CurrentChunk` does not track borrows at runtime. The holder must ensure:
//!
//! 1. Single-threaded access (`!Sync` holder).
//! 2. No re-entry during borrow: the shared reference returned by
//!    [`borrow`](CurrentChunk::borrow) must not be held across any
//!    `replace`/`drop_replace` on the same cell.

use core::cell::UnsafeCell;
use core::ptr;

use allocator_api2::alloc::Allocator;

use super::chunk_mutator::ChunkMutator;

/// Interior-mutable single-slot holder for a [`ChunkMutator`]. See
/// module docs for the soundness contract.
#[repr(transparent)]
pub(crate) struct CurrentChunk<A: Allocator + Clone>(UnsafeCell<ChunkMutator<A>>);

impl<A: Allocator + Clone> CurrentChunk<A> {
    /// Wrap an owned mutator.
    #[inline]
    pub(crate) const fn new(mutator: ChunkMutator<A>) -> Self {
        Self(UnsafeCell::new(mutator))
    }

    /// Borrow the mutator until the next `replace` / `drop_replace`.
    #[expect(clippy::inline_always, reason = "hot-path entry; must inline fully for arena performance")]
    #[inline(always)]
    pub(crate) fn borrow(&self) -> &ChunkMutator<A> {
        // SAFETY: The holder is !Sync (single-threaded access) and the
        // documented "no re-entry during borrow" contract ensures no
        // overlapping mutable access via `replace`/`drop_replace`.
        unsafe { &*self.0.get() }
    }

    /// Replace the contained mutator and return the previous one.
    #[inline]
    pub(crate) fn replace(&self, new: ChunkMutator<A>) -> ChunkMutator<A> {
        // SAFETY: the holder is `!Sync`, so access is single-threaded; the
        // caller must not hold any reference handed out by `borrow` across this
        // call, so reading the old mutator and writing the new one through the
        // `UnsafeCell` introduces no aliasing.
        unsafe {
            let slot = self.0.get();
            let prev = ptr::read(slot);
            ptr::write(slot, new);
            prev
        }
    }

    /// Replace the contained mutator, dropping the previous one in
    /// place. Equivalent to `let _ = self.replace(new);`.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // body→() leaks chunk refcount → OOM
    pub(crate) fn drop_replace(&self, new: ChunkMutator<A>) {
        let _old = self.replace(new);
    }

    /// Get a mutable reference to the contained mutator. Requires
    /// `&mut self`, so the borrow checker enforces exclusion.
    #[inline]
    pub(crate) fn get_mut(&mut self) -> &mut ChunkMutator<A> {
        self.0.get_mut()
    }
}
