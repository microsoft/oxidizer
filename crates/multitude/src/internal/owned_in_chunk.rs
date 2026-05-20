// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `InChunk<T, K>` plus one owned chunk refcount.
//!
//! Smart-pointer handles keep this in `Drop` so the chunk stays live
//! until conversion or release.

use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use super::in_chunk::{InLocalChunk, InSharedChunk};
use super::local_chunk::LocalChunk;
use super::shared_chunk::SharedChunk;

/// An [`InChunk<T, K>`] that owns one `+1` on the containing chunk.
pub(crate) struct OwnedInLocalChunk<T: ?Sized, A: Allocator + Clone> {
    // The meaningful drop is the chunk refcount release, which we do ourselves.
    ptr: ManuallyDrop<InLocalChunk<T, A>>,
    _marker: PhantomData<A>,
}

impl<T: ?Sized, A: Allocator + Clone> OwnedInLocalChunk<T, A> {
    /// Wrap a freshly allocated value pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must point into a live local chunk, address an initialized
    /// `T`, and carry one `+1` reserved for this `OwnedInLocalChunk`.
    #[inline]
    pub(crate) unsafe fn from_raw_alloc(ptr: NonNull<T>) -> Self {
        // SAFETY: caller forwards both invariants.
        let in_chunk = unsafe { InLocalChunk::new(ptr) };
        Self {
            ptr: ManuallyDrop::new(in_chunk),
            _marker: PhantomData,
        }
    }

    /// Consume self and transfer the held refcount to the returned
    /// [`InChunk`].
    #[inline]
    #[must_use]
    pub(crate) fn into_in_chunk(self) -> InLocalChunk<T, A> {
        let mut me = ManuallyDrop::new(self);
        // SAFETY: `ManuallyDrop` suppresses our `Drop` while we move out `ptr`.
        unsafe { ManuallyDrop::take(&mut me.ptr) }
    }
}

impl<T: ?Sized, A: Allocator + Clone> Drop for OwnedInLocalChunk<T, A> {
    fn drop(&mut self) {
        let chunk = self.ptr.chunk_ptr();
        // SAFETY: by construction `self` owns one `+1` on `chunk`'s
        // refcount; this release balances that increment.
        unsafe { LocalChunk::dec_ref(chunk) };
    }
}

/// Shared-chunk variant of [`OwnedInLocalChunk`].
pub(crate) struct OwnedInSharedChunk<T: ?Sized, A: Allocator + Clone> {
    ptr: ManuallyDrop<InSharedChunk<T, A>>,
    _marker: PhantomData<A>,
}

impl<T: ?Sized, A: Allocator + Clone> OwnedInSharedChunk<T, A> {
    /// Wrap a freshly allocated value pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must point into a live shared chunk, address an initialized
    /// `T`, and carry one `+1` reserved for this `OwnedInSharedChunk`.
    #[inline]
    pub(crate) unsafe fn from_raw_alloc(ptr: NonNull<T>) -> Self {
        // SAFETY: caller forwards both invariants.
        let in_chunk = unsafe { InSharedChunk::new(ptr) };
        Self {
            ptr: ManuallyDrop::new(in_chunk),
            _marker: PhantomData,
        }
    }

    /// Consume self and transfer the held refcount to the returned
    /// [`InChunk`].
    #[inline]
    #[must_use]
    pub(crate) fn into_in_chunk(self) -> InSharedChunk<T, A> {
        let mut me = ManuallyDrop::new(self);
        // SAFETY: ManuallyDrop guarantees Drop won't run.
        unsafe { ManuallyDrop::take(&mut me.ptr) }
    }
}

impl<T: ?Sized, A: Allocator + Clone> Drop for OwnedInSharedChunk<T, A> {
    fn drop(&mut self) {
        let chunk = self.ptr.chunk_ptr();
        // SAFETY: by construction `self` owns one `+1` on `chunk`'s
        // refcount; this release balances that increment.
        unsafe { SharedChunk::dec_ref(chunk) };
    }
}
