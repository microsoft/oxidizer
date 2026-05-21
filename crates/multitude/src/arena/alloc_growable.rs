// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Growable container builders on [`Arena`]: [`String`] and [`Vec`].
//!
//! All public methods are documented on [`Arena`] itself; this file
//! groups the family together to keep the central `mod.rs` smaller.

use allocator_api2::alloc::{AllocError, Allocator};

use super::{Arena, expect_alloc};
use crate::strings::String;
use crate::vec::Vec;

impl<A: Allocator + Clone> Arena<A> {
    /// Create a new, empty growable [`String`](crate::strings::String) backed by this
    /// arena. No allocation is performed until the first push.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut s = arena.alloc_string();
    /// s.push_str("hello");
    /// assert_eq!(&*s, "hello");
    /// ```
    #[must_use]
    #[inline]
    pub const fn alloc_string(&self) -> String<'_, A> {
        String::new_in(self)
    }

    /// Create a new growable arena-backed [`String`](crate::strings::String) with capacity.
    ///
    /// At least `cap` bytes are pre-allocated.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails. Use
    /// [`Self::try_alloc_string_with_capacity`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut s = arena.alloc_string_with_capacity(64);
    /// s.push_str("preallocated");
    /// assert!(s.capacity() >= 64);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_string_with_capacity(&self, cap: usize) -> String<'_, A> {
        expect_alloc(self.try_alloc_string_with_capacity(cap))
    }

    /// Fallible variant of [`Self::alloc_string_with_capacity`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    #[inline]
    pub fn try_alloc_string_with_capacity(&self, cap: usize) -> Result<String<'_, A>, AllocError> {
        String::try_with_capacity_in(cap, self)
    }

    /// Create a new, empty growable [`Vec`](crate::vec::Vec) backed by this arena.
    /// No allocation is performed until the first push.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut v = arena.alloc_vec::<u32>();
    /// v.push(1);
    /// v.push(2);
    /// assert_eq!(v.as_slice(), &[1, 2]);
    /// ```
    #[must_use]
    #[inline]
    pub const fn alloc_vec<T>(&self) -> Vec<'_, T, A> {
        Vec::new_in(self)
    }

    /// Create a new growable arena-backed [`Vec`](crate::vec::Vec) with capacity.
    ///
    /// At least `cap` elements of capacity are pre-allocated.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_vec_with_capacity`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut v = arena.alloc_vec_with_capacity::<u32>(100);
    /// for i in 0..50 {
    ///     v.push(i);
    /// }
    /// assert!(v.capacity() >= 100);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_vec_with_capacity<T>(&self, cap: usize) -> Vec<'_, T, A> {
        expect_alloc(self.try_alloc_vec_with_capacity(cap))
    }

    /// Fallible variant of [`Self::alloc_vec_with_capacity`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_vec_with_capacity<T>(&self, cap: usize) -> Result<Vec<'_, T, A>, AllocError> {
        Vec::try_with_capacity_in(cap, self)
    }
}
