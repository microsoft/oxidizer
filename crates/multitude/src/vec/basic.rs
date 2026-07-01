// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic inherent methods.

use allocator_api2::alloc::Allocator;

use super::Vec;
use crate::arena::{ExpectAlloc, panic_alloc};
use crate::internal::arena_buf::ArenaBuf;
use crate::{AllocError, Arena};

impl<'a, T, A: Allocator + Clone> Vec<'a, T, A> {
    /// Create an empty vector backed by `arena`. No allocation until the first push.
    #[inline]
    #[must_use]
    pub(crate) const fn new_in(arena: &'a Arena<A>) -> Self {
        Self::from_buf(ArenaBuf::new(), arena)
    }

    /// Create an empty vector with capacity for at least `cap` elements.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_with_capacity_in`] for a fallible variant.
    #[must_use]
    pub(crate) fn with_capacity_in(cap: usize, arena: &'a Arena<A>) -> Self {
        (Self::try_with_capacity_in(cap, arena)).expect_alloc()
    }

    /// Fallible variant of [`Self::with_capacity_in`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data
    /// alignment is at least 32 KiB.
    pub(crate) fn try_with_capacity_in(cap: usize, arena: &'a Arena<A>) -> Result<Self, AllocError> {
        let mut v = Self::new_in(arena);
        if cap > 0 {
            v.try_grow_to(cap)?;
        }
        Ok(v)
    }

    /// Push a value.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_push`] for a fallible variant.
    #[allow(
        clippy::inline_always,
        reason = "hot path; force-inlining keeps the cap-check + bump + write tight in callers' loops"
    )]
    #[inline(always)]
    pub fn push(&mut self, value: T) {
        if let Err(value) = self.buf.push_within_cap(value) {
            if self.try_grow_one().is_err() {
                panic_alloc!();
            }
            self.buf.push_within_cap(value).ok().expect("capacity grown above to fit one push");
        }
    }

    /// Fallible variant of [`Self::push`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data
    /// alignment is at least 32 KiB.
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "hot path; mirror of `push` — keep cap-check + bump + write tight in callers' loops"
    )]
    #[allow(
        clippy::missing_panics_doc,
        reason = "fallible API: the internal `.expect` guards capacity just grown above and is unreachable; a `# Panics` section would contradict the `try_` contract"
    )]
    pub fn try_push(&mut self, value: T) -> Result<(), AllocError> {
        if let Err(value) = self.buf.push_within_cap(value) {
            self.try_grow_one()?;
            self.buf.push_within_cap(value).ok().expect("capacity grown above to fit one push");
        }
        Ok(())
    }

    /// Pop a value.
    pub fn pop(&mut self) -> Option<T> {
        self.buf.pop()
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
            panic_alloc!();
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
        let needed = self.buf.len().checked_add(additional).ok_or(AllocError::CAPACITY_OVERFLOW)?;
        if needed <= self.buf.cap() {
            return Ok(());
        }
        self.try_grow_to(grow_target(self.buf.cap(), needed))
    }

    /// Drop all elements but keep the capacity.
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Returns the number of elements in the vector.
    #[must_use]
    #[inline]
    pub const fn len(&self) -> usize {
        self.buf.len()
    }

    /// Returns `true` if the vector contains no elements.
    #[must_use]
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.buf.len() == 0
    }

    /// Returns the total number of elements the vector can hold without reallocating.
    #[must_use]
    #[inline]
    pub const fn capacity(&self) -> usize {
        self.buf.cap()
    }

    /// Returns a slice view of the vector's contents.
    #[must_use]
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        self.buf.as_slice()
    }

    /// Returns a mutable slice view of the vector's contents.
    #[must_use]
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self.buf.as_mut_slice()
    }

    /// Appends all elements of `other` to this vector by copying them.
    #[inline]
    pub fn extend_from_slice(&mut self, other: impl AsRef<[T]>)
    where
        T: Copy,
    {
        if self.try_extend_from_slice(other).is_err() {
            panic_alloc!();
        }
    }

    /// Fallible variant of [`Self::extend_from_slice`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data
    /// alignment is at least 32 KiB.
    #[inline]
    pub fn try_extend_from_slice(&mut self, other: impl AsRef<[T]>) -> Result<(), AllocError>
    where
        T: Copy,
    {
        let src = other.as_ref();
        let cap = self.buf.cap();
        let len = self.buf.len();
        // `len <= cap` is a Vec invariant, so this subtraction never underflows
        // and avoids the `checked_add` overflow guard on the hot fast path.
        if cap - len < src.len() {
            let needed = len.checked_add(src.len()).ok_or(AllocError::CAPACITY_OVERFLOW)?;
            self.try_grow_to(grow_target(cap, needed))?;
        }
        self.buf.extend_copy(src);
        Ok(())
    }

    /// Build a `Vec` by collecting `iter` into a fresh vector backed by `arena`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails on growth.
    pub fn from_iter_in<I: IntoIterator<Item = T>>(iter: I, arena: &'a Arena<A>) -> Self {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        let mut v = Self::with_capacity_in(lower, arena);
        for item in iter {
            v.push(item);
        }
        v
    }

    /// Returns a raw pointer to the vector's buffer.
    #[must_use]
    #[inline]
    pub const fn as_ptr(&self) -> *const T {
        self.buf.as_ptr()
    }

    /// Returns an unsafe mutable pointer to the vector's buffer.
    #[must_use]
    #[inline]
    #[allow(clippy::needless_pass_by_ref_mut, reason = "API shape mirrors std::Vec::as_mut_ptr")]
    pub const fn as_mut_ptr(&mut self) -> *mut T {
        self.buf.as_mut_ptr()
    }

    /// Returns the remaining spare capacity of the vector as a slice of
    /// `MaybeUninit<T>`. Mirrors [`std::vec::Vec::spare_capacity_mut`].
    #[must_use]
    #[inline]
    pub fn spare_capacity_mut(&mut self) -> &mut [core::mem::MaybeUninit<T>] {
        self.buf.spare_capacity_mut()
    }
}

impl<T, A: Allocator + Clone> Vec<'_, T, A> {
    /// Grow capacity by at least 1 using the amortized doubling policy.
    #[cold]
    #[inline(never)]
    fn try_grow_one(&mut self) -> Result<(), AllocError> {
        let cur = self.buf.cap();
        let target = grow_target(cur, cur.checked_add(1).ok_or(AllocError::CAPACITY_OVERFLOW)?);
        self.try_grow_to(target)
    }
}

/// Amortized-doubling growth target: at least `needed`, at least `4`, at
/// least `2 * cur`. Saturates on overflow; the caller's `try_grow_to`
/// rejects requests whose layout overflows.
#[inline]
fn grow_target(cur: usize, needed: usize) -> usize {
    needed.max(4).max(cur.saturating_mul(2))
}
