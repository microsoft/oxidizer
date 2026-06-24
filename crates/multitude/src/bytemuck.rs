// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Safe zero-initialized arena allocations for `bytemuck::Zeroable` types.
//!
//! # Usage
//!
//! Access is through the [`BytemuckView`] obtained via [`Arena::bytemuck()`](crate::Arena::bytemuck):
//!
//! ```
//! # #[cfg(feature = "bytemuck")] {
//! use bytemuck::Zeroable;
//! use multitude::Arena;
//!
//! #[derive(Clone, Copy, Zeroable)]
//! #[repr(C)]
//! struct Pixel {
//!     r: u8,
//!     g: u8,
//!     b: u8,
//!     a: u8,
//! }
//!
//! let arena = Arena::new();
//! let pixel = arena.bytemuck().alloc_arc::<Pixel>();
//! assert_eq!(pixel.r, 0);
//! assert_eq!(pixel.a, 0);
//! # }
//! ```

use allocator_api2::alloc::{AllocError, Allocator, Global};
use bytemuck::Zeroable;

/// Zero-cost view over an [`Arena`](crate::Arena) for safe zero-initialized allocation.
///
/// Exposes safe zero-initialized allocation methods for types implementing
/// the marker trait. Obtained via [`Arena`](crate::Arena)'s ecosystem-specific accessor.
#[derive(Debug)]
pub struct BytemuckView<'a, A: Allocator + Clone = Global> {
    arena: &'a crate::Arena<A>,
}

impl<'a, A: Allocator + Clone> BytemuckView<'a, A> {
    /// Construct a new view over the given arena.
    #[inline]
    pub(crate) const fn new(arena: &'a crate::Arena<A>) -> Self {
        Self { arena }
    }

    /// Allocate a zero-initialized `T` and return an owning [`Alloc<T>`](crate::Alloc) into the arena.
    ///
    /// The returned handle's lifetime is tied to the arena.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 64 KiB or greater (which exceeds the arena chunk alignment).
    #[must_use]
    #[inline]
    pub fn alloc<T: Zeroable>(&self) -> crate::Alloc<'a, T> {
        self.arena
            .try_alloc_with::<T, _>(T::zeroed)
            .expect("bytemuck: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment
    /// >= 64 KiB.
    #[inline]
    pub fn try_alloc<T: Zeroable>(&self) -> Result<crate::Alloc<'a, T>, AllocError> {
        self.arena.try_alloc_with::<T, _>(T::zeroed)
    }

    /// Allocate a zero-initialized slice of `T` and return an owning [`Alloc<[T]>`](crate::Alloc) into the arena.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 64 KiB or greater.
    #[must_use]
    #[inline]
    pub fn alloc_slice<T: Zeroable>(&self, len: usize) -> crate::Alloc<'a, [T]> {
        self.arena
            .try_alloc_slice_fill_with(len, |_| T::zeroed())
            .expect("bytemuck: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_slice`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment
    /// >= 64 KiB.
    #[inline]
    pub fn try_alloc_slice<T: Zeroable>(&self, len: usize) -> Result<crate::Alloc<'a, [T]>, AllocError> {
        self.arena.try_alloc_slice_fill_with(len, |_| T::zeroed())
    }

    /// Allocate a zero-initialized `T` and return a [`Box<T, A>`](crate::Box).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
    #[must_use]
    #[inline]
    pub fn alloc_box<T: Zeroable>(&self) -> crate::Box<T, A> {
        self.arena
            .try_alloc_box_with::<T, _>(T::zeroed)
            .expect("bytemuck: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment
    /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
    #[inline]
    pub fn try_alloc_box<T: Zeroable>(&self) -> Result<crate::Box<T, A>, AllocError> {
        self.arena.try_alloc_box_with::<T, _>(T::zeroed)
    }

    /// Allocate a zero-initialized `T` and return an [`Arc<T, A>`](crate::Arc).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
    #[must_use]
    #[inline]
    pub fn alloc_arc<T: Zeroable + Send + Sync>(&self) -> crate::Arc<T, A>
    where
        A: Send + Sync,
    {
        self.arena
            .try_alloc_arc_with::<T, _>(T::zeroed)
            .expect("bytemuck: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment
    /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
    #[inline]
    pub fn try_alloc_arc<T: Zeroable + Send + Sync>(&self) -> Result<crate::Arc<T, A>, AllocError>
    where
        A: Send + Sync,
    {
        self.arena.try_alloc_arc_with::<T, _>(T::zeroed)
    }

    /// Allocate a zero-initialized slice of `T` and return an [`Arc<[T], A>`](crate::Arc).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
    #[must_use]
    #[inline]
    pub fn alloc_slice_arc<T: Zeroable + Send + Sync>(&self, len: usize) -> crate::Arc<[T], A>
    where
        A: Send + Sync,
    {
        self.arena
            .try_alloc_slice_fill_with_arc::<T, _>(len, |_| T::zeroed())
            .expect("bytemuck: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_slice_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment
    /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
    #[inline]
    pub fn try_alloc_slice_arc<T: Zeroable + Send + Sync>(&self, len: usize) -> Result<crate::Arc<[T], A>, AllocError>
    where
        A: Send + Sync,
    {
        self.arena.try_alloc_slice_fill_with_arc::<T, _>(len, |_| T::zeroed())
    }

    /// Allocate a zero-initialized `T` and return an [`Rc<T, A>`](crate::Rc).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater.
    #[must_use]
    #[inline]
    pub fn alloc_rc<T: Zeroable>(&self) -> crate::Rc<T, A> {
        self.arena
            .try_alloc_rc_with::<T, _>(T::zeroed)
            .expect("bytemuck: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment greater or equal to 32 KiB.
    #[inline]
    pub fn try_alloc_rc<T: Zeroable>(&self) -> Result<crate::Rc<T, A>, AllocError> {
        self.arena.try_alloc_rc_with::<T, _>(T::zeroed)
    }

    /// Allocate a zero-initialized slice of `T` and return an [`Rc<[T], A>`](crate::Rc).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater.
    #[must_use]
    #[inline]
    pub fn alloc_slice_rc<T: Zeroable>(&self, len: usize) -> crate::Rc<[T], A> {
        self.arena
            .try_alloc_slice_fill_with_rc::<T, _>(len, |_| T::zeroed())
            .expect("bytemuck: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_slice_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment greater or equal to 32 KiB.
    #[inline]
    pub fn try_alloc_slice_rc<T: Zeroable>(&self, len: usize) -> Result<crate::Rc<[T], A>, AllocError> {
        self.arena.try_alloc_slice_fill_with_rc::<T, _>(len, |_| T::zeroed())
    }

    /// Allocate a zero-initialized slice of `T` and return a [`Box<[T], A>`](crate::Box).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
    #[must_use]
    #[inline]
    pub fn alloc_slice_box<T: Zeroable>(&self, len: usize) -> crate::Box<[T], A> {
        self.arena
            .try_alloc_slice_fill_with_box::<T, _>(len, |_| T::zeroed())
            .expect("bytemuck: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_slice_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment
    /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
    #[inline]
    pub fn try_alloc_slice_box<T: Zeroable>(&self, len: usize) -> Result<crate::Box<[T], A>, AllocError> {
        self.arena.try_alloc_slice_fill_with_box::<T, _>(len, |_| T::zeroed())
    }
}
