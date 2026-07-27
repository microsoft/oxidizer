// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uninitialized / zeroed allocation API on [`Arena`].
//!
//! All public methods are documented on [`Arena`] itself; this file
//! groups the `alloc_uninit_*` / `alloc_zeroed_*` family together to
//! keep the central `mod.rs` smaller.

use core::mem::MaybeUninit;
use core::pin::Pin;

use allocator_api2::alloc::Allocator;

use super::Arena;
use crate::arc::Arc;
use crate::r#box::Box;
use crate::rc::Rc;
use crate::{Alloc, AllocError};

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate uninitialized space for a `T`, returning `Alloc<MaybeUninit<T>>`.
    ///
    /// The arena-lifetime analog of [`Self::alloc_uninit_box`]: write the value
    /// (e.g. via [`MaybeUninit::write`]) then read it back with
    /// [`MaybeUninit::assume_init_mut`].
    ///
    /// The slot holds `MaybeUninit<T>`, which has no drop glue, so the inner
    /// value's destructor is **never** run, even when the [`Alloc`] is dropped.
    /// If you need drop-on-drop semantics use [`Self::alloc`] / [`Self::alloc_with`],
    /// or freeze into a [`Box`](crate::Box) / [`Arc`](crate::Arc) / [`Rc`](crate::Rc).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut value = arena.alloc_uninit::<u32>();
    /// value.write(42);
    /// assert_eq!(unsafe { value.assume_init_ref() }, &42);
    /// ```
    #[inline]
    pub fn alloc_uninit<T>(&self) -> Alloc<'_, MaybeUninit<T>> {
        self.alloc_with::<MaybeUninit<T>, _>(MaybeUninit::uninit)
    }

    /// Fallible variant of [`Self::alloc_uninit`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(mut value) = arena.try_alloc_uninit::<u32>() else {
    ///     panic!("allocation failed");
    /// };
    /// value.write(42);
    /// assert_eq!(unsafe { value.assume_init_ref() }, &42);
    /// ```
    #[inline]
    pub fn try_alloc_uninit<T>(&self) -> Result<Alloc<'_, MaybeUninit<T>>, AllocError> {
        self.try_alloc_with::<MaybeUninit<T>, _>(MaybeUninit::uninit)
    }

    /// Like [`Self::alloc_uninit`] but the value bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_zeroed::<u32>();
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// assert_eq!(unsafe { *(&*value).as_ptr() }, 0);
    /// ```
    #[inline]
    pub fn alloc_zeroed<T>(&self) -> Alloc<'_, MaybeUninit<T>> {
        self.alloc_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)
    }

    /// Fallible variant of [`Self::alloc_zeroed`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_zeroed::<u32>() else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// assert_eq!(unsafe { *(&*value).as_ptr() }, 0);
    /// ```
    #[inline]
    pub fn try_alloc_zeroed<T>(&self) -> Result<Alloc<'_, MaybeUninit<T>>, AllocError> {
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_uninit_slice::<u32>(2);
    /// assert_eq!(value.len(), 2);
    /// ```
    #[inline]
    pub fn alloc_uninit_slice<T>(&self, len: usize) -> Alloc<'_, [MaybeUninit<T>]> {
        self.alloc_slice_fill_with::<MaybeUninit<T>, _>(len, |_| MaybeUninit::uninit())
    }

    /// Fallible variant of [`Self::alloc_uninit_slice`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_uninit_slice::<u32>(2) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(value.len(), 2);
    /// ```
    #[inline]
    pub fn try_alloc_uninit_slice<T>(&self, len: usize) -> Result<Alloc<'_, [MaybeUninit<T>]>, AllocError> {
        self.try_alloc_slice_fill_with::<MaybeUninit<T>, _>(len, |_| MaybeUninit::uninit())
    }

    /// Like [`Self::alloc_uninit_slice`] but the slice bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed_slice`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_zeroed_slice::<u32>(2);
    /// assert_eq!(value.len(), 2);
    /// assert_eq!(unsafe { *value[0].as_ptr() }, 0);
    /// ```
    #[inline]
    pub fn alloc_zeroed_slice<T>(&self, len: usize) -> Alloc<'_, [MaybeUninit<T>]> {
        self.alloc_slice_fill_with::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
    }

    /// Fallible variant of [`Self::alloc_zeroed_slice`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_zeroed_slice::<u32>(2) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(value.len(), 2);
    /// assert_eq!(unsafe { *value[0].as_ptr() }, 0);
    /// ```
    #[inline]
    pub fn try_alloc_zeroed_slice<T>(&self, len: usize) -> Result<Alloc<'_, [MaybeUninit<T>]>, AllocError> {
        self.try_alloc_slice_fill_with::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate uninitialized `T` storage in an arena-backed [`Box`](crate::Box). The caller must
    /// initialize the value (e.g., via [`MaybeUninit::write`]) before
    /// calling [`Box::<MaybeUninit<T>, A>::assume_init`].
    ///
    /// Dropping `Box<MaybeUninit<T>>` without `assume_init` is sound and does
    /// not run `T::drop` (`MaybeUninit<T>` has no drop glue).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_box`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_uninit_box::<u32>();
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_uninit_box::<u32>() else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_zeroed_box::<u32>();
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// assert_eq!(unsafe { *(&*value).as_ptr() }, 0);
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_zeroed_box::<u32>() else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// assert_eq!(unsafe { *(&*value).as_ptr() }, 0);
    /// ```
    #[inline]
    pub fn try_alloc_zeroed_box<T>(&self) -> Result<Box<MaybeUninit<T>, A>, AllocError> {
        self.try_alloc_box_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)
    }

    /// Allocate uninitialized `T` storage in an arena-backed [`Arc`](crate::Arc).
    ///
    /// Dropping `Arc<MaybeUninit<T>>` without `assume_init` is sound
    /// (`MaybeUninit<T>` has no drop glue); after
    /// `assume_init`, dropping the last `Arc<T>` runs `T::drop` eagerly
    /// via `drop_in_place::<T>` (see [`Arc`](crate::Arc)'s per-pointer
    /// reference counting).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_arc`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_uninit_arc::<u32>();
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_uninit_arc::<u32>() else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_zeroed_arc::<u32>();
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// assert_eq!(unsafe { *(&*value).as_ptr() }, 0);
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_zeroed_arc::<u32>() else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// assert_eq!(unsafe { *(&*value).as_ptr() }, 0);
    /// ```
    #[inline]
    pub fn try_alloc_zeroed_arc<T>(&self) -> Result<Arc<MaybeUninit<T>, A>, AllocError>
    where
        A: Send + Sync,
        T: Send + Sync,
    {
        self.try_alloc_arc_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)
    }

    /// Allocate `len` uninitialized `T` slots in an arena-backed [`Arc`](crate::Arc).
    ///
    /// Dropping `Arc<[MaybeUninit<T>]>`
    /// without `assume_init` is sound (`MaybeUninit<T>` has no drop
    /// glue); after `assume_init`, dropping the last `Arc<[T]>` runs the
    /// element destructors eagerly via `drop_in_place::<[T]>` (see
    /// [`Arc`](crate::Arc)'s per-pointer reference counting).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_slice_arc`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_uninit_slice_arc::<u32>(2);
    /// assert_eq!(value.len(), 2);
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_uninit_slice_arc::<u32>(2) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(value.len(), 2);
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_zeroed_slice_arc::<u32>(2);
    /// assert_eq!(value.len(), 2);
    /// assert_eq!(unsafe { *value[0].as_ptr() }, 0);
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_zeroed_slice_arc::<u32>(2) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(value.len(), 2);
    /// assert_eq!(unsafe { *value[0].as_ptr() }, 0);
    /// ```
    #[inline]
    pub fn try_alloc_zeroed_slice_arc<T>(&self, len: usize) -> Result<Arc<[MaybeUninit<T>], A>, AllocError>
    where
        A: Send + Sync,
        T: Send + Sync,
    {
        self.try_alloc_slice_fill_with_arc::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
    }

    // ===== `Rc<MaybeUninit<T>>` / `Rc<[MaybeUninit<T>]>` mirror =====

    /// Allocate uninitialized `T` storage in an arena-backed [`Rc`](crate::Rc). See [`Self::alloc_uninit_arc`].
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()` is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_uninit_rc::<u32>();
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_uninit_rc<T>(&self) -> Rc<MaybeUninit<T>, A> {
        self.alloc_rc_with::<MaybeUninit<T>, _>(MaybeUninit::uninit)
    }

    /// Fallible variant of [`Self::alloc_uninit_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_uninit_rc::<u32>() else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// ```
    #[inline]
    pub fn try_alloc_uninit_rc<T>(&self) -> Result<Rc<MaybeUninit<T>, A>, AllocError> {
        self.try_alloc_rc_with::<MaybeUninit<T>, _>(MaybeUninit::uninit)
    }

    /// Like [`Self::alloc_uninit_rc`] but the value bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()` is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_zeroed_rc::<u32>();
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// assert_eq!(unsafe { *(&*value).as_ptr() }, 0);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_rc<T>(&self) -> Rc<MaybeUninit<T>, A> {
        self.alloc_rc_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)
    }

    /// Fallible variant of [`Self::alloc_zeroed_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_zeroed_rc::<u32>() else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(core::mem::size_of_val(&*value), core::mem::size_of::<u32>());
    /// assert_eq!(unsafe { *(&*value).as_ptr() }, 0);
    /// ```
    #[inline]
    pub fn try_alloc_zeroed_rc<T>(&self) -> Result<Rc<MaybeUninit<T>, A>, AllocError> {
        self.try_alloc_rc_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)
    }

    /// Allocate `len` uninitialized `T` slots in an arena-backed [`Rc`](crate::Rc). See [`Self::alloc_uninit_slice_arc`].
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()` is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_uninit_slice_rc::<u32>(2);
    /// assert_eq!(value.len(), 2);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_uninit_slice_rc<T>(&self, len: usize) -> Rc<[MaybeUninit<T>], A> {
        self.alloc_slice_fill_with_rc::<MaybeUninit<T>, _>(len, |_| MaybeUninit::uninit())
    }

    /// Fallible variant of [`Self::alloc_uninit_slice_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_uninit_slice_rc::<u32>(2) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(value.len(), 2);
    /// ```
    #[inline]
    pub fn try_alloc_uninit_slice_rc<T>(&self, len: usize) -> Result<Rc<[MaybeUninit<T>], A>, AllocError> {
        self.try_alloc_slice_fill_with_rc::<MaybeUninit<T>, _>(len, |_| MaybeUninit::uninit())
    }

    /// Like [`Self::alloc_uninit_slice_rc`] but the slice bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()` is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_zeroed_slice_rc::<u32>(2);
    /// assert_eq!(value.len(), 2);
    /// assert_eq!(unsafe { *value[0].as_ptr() }, 0);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_slice_rc<T>(&self, len: usize) -> Rc<[MaybeUninit<T>], A> {
        self.alloc_slice_fill_with_rc::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
    }

    /// Fallible variant of [`Self::alloc_zeroed_slice_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_zeroed_slice_rc::<u32>(2) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(value.len(), 2);
    /// assert_eq!(unsafe { *value[0].as_ptr() }, 0);
    /// ```
    #[inline]
    pub fn try_alloc_zeroed_slice_rc<T>(&self, len: usize) -> Result<Rc<[MaybeUninit<T>], A>, AllocError> {
        self.try_alloc_slice_fill_with_rc::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
    }

    /// Allocate `len` uninitialized `T` slots in an arena-backed [`Box`](crate::Box).
    ///
    /// Dropping `Box<[MaybeUninit<T>]>` without `assume_init` is sound and does
    /// not run any element destructors (`MaybeUninit<T>` has no drop glue).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_slice_box`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_uninit_slice_box::<u32>(2);
    /// assert_eq!(value.len(), 2);
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_uninit_slice_box::<u32>(2) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(value.len(), 2);
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_zeroed_slice_box::<u32>(2);
    /// assert_eq!(value.len(), 2);
    /// assert_eq!(unsafe { *value[0].as_ptr() }, 0);
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_zeroed_slice_box::<u32>(2) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(value.len(), 2);
    /// assert_eq!(unsafe { *value[0].as_ptr() }, 0);
    /// ```
    #[inline]
    pub fn try_alloc_zeroed_slice_box<T>(&self, len: usize) -> Result<Box<[MaybeUninit<T>], A>, AllocError> {
        self.try_alloc_slice_fill_with_box::<MaybeUninit<T>, _>(len, |_| MaybeUninit::zeroed())
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate uninitialized `T` storage in a pinned [`Box`](crate::Box). Pair with
    /// [`Box::assume_init_pin`](crate::Box::assume_init_pin) once
    /// initialization completes to obtain `Pin<Box<T, A>>` without
    /// ever moving the value off-arena.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()`
    /// is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_uninit_box_pin::<u32>();
    /// assert_eq!(
    ///     core::mem::size_of_val(value.as_ref().get_ref()),
    ///     core::mem::size_of::<u32>()
    /// );
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_uninit_box_pin::<u32>() else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(
    ///     core::mem::size_of_val(value.as_ref().get_ref()),
    ///     core::mem::size_of::<u32>()
    /// );
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_zeroed_box_pin::<u32>();
    /// assert_eq!(
    ///     core::mem::size_of_val(value.as_ref().get_ref()),
    ///     core::mem::size_of::<u32>()
    /// );
    /// assert_eq!(unsafe { *value.as_ref().get_ref().as_ptr() }, 0);
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_zeroed_box_pin::<u32>() else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(
    ///     core::mem::size_of_val(value.as_ref().get_ref()),
    ///     core::mem::size_of::<u32>()
    /// );
    /// assert_eq!(unsafe { *value.as_ref().get_ref().as_ptr() }, 0);
    /// ```
    #[inline]
    pub fn try_alloc_zeroed_box_pin<T>(&self) -> Result<Pin<Box<MaybeUninit<T>, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_zeroed_box::<T>().map(Box::into_pin)
    }
}
