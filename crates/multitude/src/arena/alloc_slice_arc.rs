// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `Arc<[T]>` slice allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself.

use allocator_api2::alloc::{AllocError, Allocator};

use super::{Arena, expect_alloc};
use crate::arc::Arc;

impl<A: Allocator + Clone> Arena<A> {
    /// Copy `slice` into a `Shared`-flavor chunk and return an [`Arc`].
    ///
    /// The returned [`Arc`] is safe for cross-thread sharing.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_copy_arc`] for a fallible variant.
    #[inline]
    pub fn alloc_slice_copy_arc<T: Copy + Send + Sync>(&self, slice: impl AsRef<[T]>) -> Arc<[T], A>
    where
        A: Send + Sync,
    {
        expect_alloc(self.try_alloc_slice_copy_arc(slice))
    }

    /// Fallible variant of [`Self::alloc_slice_copy_arc`].
    ///
    /// Returns Err([`AllocError`]) instead of panicking if the backing
    /// allocator fails.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_slice_copy_arc<T: Copy + Send + Sync>(&self, slice: impl AsRef<[T]>) -> Result<Arc<[T], A>, AllocError>
    where
        A: Send + Sync,
    {
        let slice = slice.as_ref();
        let ptr = self.try_alloc_slice_shared_copy(slice)?;
        // SAFETY: helper initialized the slice and accounted for this Arc;
        // OwnedInSharedChunk records both invariants for the safe Arc constructor.
        let owned = unsafe { crate::internal::owned_in_chunk::OwnedInSharedChunk::from_raw_alloc(ptr) };
        Ok(Arc::from_owned_in_chunk(owned))
    }

    /// Clone every element of `slice` into a `Shared`-flavor chunk and return an [`Arc`].
    ///
    /// The returned [`Arc`] is safe for cross-thread sharing.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_clone_arc`] for a fallible variant.
    ///
    /// May panic if `T::clone` panics; already-cloned elements are dropped before the
    /// panic propagates.
    #[inline]
    pub fn alloc_slice_clone_arc<T: Clone + Send + Sync>(&self, slice: impl AsRef<[T]>) -> Arc<[T], A>
    where
        A: Send + Sync,
    {
        expect_alloc(self.try_alloc_slice_clone_arc(slice))
    }

    /// Fallible variant of [`Self::alloc_slice_clone_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// May panic if `T::clone` panics; already-cloned elements are
    /// dropped before the panic propagates.
    #[inline]
    pub fn try_alloc_slice_clone_arc<T: Clone + Send + Sync>(&self, slice: impl AsRef<[T]>) -> Result<Arc<[T], A>, AllocError>
    where
        A: Send + Sync,
    {
        let ptr = self.try_alloc_slice_shared_clone_inner(slice.as_ref())?;
        // SAFETY: helper initialized the slice and accounted for this Arc.
        Ok(Arc::from_owned_in_chunk(unsafe {
            crate::internal::owned_in_chunk::OwnedInSharedChunk::from_raw_alloc(ptr)
        }))
    }

    /// Allocate a slice of `len` elements in a `Shared`-flavor chunk via `f(i)`.
    ///
    /// Element `i` is produced by `f(i)`. The returned [`Arc`] is safe
    /// for cross-thread sharing.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_with_arc`] for a fallible variant.
    ///
    /// If `f` panics, already-initialized elements are dropped (drop guard) and the
    /// panic propagates.
    #[inline]
    pub fn alloc_slice_fill_with_arc<T, F>(&self, len: usize, f: F) -> Arc<[T], A>
    where
        T: Send + Sync,
        F: FnMut(usize) -> T,
        A: Send + Sync,
    {
        expect_alloc(self.try_alloc_slice_fill_with_arc(len, f))
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// If `f` panics, already-initialized elements are dropped and the
    /// panic propagates.
    #[inline]
    pub fn try_alloc_slice_fill_with_arc<T, F>(&self, len: usize, f: F) -> Result<Arc<[T], A>, AllocError>
    where
        T: Send + Sync,
        F: FnMut(usize) -> T,
        A: Send + Sync,
    {
        let ptr = self.try_alloc_slice_shared_fill_with_inner(len, f)?;
        // SAFETY: helper initialized the slice and accounted for this Arc.
        Ok(Arc::from_owned_in_chunk(unsafe {
            crate::internal::owned_in_chunk::OwnedInSharedChunk::from_raw_alloc(ptr)
        }))
    }

    /// Allocate a slice in a `Shared`-flavor chunk and fill it from `iter`.
    ///
    /// Returns an [`Arc`] safe for cross-thread sharing.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_iter_arc`] for a fallible variant.
    ///
    /// May also panic if the iterator yields fewer elements than its
    /// `ExactSizeIterator::len()` reported.
    #[inline]
    pub fn alloc_slice_fill_iter_arc<T, I>(&self, iter: I) -> Arc<[T], A>
    where
        T: Send + Sync,
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
        A: Send + Sync,
    {
        expect_alloc(self.try_alloc_slice_fill_iter_arc(iter))
    }

    /// Fallible variant of [`Self::alloc_slice_fill_iter_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// Panics if the iterator yields fewer elements than its
    /// `ExactSizeIterator::len()` reported.
    #[inline]
    pub fn try_alloc_slice_fill_iter_arc<T, I>(&self, iter: I) -> Result<Arc<[T], A>, AllocError>
    where
        T: Send + Sync,
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
        A: Send + Sync,
    {
        let ptr = self.try_alloc_slice_shared_fill_iter_inner(iter)?;
        // SAFETY: helper initialized the slice and accounted for this Arc.
        Ok(Arc::from_owned_in_chunk(unsafe {
            crate::internal::owned_in_chunk::OwnedInSharedChunk::from_raw_alloc(ptr)
        }))
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate `len` slots and fill each via `f(i)`, returning a
    /// [`Pin<Arc<[T], A>>`](core::pin::Pin). Pin is preserved across
    /// `Arc::clone` and across threads.
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_slice_fill_with_arc`].
    #[must_use]
    #[inline]
    pub fn alloc_slice_fill_with_arc_pin<T, F>(&self, len: usize, f: F) -> core::pin::Pin<Arc<[T], A>>
    where
        T: Send + Sync,
        F: FnMut(usize) -> T,
        A: Send + Sync + 'static,
    {
        crate::arc::Arc::into_pin(self.alloc_slice_fill_with_arc(len, f))
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with_arc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_slice_fill_with_arc`].
    #[inline]
    pub fn try_alloc_slice_fill_with_arc_pin<T, F>(&self, len: usize, f: F) -> Result<core::pin::Pin<Arc<[T], A>>, AllocError>
    where
        T: Send + Sync,
        F: FnMut(usize) -> T,
        A: Send + Sync + 'static,
    {
        self.try_alloc_slice_fill_with_arc(len, f).map(crate::arc::Arc::into_pin)
    }
}
