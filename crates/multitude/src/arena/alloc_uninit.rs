// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uninitialized / zeroed allocation API on [`Arena`].
//!
//! All public methods are documented on [`Arena`] itself; this file
//! groups the `alloc_uninit_*` / `alloc_zeroed_*` family together to
//! keep the central `mod.rs` smaller.

use core::mem;
use core::mem::MaybeUninit;
use core::pin::Pin;

use allocator_api2::alloc::{AllocError, Allocator};

use super::{Arena, ExpectAlloc};
use crate::arc::Arc;
use crate::r#box::Box;

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate uninitialized space for a `T` and return an
    /// [`Box<MaybeUninit<T>, A>`](crate::Box). The caller must
    /// initialize the value (e.g., via [`MaybeUninit::write`]) before
    /// calling [`Box::<MaybeUninit<T>, A>::assume_init`].
    ///
    /// No drop entry is reserved for this box allocation. Dropping
    /// `Box<MaybeUninit<T>>` without `assume_init` is sound and does not
    /// run `T::drop`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_box`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_box<T>(&self) -> Box<MaybeUninit<T>, A> {
        self.alloc_box_with::<MaybeUninit<T>, _>(MaybeUninit::uninit)
    }

    /// Fallible variant of [`Self::alloc_uninit_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_uninit_box<T>(&self) -> Result<Box<MaybeUninit<T>, A>, AllocError> {
        self.try_alloc_box_with::<MaybeUninit<T>, _>(MaybeUninit::uninit)
    }

    /// Like [`Self::alloc_uninit_box`] but the value bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed_box`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_box<T>(&self) -> Box<MaybeUninit<T>, A> {
        self.alloc_box_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)
    }

    /// Fallible variant of [`Self::alloc_zeroed_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_zeroed_box<T>(&self) -> Result<Box<MaybeUninit<T>, A>, AllocError> {
        self.try_alloc_box_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)
    }

    /// Allocate uninitialized space for a `T` and return an
    /// [`Arc<MaybeUninit<T>, A>`](crate::Arc).
    ///
    /// For `T: Drop`, this reserves a placeholder drop entry. Dropping
    /// `Arc<MaybeUninit<T>>` without `assume_init` is sound; `assume_init`
    /// commits the entry so a later `Arc<T>` drop runs `T::drop`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_arc`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_arc<T>(&self) -> Arc<MaybeUninit<T>, A>
    where
        A: Send + Sync,
        T: Send + Sync,
    {
        if const { mem::needs_drop::<T>() } {
            (self.impl_alloc_uninit_arc::<T>(false)).expect_alloc()
        } else {
            self.alloc_arc_with::<MaybeUninit<T>, _>(MaybeUninit::uninit)
        }
    }

    /// Fallible variant of [`Self::alloc_uninit_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_uninit_arc<T>(&self) -> Result<Arc<MaybeUninit<T>, A>, AllocError>
    where
        A: Send + Sync,
        T: Send + Sync,
    {
        if const { mem::needs_drop::<T>() } {
            self.impl_alloc_uninit_arc::<T>(false)
        } else {
            self.try_alloc_arc_with::<MaybeUninit<T>, _>(MaybeUninit::uninit)
        }
    }

    /// Like [`Self::alloc_uninit_arc`] but the value bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed_arc`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_arc<T>(&self) -> Arc<MaybeUninit<T>, A>
    where
        A: Send + Sync,
        T: Send + Sync,
    {
        if const { mem::needs_drop::<T>() } {
            (self.impl_alloc_uninit_arc::<T>(true)).expect_alloc()
        } else {
            self.alloc_arc_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)
        }
    }

    /// Fallible variant of [`Self::alloc_zeroed_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_zeroed_arc<T>(&self) -> Result<Arc<MaybeUninit<T>, A>, AllocError>
    where
        A: Send + Sync,
        T: Send + Sync,
    {
        if const { mem::needs_drop::<T>() } {
            self.impl_alloc_uninit_arc::<T>(true)
        } else {
            self.try_alloc_arc_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)
        }
    }

    /// Allocate `len` uninitialized `T` slots and return an
    /// [`Arc<[MaybeUninit<T>], A>`](crate::Arc).
    ///
    /// For `T: Drop`, this reserves a placeholder slice drop entry.
    /// Dropping `Arc<[MaybeUninit<T>]>` without `assume_init` is sound;
    /// `assume_init` commits the entry so dropping `Arc<[T]>` runs element
    /// destructors.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_slice_arc`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_slice_arc<T>(&self, len: usize) -> Arc<[MaybeUninit<T>], A>
    where
        A: Send + Sync,
        T: Send + Sync,
    {
        if const { mem::needs_drop::<T>() } {
            (self.impl_alloc_uninit_slice_arc::<T>(len, false)).expect_alloc()
        } else {
            self.alloc_slice_fill_with_arc::<MaybeUninit<T>, _>(len, |_| MaybeUninit::uninit())
        }
    }

    /// Fallible variant of [`Self::alloc_uninit_slice_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_uninit_slice_arc<T>(&self, len: usize) -> Result<Arc<[MaybeUninit<T>], A>, AllocError>
    where
        A: Send + Sync,
        T: Send + Sync,
    {
        if const { mem::needs_drop::<T>() } {
            self.impl_alloc_uninit_slice_arc::<T>(len, false)
        } else {
            self.try_alloc_slice_fill_with_arc::<MaybeUninit<T>, _>(len, |_| MaybeUninit::uninit())
        }
    }

    /// Like [`Self::alloc_uninit_slice_arc`] but the slice bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed_slice_arc`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_slice_arc<T>(&self, len: usize) -> Arc<[MaybeUninit<T>], A>
    where
        A: Send + Sync,
        T: Send + Sync,
    {
        if const { mem::needs_drop::<T>() } {
            (self.impl_alloc_uninit_slice_arc::<T>(len, true)).expect_alloc()
        } else {
            self.alloc_slice_fill_with_arc::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
        }
    }

    /// Fallible variant of [`Self::alloc_zeroed_slice_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_zeroed_slice_arc<T>(&self, len: usize) -> Result<Arc<[MaybeUninit<T>], A>, AllocError>
    where
        A: Send + Sync,
        T: Send + Sync,
    {
        if const { mem::needs_drop::<T>() } {
            self.impl_alloc_uninit_slice_arc::<T>(len, true)
        } else {
            self.try_alloc_slice_fill_with_arc::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
        }
    }

    /// Allocate `len` uninitialized `T` slots and return an
    /// [`Box<[MaybeUninit<T>], A>`](crate::Box).
    ///
    /// No drop entry is reserved for this box allocation. Dropping
    /// `Box<[MaybeUninit<T>]>` without `assume_init` is sound and does not
    /// run any element destructors.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_slice_box`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_slice_box<T>(&self, len: usize) -> Box<[MaybeUninit<T>], A> {
        self.alloc_slice_fill_with_box::<MaybeUninit<T>, _>(len, |_| MaybeUninit::uninit())
    }

    /// Fallible variant of [`Self::alloc_uninit_slice_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_uninit_slice_box<T>(&self, len: usize) -> Result<Box<[MaybeUninit<T>], A>, AllocError> {
        self.try_alloc_slice_fill_with_box::<MaybeUninit<T>, _>(len, |_| MaybeUninit::uninit())
    }

    /// Like [`Self::alloc_uninit_slice_box`] but the slice bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed_slice_box`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_slice_box<T>(&self, len: usize) -> Box<[MaybeUninit<T>], A> {
        self.alloc_slice_fill_with_box::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
    }

    /// Fallible variant of [`Self::alloc_zeroed_slice_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_zeroed_slice_box<T>(&self, len: usize) -> Result<Box<[MaybeUninit<T>], A>, AllocError> {
        self.try_alloc_slice_fill_with_box::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate an uninitialized `MaybeUninit<T>` slot and return a
    /// [`Pin<Box<MaybeUninit<T>, A>>`](core::pin::Pin). Pair with
    /// [`Box::assume_init_pin`](crate::Box::assume_init_pin) once
    /// initialization completes to obtain `Pin<Box<T, A>>` without
    /// ever moving the value off-arena.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()`
    /// is at least 32 KiB.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_box_pin<T>(&self) -> Pin<Box<MaybeUninit<T>, A>>
    where
        A: 'static,
    {
        Box::into_pin(self.alloc_uninit_box::<T>())
    }

    /// Fallible variant of [`Self::alloc_uninit_box_pin`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    #[inline]
    pub fn try_alloc_uninit_box_pin<T>(&self) -> Result<Pin<Box<MaybeUninit<T>, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_uninit_box::<T>().map(Box::into_pin)
    }

    /// Zeroed pinned uninit variant of [`Self::alloc_uninit_box_pin`].
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_box_pin<T>(&self) -> Pin<Box<MaybeUninit<T>, A>>
    where
        A: 'static,
    {
        Box::into_pin(self.alloc_zeroed_box::<T>())
    }

    /// Fallible variant of [`Self::alloc_zeroed_box_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[inline]
    pub fn try_alloc_zeroed_box_pin<T>(&self) -> Result<Pin<Box<MaybeUninit<T>, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_zeroed_box::<T>().map(Box::into_pin)
    }

    /// `Arc` mirror of [`Self::alloc_uninit_box_pin`].
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[must_use]
    #[inline]
    pub fn alloc_uninit_arc_pin<T>(&self) -> Pin<Arc<MaybeUninit<T>, A>>
    where
        A: Send + Sync + 'static,
        T: Send + Sync,
    {
        Arc::into_pin(self.alloc_uninit_arc::<T>())
    }

    /// Fallible variant of [`Self::alloc_uninit_arc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[inline]
    pub fn try_alloc_uninit_arc_pin<T>(&self) -> Result<Pin<Arc<MaybeUninit<T>, A>>, AllocError>
    where
        A: Send + Sync + 'static,
        T: Send + Sync,
    {
        self.try_alloc_uninit_arc::<T>().map(Arc::into_pin)
    }

    /// `Arc` mirror of [`Self::alloc_zeroed_box_pin`].
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_arc_pin<T>(&self) -> Pin<Arc<MaybeUninit<T>, A>>
    where
        A: Send + Sync + 'static,
        T: Send + Sync,
    {
        Arc::into_pin(self.alloc_zeroed_arc::<T>())
    }

    /// Fallible variant of [`Self::alloc_zeroed_arc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[inline]
    pub fn try_alloc_zeroed_arc_pin<T>(&self) -> Result<Pin<Arc<MaybeUninit<T>, A>>, AllocError>
    where
        A: Send + Sync + 'static,
        T: Send + Sync,
    {
        self.try_alloc_zeroed_arc::<T>().map(Arc::into_pin)
    }
}
