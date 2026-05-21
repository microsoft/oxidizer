// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic inherent methods.

use core::ptr::{self, NonNull};
use core::slice;

use allocator_api2::alloc::{AllocError, Allocator};

use super::Vec;
use crate::Arena;

impl<'a, T, A: Allocator + Clone> Vec<'a, T, A> {
    /// Create an empty vector backed by `arena`. No allocation until the first push.
    #[inline]
    #[must_use]
    pub const fn new_in(arena: &'a Arena<A>) -> Self {
        Self {
            arena,
            data: NonNull::dangling(),
            len: 0,
            cap: 0,
        }
    }

    /// Create an empty vector with capacity for at least `cap` elements.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_with_capacity_in`] for a fallible variant.
    #[must_use]
    pub fn with_capacity_in(cap: usize, arena: &'a Arena<A>) -> Self {
        match Self::try_with_capacity_in(cap, arena) {
            Ok(vec) => vec,
            Err(_e) => panic!("multitude: allocator returned AllocError"),
        }
    }

    /// Fallible variant of [`Self::with_capacity_in`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data
    /// alignment is at least 32 KiB.
    pub fn try_with_capacity_in(cap: usize, arena: &'a Arena<A>) -> Result<Self, AllocError> {
        let mut vec = Self::new_in(arena);
        vec.try_reserve_exact(cap)?;
        Ok(vec)
    }

    /// Push a value.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_push`] for a fallible variant.
    #[expect(
        clippy::inline_always,
        reason = "hot path; force-inlining keeps the cap-check + bump + write tight in callers' loops"
    )]
    #[inline(always)]
    pub fn push(&mut self, value: T) {
        if self.len == self.cap {
            self.grow_one();
        }
        // SAFETY: the capacity check above guarantees `len < cap`, so this slot is allocated and uninitialized.
        unsafe { self.data.as_ptr().add(self.len).write(value) };
        self.len += 1;
    }

    #[cold]
    #[inline(never)]
    pub(super) fn grow_one(&mut self) {
        if self.try_grow_amortized(1).is_err() {
            panic!("multitude: allocator returned AllocError");
        }
    }

    /// Fallible variant of [`Self::push`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data
    /// alignment is at least 32 KiB.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "hot path; mirror of `push` — keep cap-check + bump + write tight in callers' loops"
    )]
    pub fn try_push(&mut self, value: T) -> Result<(), AllocError> {
        if self.len == self.cap {
            self.try_grow_amortized(1)?;
        }
        // SAFETY: growth above guarantees `len < cap`, so this slot is allocated and uninitialized.
        unsafe { self.data.as_ptr().add(self.len).write(value) };
        self.len += 1;
        Ok(())
    }

    /// Pop a value.
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        // SAFETY: the old last element was initialized; decrementing len transfers ownership to the caller.
        Some(unsafe { self.data.as_ptr().add(self.len).read() })
    }

    /// Reserve capacity for at least `additional` more elements.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_reserve`] for a fallible variant.
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        if self.try_reserve(additional).is_err() {
            panic!("multitude: allocator returned AllocError");
        }
    }

    /// Fallible variant of [`Self::reserve`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data
    /// alignment is at least 32 KiB.
    #[inline]
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), AllocError> {
        self.try_grow_amortized(additional)
    }

    /// Drop all elements but keep the capacity.
    pub fn clear(&mut self) {
        self.truncate(0);
    }

    /// Returns the number of elements in the vector.
    #[must_use]
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the vector contains no elements.
    #[must_use]
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the total number of elements the vector can hold without reallocating.
    #[must_use]
    #[inline]
    pub const fn capacity(&self) -> usize {
        self.cap
    }

    /// Returns a slice view of the vector's contents.
    #[must_use]
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        // SAFETY: `data` points to `cap` elements (or is dangling for len 0/ZST), and the first `len` are initialized.
        unsafe { slice::from_raw_parts(self.data.as_ptr(), self.len) }
    }

    /// Returns a mutable slice view of the vector's contents.
    #[must_use]
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        // SAFETY: `data` points to `cap` elements (or is dangling for len 0/ZST), and the first `len` are initialized.
        unsafe { slice::from_raw_parts_mut(self.data.as_ptr(), self.len) }
    }

    /// Appends all elements of `other` to this vector by copying them.
    #[inline]
    pub fn extend_from_slice(&mut self, other: impl AsRef<[T]>)
    where
        T: Copy,
    {
        let other = other.as_ref();
        self.reserve(other.len());
        // SAFETY: reserve guarantees enough tail capacity, and the destination tail is uninitialized.
        unsafe { ptr::copy_nonoverlapping(other.as_ptr(), self.data.as_ptr().add(self.len), other.len()) };
        self.len += other.len();
    }

    /// Build a `Vec` by collecting `iter` into a fresh vector backed by `arena`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails on growth.
    pub fn from_iter_in<I: IntoIterator<Item = T>>(iter: I, arena: &'a Arena<A>) -> Self {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        let mut vec = Self::with_capacity_in(lower, arena);
        vec.extend(iter);
        vec
    }

    /// Returns a raw pointer to the vector's buffer.
    #[must_use]
    #[inline]
    pub const fn as_ptr(&self) -> *const T {
        self.data.as_ptr()
    }

    /// Returns an unsafe mutable pointer to the vector's buffer.
    #[must_use]
    #[inline]
    #[expect(clippy::needless_pass_by_ref_mut, reason = "API shape mirrors std::Vec::as_mut_ptr")]
    pub const fn as_mut_ptr(&mut self) -> *mut T {
        self.data.as_ptr()
    }
}
