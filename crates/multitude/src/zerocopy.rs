// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Safe zero-initialized arena allocations for `zerocopy::FromZeros` types.
//!
//! # Usage
//!
//! Access is through the [`ZerocopyView`] obtained via [`Arena::zerocopy()`](crate::Arena::zerocopy):
//!
//! ```
//! # #[cfg(feature = "zerocopy")] {
//! use multitude::Arena;
//! use zerocopy::FromZeros;
//!
//! #[derive(FromZeros)]
//! struct Header {
//!     version: u32,
//!     flags: u64,
//! }
//!
//! let arena = Arena::new();
//! let header = arena.zerocopy().alloc_arc::<Header>();
//! assert_eq!(header.version, 0);
//! assert_eq!(header.flags, 0);
//! # }
//! ```

use allocator_api2::alloc::{Allocator, Global};
use zerocopy::FromZeros;

use crate::AllocError;

/// Zero-cost view over an [`Arena`](crate::Arena) for safe zero-initialized allocation.
///
/// Exposes safe zero-initialized allocation methods for types implementing
/// the marker trait. Obtained via [`Arena`](crate::Arena)'s ecosystem-specific accessor.
///
/// ```
/// # #[cfg(feature = "zerocopy")]
/// # fn main() {
/// let arena = multitude::Arena::new();
/// let view: multitude::zerocopy::ZerocopyView<'_> = arena.zerocopy();
/// let value: multitude::Alloc<'_, u32> = view.alloc();
/// assert_eq!(*value, 0);
/// # }
/// # #[cfg(not(feature = "zerocopy"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct ZerocopyView<'a, A: Allocator + Clone = Global> {
    arena: &'a crate::Arena<A>,
}

impl<'a, A: Allocator + Clone> ZerocopyView<'a, A> {
    /// Construct a new view over the given arena.
    #[inline]
    pub(crate) const fn new(arena: &'a crate::Arena<A>) -> Self {
        Self { arena }
    }

