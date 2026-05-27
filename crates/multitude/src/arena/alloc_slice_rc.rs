// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `Rc<[T]>` slice allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself.

use allocator_api2::alloc::{AllocError, Allocator};

use super::{AllocFlavor, Arena, expect_alloc};
use crate::rc::Rc;

impl<A: Allocator + Clone> Arena<A> {
    /// Copy `slice` into the arena, returning an immutable smart pointer.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_copy_rc`] for a fallible variant.
    #[inline]
    pub fn alloc_slice_copy_rc<T: Copy>(&self, slice: impl AsRef<[T]>) -> Rc<[T], A> {
        let slice = slice.as_ref();
        let ptr = expect_alloc(self.try_alloc_slice_local_copy::<_, true>(slice, AllocFlavor::Rc));
        // SAFETY: helper initialized the slice and bumped the refcount for this Rc.
        Rc::from_owned_in_chunk(unsafe { crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr) })
    }

    /// Fallible variant of [`Self::alloc_slice_copy_rc`].
    ///
    /// Returns Err([`AllocError`]) instead of panicking if the backing
    /// allocator fails.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_slice_copy_rc<T: Copy>(&self, slice: impl AsRef<[T]>) -> Result<Rc<[T], A>, AllocError> {
        let slice = slice.as_ref();
        let ptr = self.try_alloc_slice_local_copy::<_, false>(slice, AllocFlavor::Rc)?;
        // SAFETY: helper initialized the slice and bumped the refcount for this Rc.
        Ok(Rc::from_owned_in_chunk(unsafe {
            crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr)
        }))
    }

    /// Clone every element of `slice` into the arena, returning an [`Rc`].
    ///
    /// The returned smart pointer is immutable.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_clone_rc`] for a fallible variant.
    #[inline]
    pub fn alloc_slice_clone_rc<T: Clone>(&self, slice: impl AsRef<[T]>) -> Rc<[T], A> {
        let ptr = expect_alloc(self.try_alloc_slice_local_clone_inner::<_, true>(slice.as_ref(), AllocFlavor::Rc));
        // SAFETY: helper initialized the slice and bumped the refcount for this Rc.
        Rc::from_owned_in_chunk(unsafe { crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr) })
    }

    /// Fallible variant of [`Self::alloc_slice_clone_rc`].
    ///
    /// Returns Err([`AllocError`]) instead of panicking if the backing
    /// allocator cannot satisfy the request.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// Cannot panic from the iterator length mismatch — `slice.len()`
    /// matches the iterator's length by construction. May panic if a
    /// `T::clone()` impl panics; in that case already-initialized
    /// elements are dropped via the slice init guard before the panic
    /// propagates.
    #[inline]
    pub fn try_alloc_slice_clone_rc<T: Clone>(&self, slice: impl AsRef<[T]>) -> Result<Rc<[T], A>, AllocError> {
        let ptr = self.try_alloc_slice_local_clone_inner::<_, false>(slice.as_ref(), AllocFlavor::Rc)?;
        // SAFETY: helper initialized the slice and bumped the refcount for this Rc.
        Ok(Rc::from_owned_in_chunk(unsafe {
            crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr)
        }))
    }

    /// Allocate a slice of `len` elements, with element `i` produced by
    /// `f(i)`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_with_rc`] for a fallible variant.
    ///
    /// If `f` panics, already-initialized elements are dropped (drop guard) and the
    /// panic propagates.
    #[inline]
    pub fn alloc_slice_fill_with_rc<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Rc<[T], A> {
        let ptr = expect_alloc(self.try_alloc_slice_local_fill_with_inner::<_, _, true>(len, AllocFlavor::Rc, f));
        // SAFETY: helper initialized the slice and bumped the refcount for this Rc.
        Rc::from_owned_in_chunk(unsafe { crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr) })
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with_rc`].
    ///
    /// Returns Err([`AllocError`]) instead of panicking if the backing
    /// allocator fails. If `f` panics, already-initialized elements are
    /// dropped and the panic propagates.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_slice_fill_with_rc<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<Rc<[T], A>, AllocError> {
        let ptr = self.try_alloc_slice_local_fill_with_inner::<_, _, false>(len, AllocFlavor::Rc, f)?;
        // SAFETY: helper initialized the slice and bumped the refcount for this Rc.
        Ok(Rc::from_owned_in_chunk(unsafe {
            crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr)
        }))
    }

    /// Allocate a slice and fill it with values pulled from `iter`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_iter_rc`] for a fallible variant.
    ///
    /// May also panic if the iterator yields fewer elements than its
    /// `ExactSizeIterator::len()` reported.
    #[inline]
    pub fn alloc_slice_fill_iter_rc<T, I>(&self, iter: I) -> Rc<[T], A>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let ptr = expect_alloc(self.try_alloc_slice_local_fill_iter_inner::<_, _, true>(iter, AllocFlavor::Rc));
        // SAFETY: helper initialized the slice and bumped the refcount for this Rc.
        Rc::from_owned_in_chunk(unsafe { crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr) })
    }

    /// Fallible variant of [`Self::alloc_slice_fill_iter_rc`].
    ///
    /// Returns Err([`AllocError`]) instead of panicking if the backing
    /// allocator fails.
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
    pub fn try_alloc_slice_fill_iter_rc<T, I>(&self, iter: I) -> Result<Rc<[T], A>, AllocError>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let ptr = self.try_alloc_slice_local_fill_iter_inner::<_, _, false>(iter, AllocFlavor::Rc)?;
        // SAFETY: helper initialized the slice and bumped the refcount for this Rc.
        Ok(Rc::from_owned_in_chunk(unsafe {
            crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr)
        }))
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate `len` slots and fill each via `f(i)`, returning a
    /// [`Pin<Rc<[T], A>>`](core::pin::Pin).
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_slice_fill_with_rc`].
    #[must_use]
    #[inline]
    pub fn alloc_slice_fill_with_rc_pin<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> core::pin::Pin<Rc<[T], A>>
    where
        A: 'static,
    {
        crate::rc::Rc::into_pin(self.alloc_slice_fill_with_rc(len, f))
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with_rc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_slice_fill_with_rc`].
    #[inline]
    pub fn try_alloc_slice_fill_with_rc_pin<T, F: FnMut(usize) -> T>(
        &self,
        len: usize,
        f: F,
    ) -> Result<core::pin::Pin<Rc<[T], A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_slice_fill_with_rc(len, f).map(crate::rc::Rc::into_pin)
    }
}
