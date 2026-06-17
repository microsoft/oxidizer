// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`ChunkRef`] — a RAII handle for a single strong reference on a
//! [`SharedChunk`].
//!
//! Centralizes the "+1 on a chunk that must be released exactly once,
//! even on panic" pattern used by smart pointers and in-flight slot
//! initialization. One machine word, `!Send`/`!Sync`, and inhibits
//! implicit `Copy`/`Clone` so the +1 ownership is linear.

use core::marker::PhantomData;
use core::mem;
use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use super::shared_chunk::SharedChunk;

/// Owns a single strong reference on a [`SharedChunk`]; releases the
/// ref on drop.
pub(crate) struct ChunkRef<A: Allocator + Clone> {
    chunk: NonNull<SharedChunk<A>>,
    _phantom: PhantomData<*const SharedChunk<A>>,
}

impl<A: Allocator + Clone> ChunkRef<A> {
    /// Adopts a pre-existing +1 strong reference on `chunk`.
    ///
    /// # Safety
    ///
    /// Caller must own a strong reference on `chunk` whose ownership
    /// is transferred to this `ChunkRef`. After this call the caller
    /// must not release that same reference through any other path.
    #[inline]
    pub(crate) unsafe fn adopt(chunk: NonNull<SharedChunk<A>>) -> Self {
        Self {
            chunk,
            _phantom: PhantomData,
        }
    }

    /// Recovers a [`ChunkRef`] from a pointer to a value living inside
    /// the chunk's payload. Adopts a +1 the caller owns.
    ///
    /// # Safety
    ///
    /// - `value` must point inside a 64K-aligned `SharedChunk<A>` and
    ///   lie within the first `CHUNK_ALIGN` bytes of that chunk's
    ///   allocation.
    /// - Caller must own a strong reference on that chunk whose
    ///   ownership is transferred to this `ChunkRef`.
    #[inline]
    pub(crate) unsafe fn from_value_ptr<T: ?Sized>(value: NonNull<T>) -> Self {
        // SAFETY: caller contract; `header_from_value_ptr` returns a
        // thin pointer with full chunk provenance via `with_addr`.
        unsafe {
            let header = SharedChunk::<A>::header_from_value_ptr(value.cast::<u8>());
            let chunk_fat = SharedChunk::<A>::header_to_fat(header.as_ptr());
            Self {
                chunk: NonNull::new_unchecked(chunk_fat),
                _phantom: PhantomData,
            }
        }
    }

    /// Cancels release-on-drop and returns the raw chunk pointer with
    /// the +1 still live. Use when ownership of the +1 is being
    /// handed to another holder (e.g. a freshly-constructed `Box`).
    #[inline]
    pub(crate) fn forget(self) -> NonNull<SharedChunk<A>> {
        let chunk = self.chunk;
        mem::forget(self);
        chunk
    }
}

impl<A: Allocator + Clone> Drop for ChunkRef<A> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: this `ChunkRef` owns exactly one +1 strong reference
        // on `chunk` by its construction contract; we are releasing
        // that reference now.
        unsafe {
            SharedChunk::<A>::release_one_ref(self.chunk);
        }
    }
}