    /// Allocate a zero-initialized `T` and return an owning [`Alloc<T>`](crate::Alloc) into the arena.
    ///
    /// The returned handle's lifetime is tied to the arena.
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() {
    /// let arena = multitude::Arena::new();
    /// let value: multitude::Alloc<'_, u32> = arena.zerocopy().alloc::<u32>();
    /// assert_eq!(*value, 0);
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 64 KiB or greater (which exceeds the arena chunk alignment).
    #[must_use]
    #[inline]
    pub fn alloc<T: FromZeros>(&self) -> crate::Alloc<'a, T> {
        self.arena
            .try_alloc_with::<T, _>(T::new_zeroed)
            .expect("zerocopy: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment
    /// >= 64 KiB.
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = multitude::Arena::new();
    /// let value: multitude::Alloc<'_, u32> = arena.zerocopy().try_alloc::<u32>()?;
    /// assert_eq!(*value, 0);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    #[inline]
    pub fn try_alloc<T: FromZeros>(&self) -> Result<crate::Alloc<'a, T>, AllocError> {
        self.arena.try_alloc_with::<T, _>(T::new_zeroed)
    }

    /// Allocate a zero-initialized `T` slice in an owning [`Alloc`](crate::Alloc).
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() {
    /// let arena = multitude::Arena::new();
    /// let values: multitude::Alloc<'_, [u32]> = arena.zerocopy().alloc_slice(3);
    /// assert_eq!(&*values, &[0; 3]);
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 64 KiB or greater.
    #[must_use]
    #[inline]
    pub fn alloc_slice<T: FromZeros>(&self, len: usize) -> crate::Alloc<'a, [T]> {
        self.arena
            .try_alloc_slice_fill_with(len, |_| T::new_zeroed())
            .expect("zerocopy: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_slice`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment
    /// >= 64 KiB.
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = multitude::Arena::new();
    /// let values: multitude::Alloc<'_, [u32]> = arena.zerocopy().try_alloc_slice(3)?;
    /// assert_eq!(&*values, &[0; 3]);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    #[inline]
    pub fn try_alloc_slice<T: FromZeros>(&self, len: usize) -> Result<crate::Alloc<'a, [T]>, AllocError> {
        self.arena.try_alloc_slice_fill_with(len, |_| T::new_zeroed())
    }

    /// Allocate a zero-initialized `T` and return a [`Box<T, A>`](crate::Box).
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() {
    /// let arena = multitude::Arena::new();
    /// let value: multitude::Box<u32> = arena.zerocopy().alloc_box();
    /// assert_eq!(*value, 0);
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
    #[must_use]
    #[inline]
    pub fn alloc_box<T: FromZeros>(&self) -> crate::Box<T, A> {
        self.arena
            .try_alloc_box_with::<T, _>(T::new_zeroed)
            .expect("zerocopy: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment
    /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = multitude::Arena::new();
    /// let value: multitude::Box<u32> = arena.zerocopy().try_alloc_box()?;
    /// assert_eq!(*value, 0);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    #[inline]
    pub fn try_alloc_box<T: FromZeros>(&self) -> Result<crate::Box<T, A>, AllocError> {
        self.arena.try_alloc_box_with::<T, _>(T::new_zeroed)
    }

    /// Allocate a zero-initialized `T` and return an [`Arc<T, A>`](crate::Arc).
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() {
    /// let arena = multitude::Arena::new();
    /// let value: multitude::Arc<u32> = arena.zerocopy().alloc_arc();
    /// assert_eq!(*value, 0);
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
    #[must_use]
    #[inline]
    pub fn alloc_arc<T: FromZeros + Send + Sync>(&self) -> crate::Arc<T, A>
    where
        A: Send + Sync,
    {
        self.arena
            .try_alloc_arc_with::<T, _>(T::new_zeroed)
            .expect("zerocopy: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment
    /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = multitude::Arena::new();
    /// let value: multitude::Arc<u32> = arena.zerocopy().try_alloc_arc()?;
    /// assert_eq!(*value, 0);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    #[inline]
    pub fn try_alloc_arc<T: FromZeros + Send + Sync>(&self) -> Result<crate::Arc<T, A>, AllocError>
    where
        A: Send + Sync,
    {
        self.arena.try_alloc_arc_with::<T, _>(T::new_zeroed)
    }

    /// Allocate a zero-initialized slice of `T` and return an [`Arc<[T], A>`](crate::Arc).
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() {
    /// let arena = multitude::Arena::new();
    /// let values: multitude::Arc<[u32]> = arena.zerocopy().alloc_slice_arc(3);
    /// assert_eq!(&*values, &[0; 3]);
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
    #[must_use]
    #[inline]
    pub fn alloc_slice_arc<T: FromZeros + Send + Sync>(&self, len: usize) -> crate::Arc<[T], A>
    where
        A: Send + Sync,
    {
        self.arena
            .try_alloc_slice_fill_with_arc::<T, _>(len, |_| T::new_zeroed())
            .expect("zerocopy: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_slice_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment
    /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = multitude::Arena::new();
    /// let values: multitude::Arc<[u32]> = arena.zerocopy().try_alloc_slice_arc(3)?;
    /// assert_eq!(&*values, &[0; 3]);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    #[inline]
    pub fn try_alloc_slice_arc<T: FromZeros + Send + Sync>(&self, len: usize) -> Result<crate::Arc<[T], A>, AllocError>
    where
        A: Send + Sync,
    {
        self.arena.try_alloc_slice_fill_with_arc::<T, _>(len, |_| T::new_zeroed())
    }

    /// Allocate a zero-initialized `T` and return an [`Rc<T, A>`](crate::Rc).
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() {
    /// let arena = multitude::Arena::new();
    /// let value: multitude::Rc<u32> = arena.zerocopy().alloc_rc();
    /// assert_eq!(*value, 0);
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater.
    #[must_use]
    #[inline]
    pub fn alloc_rc<T: FromZeros>(&self) -> crate::Rc<T, A> {
        self.arena
            .try_alloc_rc_with::<T, _>(T::new_zeroed)
            .expect("zerocopy: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment greater or equal to 32 KiB.
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = multitude::Arena::new();
    /// let value: multitude::Rc<u32> = arena.zerocopy().try_alloc_rc()?;
    /// assert_eq!(*value, 0);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    #[inline]
    pub fn try_alloc_rc<T: FromZeros>(&self) -> Result<crate::Rc<T, A>, AllocError> {
        self.arena.try_alloc_rc_with::<T, _>(T::new_zeroed)
    }

    /// Allocate a zero-initialized slice of `T` and return an [`Rc<[T], A>`](crate::Rc).
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() {
    /// let arena = multitude::Arena::new();
    /// let values: multitude::Rc<[u32]> = arena.zerocopy().alloc_slice_rc(3);
    /// assert_eq!(&*values, &[0; 3]);
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater.
    #[must_use]
    #[inline]
    pub fn alloc_slice_rc<T: FromZeros>(&self, len: usize) -> crate::Rc<[T], A> {
        self.arena
            .try_alloc_slice_fill_with_rc::<T, _>(len, |_| T::new_zeroed())
            .expect("zerocopy: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_slice_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment greater or equal to 32 KiB.
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = multitude::Arena::new();
    /// let values: multitude::Rc<[u32]> = arena.zerocopy().try_alloc_slice_rc(3)?;
    /// assert_eq!(&*values, &[0; 3]);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    #[inline]
    pub fn try_alloc_slice_rc<T: FromZeros>(&self, len: usize) -> Result<crate::Rc<[T], A>, AllocError> {
        self.arena.try_alloc_slice_fill_with_rc::<T, _>(len, |_| T::new_zeroed())
    }

    /// Allocate a zero-initialized slice of `T` and return a [`Box<[T], A>`](crate::Box).
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() {
    /// let arena = multitude::Arena::new();
    /// let values: multitude::Box<[u32]> = arena.zerocopy().alloc_slice_box(3);
    /// assert_eq!(&*values, &[0; 3]);
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
    #[must_use]
    #[inline]
    pub fn alloc_slice_box<T: FromZeros>(&self, len: usize) -> crate::Box<[T], A> {
        self.arena
            .try_alloc_slice_fill_with_box::<T, _>(len, |_| T::new_zeroed())
            .expect("zerocopy: arena allocation failed")
    }

    /// Fallible variant of [`Self::alloc_slice_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if `T` requires alignment
    /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")]
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = multitude::Arena::new();
    /// let values: multitude::Box<[u32]> = arena.zerocopy().try_alloc_slice_box(3)?;
    /// assert_eq!(&*values, &[0; 3]);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "zerocopy"))]
    /// # fn main() {}
    /// ```
    #[inline]
    pub fn try_alloc_slice_box<T: FromZeros>(&self, len: usize) -> Result<crate::Box<[T], A>, AllocError> {
        self.arena.try_alloc_slice_fill_with_box::<T, _>(len, |_| T::new_zeroed())
    }
}
