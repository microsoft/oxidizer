// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared builder-shell boilerplate for arena-backed `String` (UTF-8) and
//! `Utf16String` (UTF-16). Both types are thin wrappers around
//! `crate::vec::Vec<'a, $Elem, A>` with identical lifecycle / capacity /
//! pointer-access surface. Validation, push/insert/remove, freeze, and
//! formatting are encoding-specific and stay hand-written in the per-type
//! modules.
//!
//! Element type (`u8` for UTF-8, `u16` for UTF-16) is threaded through the
//! macro because `as_ptr`/`as_mut_ptr` types differ.

macro_rules! impl_arena_string_common {
    ($Ty:ident, $Elem:ty) => {
        impl<'a, A: allocator_api2::alloc::Allocator + Clone> $Ty<'a, A> {
            /// Create a new, empty arena-backed string.
            ///
            /// No allocation is performed until the first push.
            #[must_use]
            pub(crate) const fn new_in(arena: &'a $crate::Arena<A>) -> Self {
                Self {
                    inner: $crate::vec::Vec::new_in(arena),
                }
            }

            /// Create a new arena-backed string with at least `cap` elements of
            /// pre-allocated capacity.
            ///
            /// # Panics
            ///
            /// Panics if the backing allocator fails. Use
            /// [`Self::try_with_capacity_in`] for a fallible variant.
            #[must_use]
            pub(crate) fn with_capacity_in(cap: usize, arena: &'a $crate::Arena<A>) -> Self {
                Self {
                    inner: $crate::vec::Vec::with_capacity_in(cap, arena),
                }
            }

            /// Fallible variant of [`Self::with_capacity_in`].
            ///
            /// # Errors
            ///
            /// Returns [`allocator_api2::alloc::AllocError`] if the backing
            /// allocator fails.
            pub(crate) fn try_with_capacity_in(cap: usize, arena: &'a $crate::Arena<A>) -> Result<Self, allocator_api2::alloc::AllocError> {
                Ok(Self {
                    inner: $crate::vec::Vec::try_with_capacity_in(cap, arena)?,
                })
            }

            /// Returns the length of this string in elements.
            #[must_use]
            #[inline]
            pub const fn len(&self) -> usize {
                self.inner.len()
            }

            /// Returns `true` if this string has a length of zero.
            #[must_use]
            #[inline]
            pub const fn is_empty(&self) -> bool {
                self.inner.is_empty()
            }

            /// Returns this string's capacity, in elements.
            #[must_use]
            pub const fn capacity(&self) -> usize {
                self.inner.capacity()
            }

            /// Reserve capacity for at least `additional` more elements.
            ///
            /// # Panics
            ///
            /// Panics if the backing allocator fails. Use
            /// [`Self::try_reserve`] for a fallible variant.
            #[inline]
            pub fn reserve(&mut self, additional: usize) {
                self.inner.reserve(additional);
            }

            /// Fallible variant of [`Self::reserve`].
            ///
            /// # Errors
            ///
            /// Returns [`allocator_api2::alloc::AllocError`] if the backing
            /// allocator fails.
            #[inline]
            pub fn try_reserve(&mut self, additional: usize) -> Result<(), allocator_api2::alloc::AllocError> {
                self.inner.try_reserve(additional)
            }

            /// Reserve capacity for at least `additional` more elements,
            /// without the amortized-growth slack of [`Self::reserve`].
            #[inline]
            pub fn reserve_exact(&mut self, additional: usize) {
                self.inner.reserve_exact(additional);
            }

            /// Fallible variant of [`Self::reserve_exact`].
            ///
            /// # Errors
            ///
            /// Returns [`allocator_api2::alloc::AllocError`] if the backing
            /// allocator fails.
            #[inline]
            pub fn try_reserve_exact(&mut self, additional: usize) -> Result<(), allocator_api2::alloc::AllocError> {
                self.inner.try_reserve_exact(additional)
            }

            /// Release any unused capacity back to the chunk's bump cursor.
            ///
            /// O(1) when the backing buffer is at the chunk's bump cursor;
            /// otherwise a no-op. See [`crate::vec::Vec::shrink_to_fit`].
            pub fn shrink_to_fit(&mut self) {
                self.inner.shrink_to_fit();
            }

            /// Shrink the capacity with a lower bound (in elements). See
            /// [`crate::vec::Vec::shrink_to`].
            pub fn shrink_to(&mut self, min_capacity: usize) {
                self.inner.shrink_to(min_capacity);
            }

            /// Truncates this string, removing all contents.
            ///
            /// The capacity is preserved.
            pub fn clear(&mut self) {
                self.inner.clear();
            }
        }

        impl<A: allocator_api2::alloc::Allocator + Clone> Eq for $Ty<'_, A> {}

        impl<A: allocator_api2::alloc::Allocator + Clone> PartialOrd for $Ty<'_, A> {
            fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
                Some(Ord::cmp(self, other))
            }
        }
    };
}

pub(in crate::strings) use impl_arena_string_common;
