// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A `NonNull<T>` known to lie inside a live chunk of a given flavor.
//!
//! This moves chunk-header recovery behind a safe `chunk_ptr` call once
//! the caller has established the invariant at construction.

use core::marker::PhantomData;
use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use super::local_chunk::LocalChunk;
use super::mask::{local_chunk_of, shared_chunk_of};
use super::shared_chunk::SharedChunk;

mod sealed {
    pub trait Sealed {}
}

/// Recover the chunk header for a value pointer of `Self`'s flavor.
///
/// Sealed to [`LocalChunk<A>`] and [`SharedChunk<A>`].
pub(crate) trait ChunkRecover: sealed::Sealed {
    /// Recover the fat chunk pointer for `value`.
    ///
    /// # Safety
    ///
    /// `value` must point into a live chunk of `Self`'s flavor. The
    /// result is valid only while that chunk stays live.
    unsafe fn recover_chunk<T: ?Sized>(value: NonNull<T>) -> NonNull<Self>;
}

impl<A: Allocator + Clone> sealed::Sealed for LocalChunk<A> {}
impl<A: Allocator + Clone> ChunkRecover for LocalChunk<A> {
    #[inline]
    unsafe fn recover_chunk<T: ?Sized>(value: NonNull<T>) -> NonNull<Self> {
        // SAFETY: caller forwards the chunk-header invariant.
        unsafe { local_chunk_of::<T, A>(value) }
    }
}

impl<A: Allocator + Clone> sealed::Sealed for SharedChunk<A> {}
impl<A: Allocator + Clone> ChunkRecover for SharedChunk<A> {
    #[inline]
    unsafe fn recover_chunk<T: ?Sized>(value: NonNull<T>) -> NonNull<Self> {
        // SAFETY: caller forwards the chunk-header invariant.
        unsafe { shared_chunk_of::<T, A>(value) }
    }
}

/// A non-null pointer to `T` known to lie inside a live chunk of `K`.
///
/// Construction is `unsafe`; chunk recovery is then safe.
///
/// `InChunk` stays `!Send + !Sync` so the smart-pointer wrapper makes
/// the cross-thread claim.
pub(crate) struct InChunk<T: ?Sized, K: ChunkRecover + ?Sized> {
    ptr: NonNull<T>,
    // `*const K` keeps dropck for `K` and leaves Send/Sync to the wrapper.
    _marker: PhantomData<*const K>,
}

impl<T: ?Sized, K: ChunkRecover + ?Sized> InChunk<T, K> {
    /// Wrap a value pointer inside a live chunk of flavor `K`.
    ///
    /// # Safety
    ///
    /// `ptr` must point into the payload of a live `K` chunk produced by
    /// an [`crate::Arena`] allocation.
    #[inline]
    pub(crate) const unsafe fn new(ptr: NonNull<T>) -> Self {
        Self { ptr, _marker: PhantomData }
    }

    /// Return the wrapped pointer.
    #[inline]
    #[must_use]
    pub(crate) const fn as_non_null(self) -> NonNull<T> {
        self.ptr
    }

    /// Return the wrapped pointer as `*mut T`.
    #[inline]
    #[must_use]
    pub(crate) const fn as_ptr(self) -> *mut T {
        self.ptr.as_ptr()
    }

    /// Borrow the pointee as `&T`.
    ///
    /// The pointee must also be initialized; `InChunk` only constrains
    /// its address.
    ///
    /// # Safety
    ///
    /// Same as [`NonNull::as_ref`].
    #[inline]
    pub(crate) unsafe fn as_ref<'a>(&self) -> &'a T {
        // SAFETY: forwarded by caller.
        unsafe { self.ptr.as_ref() }
    }

    /// Borrow the pointee as `&mut T`.
    ///
    /// # Safety
    ///
    /// Same as [`NonNull::as_mut`].
    #[inline]
    pub(crate) unsafe fn as_mut<'a>(&mut self) -> &'a mut T {
        // SAFETY: forwarded by caller.
        unsafe { self.ptr.as_mut() }
    }

    /// Recover the chunk header that owns this value.
    ///
    /// This is safe because the `InChunk` invariant guarantees the mask
    /// lands on the right live chunk.
    #[inline]
    #[must_use]
    pub(crate) fn chunk_ptr(self) -> NonNull<K> {
        // SAFETY: by `InChunk`'s construction invariant, `self.ptr`
        // points into a live `K`-flavored chunk, so `recover_chunk`'s
        // safety condition is met.
        unsafe { K::recover_chunk(self.ptr) }
    }
}

impl<T: ?Sized, K: ChunkRecover + ?Sized> Clone for InChunk<T, K> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized, K: ChunkRecover + ?Sized> Copy for InChunk<T, K> {}

/// `InChunk<T, _>` over a [`LocalChunk<A>`].
pub(crate) type InLocalChunk<T, A> = InChunk<T, LocalChunk<A>>;

/// `InChunk<T, _>` over a [`SharedChunk<A>`].
pub(crate) type InSharedChunk<T, A> = InChunk<T, SharedChunk<A>>;

#[cfg(test)]
mod tests {
    use super::{InLocalChunk, InSharedChunk};
    use crate::Arena;

    /// `Rc::clone`/`Arc::clone` can bypass the manual `Clone` body, so
    /// call the trait method explicitly here.
    #[test]
    fn in_local_chunk_explicit_clone_runs_clone_body() {
        let arena = Arena::new();
        let rc = arena.alloc_rc(7_u32);
        let raw = core::ptr::NonNull::new(rc.as_ptr().cast_mut()).unwrap();
        // SAFETY: `Rc::as_ptr` points into a live local chunk for as long as `rc` is alive.
        let in_chunk: InLocalChunk<u32, allocator_api2::alloc::Global> = unsafe { InLocalChunk::new(raw) };
        let cloned = Clone::clone(&in_chunk);
        assert_eq!(cloned.as_ptr(), in_chunk.as_ptr());
    }

    #[test]
    fn in_shared_chunk_explicit_clone_runs_clone_body() {
        let arena = Arena::new();
        let arc = arena.alloc_arc(11_u32);
        let raw = core::ptr::NonNull::new(arc.as_ptr().cast_mut()).unwrap();
        // SAFETY: `Arc::as_ptr` points into a live shared chunk.
        let in_chunk: InSharedChunk<u32, allocator_api2::alloc::Global> = unsafe { InSharedChunk::new(raw) };
        let cloned = Clone::clone(&in_chunk);
        assert_eq!(cloned.as_ptr(), in_chunk.as_ptr());
    }
}
