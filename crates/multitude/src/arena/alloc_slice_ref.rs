// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `&mut [T]` slice allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself.

use allocator_api2::alloc::{AllocError, Allocator};

use super::{AllocFlavor, Arena, expect_alloc};

impl<A: Allocator + Clone> Arena<A> {
    /// Bump-allocate a copy of `slice` (element-by-element `Copy`) into the arena.
    ///
    /// Returns a mutable slice whose lifetime is tied to `&self`. Like
    /// [`Self::alloc`] but for slices of `T: Copy`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_copy`] for a fallible variant.
    #[must_use]
    #[expect(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn alloc_slice_copy<T: Copy>(&self, slice: impl AsRef<[T]>) -> &mut [T] {
        let slice = slice.as_ref();
        // Use the panicking-inner variant rather than `expect_alloc(try_alloc_slice_copy(...))`
        // so the inlined hot path does not carry a dead `Result<NonNull<_>, _>` niche check.
        let ptr = self.alloc_slice_local_copy_or_panic(slice);
        // SAFETY: helper initialized the full slice and pinned the chunk via `pinned_local`/`current_local_pinned`, so the `&mut` reborrow bounded by `&self` is valid for the arena's lifetime.
        unsafe { &mut *ptr.as_ptr() }
    }

    /// Fallible variant of [`Self::alloc_slice_copy`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[expect(clippy::mut_from_ref, reason = "simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn try_alloc_slice_copy<T: Copy>(&self, slice: impl AsRef<[T]>) -> Result<&mut [T], AllocError> {
        let slice = slice.as_ref();
        let ptr = self.try_alloc_slice_local_copy::<_, false>(slice, AllocFlavor::SimpleRef)?;
        // SAFETY: helper initialized the full slice and pinned the chunk via `pinned_local`/`current_local_pinned`, so the `&mut` reborrow bounded by `&self` is valid for the arena's lifetime.
        Ok(unsafe { &mut *ptr.as_ptr() })
    }

    /// Bump-allocate a slice and fill it with values pulled from `f`.
    ///
    /// Returns a mutable slice whose lifetime is tied to `&self`. If
    /// `T: Drop`, a drop entry is registered (drops at arena drop).
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_with`] for a fallible variant.
    ///
    /// If `f` panics, already-initialized elements are dropped (drop guard) and the
    /// panic propagates.
    #[must_use]
    #[inline]
    pub fn alloc_slice_fill_with<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> &mut [T] {
        expect_alloc(self.try_alloc_slice_fill_with(len, f))
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// If `f` panics, already-initialized elements are dropped.
    #[expect(clippy::mut_from_ref, reason = "simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn try_alloc_slice_fill_with<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<&mut [T], AllocError> {
        let ptr = self.try_alloc_slice_local_fill_with_inner::<_, _, false>(len, AllocFlavor::SimpleRef, f)?;
        // SAFETY: helper initialized the full slice and pinned the chunk via `pinned_local`/`current_local_pinned`, so the `&mut` reborrow bounded by `&self` is valid for the arena's lifetime.
        Ok(unsafe { &mut *ptr.as_ptr() })
    }

    /// Bump-allocate a slice by cloning each element of `slice` into the arena.
    ///
    /// Returns a mutable slice whose lifetime is tied to `&self`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_clone`] for a fallible variant.
    ///
    /// May panic if `T::clone` panics; already-cloned elements are dropped before the
    /// panic propagates.
    #[must_use]
    #[inline]
    pub fn alloc_slice_clone<T: Clone>(&self, slice: impl AsRef<[T]>) -> &mut [T] {
        expect_alloc(self.try_alloc_slice_clone(slice))
    }

    /// Fallible variant of [`Self::alloc_slice_clone`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// May panic if a `T::clone` impl panics; already-cloned elements
    /// are dropped before the panic propagates.
    #[expect(clippy::mut_from_ref, reason = "simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn try_alloc_slice_clone<T: Clone>(&self, slice: impl AsRef<[T]>) -> Result<&mut [T], AllocError> {
        let ptr = self.try_alloc_slice_local_clone_inner::<_, false>(slice.as_ref(), AllocFlavor::SimpleRef)?;
        // SAFETY: helper initialized the full slice and pinned the chunk via `pinned_local`/`current_local_pinned`, so the `&mut` reborrow bounded by `&self` is valid for the arena's lifetime.
        Ok(unsafe { &mut *ptr.as_ptr() })
    }

    /// Bump-allocate a slice and fill it with values pulled from `iter`.
    ///
    /// Returns a mutable slice whose lifetime is tied to `&self`. If
    /// `T: Drop`, a drop entry is registered (drops at arena drop).
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_iter`] for a fallible variant.
    ///
    /// May also panic if the iterator yields fewer elements than its
    /// `ExactSizeIterator::len()` reported.
    #[must_use]
    #[inline]
    pub fn alloc_slice_fill_iter<T, I>(&self, iter: I) -> &mut [T]
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        expect_alloc(self.try_alloc_slice_fill_iter(iter))
    }

    /// Fallible variant of [`Self::alloc_slice_fill_iter`].
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
    #[expect(clippy::mut_from_ref, reason = "see `try_alloc_with`")]
    pub fn try_alloc_slice_fill_iter<T, I>(&self, iter: I) -> Result<&mut [T], AllocError>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let ptr = self.try_alloc_slice_local_fill_iter_inner::<_, _, false>(iter, AllocFlavor::SimpleRef)?;
        // SAFETY: helper initialized the full slice and pinned the chunk via `pinned_local`/`current_local_pinned`, so the `&mut` reborrow bounded by `&self` is valid for the arena's lifetime.
        Ok(unsafe { &mut *ptr.as_ptr() })
    }
}
