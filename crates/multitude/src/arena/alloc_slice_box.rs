// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `Box<[T]>` slice allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself.

use allocator_api2::alloc::{AllocError, Allocator};

use super::{AllocFlavor, Arena, expect_alloc};
use crate::r#box::Box;

impl<A: Allocator + Clone> Arena<A> {
    /// Copy `slice` into the arena and return a [`Box<[T], A>`](crate::Box).
    ///
    /// The returned smart pointer is owned and mutable; its `Drop` runs
    /// `T::drop` on each element immediately when the smart pointer is
    /// dropped.
    ///
    /// Available only with the `dst` Cargo feature, which pulls in the
    /// `ptr_meta` crate to polyfill stable `core::ptr::metadata`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_copy_box`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_slice_copy_box<T: Copy>(&self, slice: impl AsRef<[T]>) -> Box<[T], A> {
        expect_alloc(self.try_alloc_slice_copy_box(slice))
    }

    /// Fallible variant of [`Self::alloc_slice_copy_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_slice_copy_box<T: Copy>(&self, slice: impl AsRef<[T]>) -> Result<Box<[T], A>, AllocError> {
        let slice = slice.as_ref();
        let ptr = self.try_alloc_slice_local_copy(slice, AllocFlavor::Box)?;
        // SAFETY: helper initialized the slice and bumped the refcount for this Box.
        Ok(Box::from_owned_in_chunk_unsized(unsafe {
            crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr)
        }))
    }

    /// Clone every element of `slice` into the arena and return an
    /// owned, mutable [`Box<[T], A>`](crate::Box).
    ///
    /// Available only with the `dst` Cargo feature.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_clone_box`] for a fallible variant.
    ///
    /// May panic if `T::clone` panics; already-cloned elements are dropped before the
    /// panic propagates.
    #[inline]
    pub fn alloc_slice_clone_box<T: Clone>(&self, slice: impl AsRef<[T]>) -> Box<[T], A> {
        expect_alloc(self.try_alloc_slice_clone_box(slice))
    }

    /// Fallible variant of [`Self::alloc_slice_clone_box`].
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
    pub fn try_alloc_slice_clone_box<T: Clone>(&self, slice: impl AsRef<[T]>) -> Result<Box<[T], A>, AllocError> {
        let ptr = self.try_alloc_slice_local_clone_inner(slice.as_ref(), AllocFlavor::Box)?;
        // SAFETY: helper initialized the slice and bumped the refcount for this Box.
        Ok(Box::from_owned_in_chunk_unsized(unsafe {
            crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr)
        }))
    }

    /// Allocate a slice of `len` elements, with element `i` produced by `f(i)`.
    ///
    /// Returns an owned, mutable [`Box<[T], A>`](crate::Box).
    ///
    /// Available only with the `dst` Cargo feature.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_with_box`] for a fallible variant.
    ///
    /// If `f` panics, already-initialized elements are dropped (drop guard) and the
    /// panic propagates.
    #[inline]
    pub fn alloc_slice_fill_with_box<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Box<[T], A> {
        expect_alloc(self.try_alloc_slice_fill_with_box(len, f))
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with_box`].
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
    pub fn try_alloc_slice_fill_with_box<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<Box<[T], A>, AllocError> {
        let ptr = self.try_alloc_slice_local_fill_with_inner(len, AllocFlavor::Box, f)?;
        // SAFETY: helper initialized the slice and bumped the refcount for this Box.
        Ok(Box::from_owned_in_chunk_unsized(unsafe {
            crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr)
        }))
    }

    /// Allocate a slice and fill it with values pulled from `iter`.
    ///
    /// Returns an owned, mutable [`Box<[T], A>`](crate::Box).
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_iter_box`] for a fallible variant.
    ///
    /// May also panic if the iterator yields fewer elements than its
    /// `ExactSizeIterator::len()` reported.
    #[inline]
    pub fn alloc_slice_fill_iter_box<T, I>(&self, iter: I) -> Box<[T], A>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        expect_alloc(self.try_alloc_slice_fill_iter_box(iter))
    }

    /// Fallible variant of [`Self::alloc_slice_fill_iter_box`].
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
    pub fn try_alloc_slice_fill_iter_box<T, I>(&self, iter: I) -> Result<Box<[T], A>, AllocError>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let ptr = self.try_alloc_slice_local_fill_iter_inner(iter, AllocFlavor::Box)?;
        // SAFETY: helper initialized the slice and bumped the refcount for this Box.
        Ok(Box::from_owned_in_chunk_unsized(unsafe {
            crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr)
        }))
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate `len` slots and fill each via `f(i)`, returning a
    /// [`Pin<Box<[T], A>>`](core::pin::Pin). Each element is pinned
    /// to its slot.
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_slice_fill_with_box`].
    #[must_use]
    #[inline]
    pub fn alloc_slice_fill_with_box_pin<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> core::pin::Pin<Box<[T], A>>
    where
        A: 'static,
    {
        crate::r#box::Box::into_pin(self.alloc_slice_fill_with_box(len, f))
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with_box_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_slice_fill_with_box`].
    #[inline]
    pub fn try_alloc_slice_fill_with_box_pin<T, F: FnMut(usize) -> T>(
        &self,
        len: usize,
        f: F,
    ) -> Result<core::pin::Pin<Box<[T], A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_slice_fill_with_box(len, f).map(crate::r#box::Box::into_pin)
    }
}
