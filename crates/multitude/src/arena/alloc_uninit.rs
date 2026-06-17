// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uninitialized / zeroed allocation API on [`Arena`].
//!
//! All public methods are documented on [`Arena`] itself; this file
//! groups the `alloc_uninit_*` / `alloc_zeroed_*` family together to
//! keep the central `mod.rs` smaller.

use core::mem::MaybeUninit;
use core::pin::Pin;

use allocator_api2::alloc::{AllocError, Allocator};

use super::Arena;
use crate::arc::Arc;
use crate::r#box::Box;

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate uninitialized space for a `T`, returning `&mut MaybeUninit<T>`.
    ///
    /// The arena-lifetime analog of [`Self::alloc_uninit_box`]: write the value
    /// (e.g. via [`MaybeUninit::write`]) then read it back with
    /// [`MaybeUninit::assume_init_mut`].
    ///
    /// Unlike [`Self::alloc`], the value's destructor is **never** run — the
    /// slot holds `MaybeUninit<T>`, which has no drop glue, and there is no way
    /// to register one after the fact for a bare reference. If you need
    /// drop-at-teardown semantics use [`Self::alloc`] / [`Self::alloc_with`],
    /// or freeze into a [`Box`](crate::Box) / [`Arc`](crate::Arc).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit`] for a fallible variant.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn alloc_uninit<T: Send>(&self) -> &mut MaybeUninit<T> {
        self.alloc_with::<MaybeUninit<T>, _>(MaybeUninit::uninit)
    }

    /// Fallible variant of [`Self::alloc_uninit`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn try_alloc_uninit<T: Send>(&self) -> Result<&mut MaybeUninit<T>, AllocError> {
        self.try_alloc_with::<MaybeUninit<T>, _>(MaybeUninit::uninit)
    }

    /// Like [`Self::alloc_uninit`] but the value bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed`] for a fallible variant.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn alloc_zeroed<T: Send>(&self) -> &mut MaybeUninit<T> {
        self.alloc_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)
    }

    /// Fallible variant of [`Self::alloc_zeroed`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn try_alloc_zeroed<T: Send>(&self) -> Result<&mut MaybeUninit<T>, AllocError> {
        self.try_alloc_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)
    }

    /// Allocate an uninitialized `[MaybeUninit<T>]` of length `len`.
    ///
    /// The arena-lifetime analog of [`Self::alloc_uninit_slice_box`]. Each
    /// element's destructor is never run (see [`Self::alloc_uninit`]).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_slice`] for a fallible variant.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn alloc_uninit_slice<T: Send>(&self, len: usize) -> &mut [MaybeUninit<T>] {
        self.alloc_slice_fill_with::<MaybeUninit<T>, _>(len, |_| MaybeUninit::uninit())
    }

    /// Fallible variant of [`Self::alloc_uninit_slice`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn try_alloc_uninit_slice<T: Send>(&self, len: usize) -> Result<&mut [MaybeUninit<T>], AllocError> {
        self.try_alloc_slice_fill_with::<MaybeUninit<T>, _>(len, |_| MaybeUninit::uninit())
    }

    /// Like [`Self::alloc_uninit_slice`] but the slice bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed_slice`] for a fallible variant.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn alloc_zeroed_slice<T: Send>(&self, len: usize) -> &mut [MaybeUninit<T>] {
        self.alloc_slice_fill_with::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
    }

    /// Fallible variant of [`Self::alloc_zeroed_slice`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn try_alloc_zeroed_slice<T: Send>(&self, len: usize) -> Result<&mut [MaybeUninit<T>], AllocError> {
        self.try_alloc_slice_fill_with::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
    }
}

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
    /// No drop entry is reserved. Dropping `Arc<MaybeUninit<T>>` without
    /// `assume_init` is sound (`MaybeUninit<T>` has no drop glue); after
    /// `assume_init`, dropping the last `Arc<T>` runs `T::drop` eagerly
    /// via `drop_in_place::<T>` (see [`Arc`](crate::Arc)'s per-pointer
    /// reference counting).
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
        self.alloc_arc_with::<MaybeUninit<T>, _>(MaybeUninit::uninit)
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
        self.try_alloc_arc_with::<MaybeUninit<T>, _>(MaybeUninit::uninit)
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
        self.alloc_arc_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)
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
        self.try_alloc_arc_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)
    }

    /// Allocate `len` uninitialized `T` slots and return an
    /// [`Arc<[MaybeUninit<T>], A>`](crate::Arc).
    ///
    /// No drop entry is reserved. Dropping `Arc<[MaybeUninit<T>]>`
    /// without `assume_init` is sound (`MaybeUninit<T>` has no drop
    /// glue); after `assume_init`, dropping the last `Arc<[T]>` runs the
    /// element destructors eagerly via `drop_in_place::<[T]>` (see
    /// [`Arc`](crate::Arc)'s per-pointer reference counting).
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
        self.alloc_slice_fill_with_arc::<MaybeUninit<T>, _>(len, |_| MaybeUninit::uninit())
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
        self.try_alloc_slice_fill_with_arc::<MaybeUninit<T>, _>(len, |_| MaybeUninit::uninit())
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
        self.alloc_slice_fill_with_arc::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
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
        self.try_alloc_slice_fill_with_arc::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
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
