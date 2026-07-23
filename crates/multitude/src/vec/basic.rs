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
    #[expect(
        clippy::inline_always,
        reason = "hot path; force-inlining keeps the cap-check + bump + write tight in callers' loops"
    )]
    #[inline(always)]
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.push(7);
    /// assert_eq!(values.as_slice(), &[7]);
    /// ```
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
    #[expect(
        clippy::inline_always,
        reason = "hot path; mirror of `push` — keep cap-check + bump + write tight in callers' loops"
    )]
    #[expect(
        clippy::missing_panics_doc,
        reason = "fallible API: the internal `.expect` guards capacity just grown above and is unreachable; a `# Panics` section would contradict the `try_` contract"
    )]
    /// ```
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.try_push(7)?;
    /// assert_eq!(values.as_slice(), &[7]);
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_push(&mut self, value: T) -> Result<(), AllocError> {
        if let Err(value) = self.buf.push_within_cap(value) {
            self.try_grow_one()?;
            self.buf.push_within_cap(value).ok().expect("capacity grown above to fit one push");
        }
        Ok(())
    }

    /// Pop a value.
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.push(7);
    /// assert_eq!(values.pop(), Some(7));
    /// assert_eq!(values.pop(), None);
    /// ```
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
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec::<u8>();
    /// values.reserve(8);
    /// assert!(values.capacity() >= 8);
    /// ```
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
    /// ```
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec::<u8>();
    /// values.try_reserve(8)?;
    /// assert!(values.capacity() >= 8);
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), AllocError> {
        let needed = self.buf.len().checked_add(additional).ok_or(AllocError::CAPACITY_OVERFLOW)?;
        if needed <= self.buf.cap() {
            return Ok(());
        }
        self.try_grow_to(grow_target(self.buf.cap(), needed))
    }

    /// Drop all elements but keep the capacity.
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.extend_from_slice([1, 2]);
    /// values.clear();
    /// assert!(values.is_empty());
    /// ```
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Returns the number of elements in the vector.
    #[must_use]
    #[inline]
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.push(1);
    /// assert_eq!(values.len(), 1);
    /// ```
    pub const fn len(&self) -> usize {
        self.buf.len()
    }

    /// Returns `true` if the vector contains no elements.
    #[must_use]
    #[inline]
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// assert!(values.is_empty());
    /// values.push(1);
    /// assert!(!values.is_empty());
    /// ```
    pub const fn is_empty(&self) -> bool {
        self.buf.len() == 0
    }

    /// Returns the total number of elements the vector can hold without reallocating.
    #[must_use]
    #[inline]
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let values = arena.alloc_vec_with_capacity::<u8>(8);
    /// assert!(values.capacity() >= 8);
    /// ```
    pub const fn capacity(&self) -> usize {
        self.buf.cap()
    }

    /// Returns a slice view of the vector's contents.
    #[must_use]
    #[inline]
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.extend_from_slice([1, 2]);
    /// assert_eq!(values.as_slice(), &[1, 2]);
    /// ```
    pub fn as_slice(&self) -> &[T] {
        self.buf.as_slice()
    }

    /// Returns a mutable slice view of the vector's contents.
    #[must_use]
    #[inline]
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.extend_from_slice([1, 2]);
    /// values.as_mut_slice().reverse();
    /// assert_eq!(values.as_slice(), &[2, 1]);
    /// ```
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self.buf.as_mut_slice()
    }

    /// Appends all elements of `other` to this vector by copying them.
    #[inline]
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.extend_from_slice([1, 2, 3]);
    /// assert_eq!(values.as_slice(), &[1, 2, 3]);
    /// ```
    pub fn extend_from_slice(&mut self, other: impl AsRef<[T]>)
    where
        T: Copy,
    {
        let src = other.as_ref();
        if extend_needs_growth(self.buf.remaining_cap(), src.len()) {
            self.extend_from_slice_slow(src.len());
        }
        self.buf.extend_copy(src);
    }

    /// Fallible variant of [`Self::extend_from_slice`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data
    /// alignment is at least 32 KiB.
    #[inline]
    /// ```
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.try_extend_from_slice([1, 2, 3])?;
    /// assert_eq!(values.as_slice(), &[1, 2, 3]);
    /// # Ok(())
    /// # }
    /// ```
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

    #[cold]
    #[inline(never)]
    fn extend_from_slice_slow(&mut self, additional: usize) {
        if self.try_reserve(additional).is_err() {
            panic_alloc!();
        }
    }

    /// Build a `Vec` by collecting `iter` into a fresh vector backed by `arena`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails on growth.
    /// ```
    /// use multitude::Arena;
    /// use multitude::vec::Vec;
    ///
    /// let arena = Arena::new();
    /// let values = Vec::from_iter_in(1..=3, &arena);
    /// assert_eq!(values.as_slice(), &[1, 2, 3]);
    /// ```
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
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.push(9);
    /// assert_eq!(values.as_ptr(), values.as_slice().as_ptr());
    /// ```
    pub const fn as_ptr(&self) -> *const T {
        self.buf.as_ptr()
    }

    /// Returns an unsafe mutable pointer to the vector's buffer.
    #[must_use]
    #[inline]
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.push(1);
    /// // SAFETY: the pointer addresses the initialized first element.
    /// unsafe { values.as_mut_ptr().write(2) };
    /// assert_eq!(values.as_slice(), &[2]);
    /// ```
    pub const fn as_mut_ptr(&mut self) -> *mut T {
        self.buf.as_mut_ptr()
    }

    /// Return the vector's spare capacity as `MaybeUninit<T>` elements.
    ///
    /// This mirrors [`std::vec::Vec::spare_capacity_mut`].
    #[must_use]
    #[inline]
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec_with_capacity::<u8>(1);
    /// values.spare_capacity_mut()[0].write(5);
    /// // SAFETY: the first spare slot was initialized above.
    /// unsafe { values.set_len(1) };
    /// assert_eq!(values.as_slice(), &[5]);
    /// ```
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

#[inline]
#[cfg_attr(test, mutants::skip)] // `<` → `<=` only calls the cold no-op reserve path at exact capacity
const fn extend_needs_growth(remaining: usize, additional: usize) -> bool {
    remaining < additional
}
