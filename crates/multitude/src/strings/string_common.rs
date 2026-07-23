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

            /// Create an arena-backed string with capacity for `cap` elements.
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
            /// Returns [`$crate::AllocError`] if the backing allocator fails,
            /// or if computing the required capacity overflows `usize`. Use
            /// [`$crate::AllocError::is_allocator_failure`] and
            /// [`$crate::AllocError::is_capacity_overflow`] to tell the two apart.
            pub(crate) fn try_with_capacity_in(cap: usize, arena: &'a $crate::Arena<A>) -> Result<Self, $crate::AllocError> {
                Ok(Self {
                    inner: $crate::vec::Vec::try_with_capacity_in(cap, arena)?,
                })
            }

            /// Returns the length of this string in elements.
            #[must_use]
            #[inline]
            /// ```
            /// use multitude::Arena;
            ///
            /// let arena = Arena::new();
            /// let mut value = arena.alloc_string();
            /// value.push_str("rust");
            /// assert_eq!(value.len(), 4);
            /// ```
            pub const fn len(&self) -> usize {
                self.inner.len()
            }

            /// Returns `true` if this string has a length of zero.
            #[must_use]
            #[inline]
            /// ```
            /// use multitude::Arena;
            ///
            /// let arena = Arena::new();
            /// let mut value = arena.alloc_string();
            /// assert!(value.is_empty());
            /// value.push('x');
            /// assert!(!value.is_empty());
            /// ```
            pub const fn is_empty(&self) -> bool {
                self.inner.is_empty()
            }

            /// Returns this string's capacity, in elements.
            #[must_use]
            /// ```
            /// use multitude::Arena;
            ///
            /// let arena = Arena::new();
            /// let value = arena.alloc_string_with_capacity(8);
            /// assert!(value.capacity() >= 8);
            /// ```
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
            /// ```
            /// use multitude::Arena;
            ///
            /// let arena = Arena::new();
            /// let mut value = arena.alloc_string();
            /// value.reserve(8);
            /// assert!(value.capacity() >= 8);
            /// ```
            pub fn reserve(&mut self, additional: usize) {
                self.inner.reserve(additional);
            }

            /// Fallible variant of [`Self::reserve`].
            ///
            /// # Errors
            ///
            /// Returns [`$crate::AllocError`] if the backing allocator fails,
            /// or if `len + additional` overflows `usize`. Use
            /// [`$crate::AllocError::is_allocator_failure`] and
            /// [`$crate::AllocError::is_capacity_overflow`] to tell the two apart.
            #[inline]
            /// ```
            /// # fn main() -> Result<(), multitude::AllocError> {
            /// use multitude::Arena;
            ///
            /// let arena = Arena::new();
            /// let mut value = arena.alloc_string();
            /// value.try_reserve(8)?;
            /// assert!(value.capacity() >= 8);
            /// # Ok(())
            /// # }
            /// ```
            pub fn try_reserve(&mut self, additional: usize) -> Result<(), $crate::AllocError> {
                self.inner.try_reserve(additional)
            }

            /// Reserve exactly enough capacity for `additional` elements.
            ///
            /// This omits the amortized-growth slack of [`Self::reserve`].
            #[inline]
            /// ```
            /// use multitude::Arena;
            ///
            /// let arena = Arena::new();
            /// let mut value = arena.alloc_string();
            /// value.reserve_exact(4);
            /// assert_eq!(value.capacity(), 4);
            /// ```
            pub fn reserve_exact(&mut self, additional: usize) {
                self.inner.reserve_exact(additional);
            }

            /// Fallible variant of [`Self::reserve_exact`].
            ///
            /// # Errors
            ///
            /// Returns [`$crate::AllocError`] if the backing allocator fails,
            /// or if `len + additional` overflows `usize`. Use
            /// [`$crate::AllocError::is_allocator_failure`] and
            /// [`$crate::AllocError::is_capacity_overflow`] to tell the two apart.
            #[inline]
            /// ```
            /// # fn main() -> Result<(), multitude::AllocError> {
            /// use multitude::Arena;
            ///
            /// let arena = Arena::new();
            /// let mut value = arena.alloc_string();
            /// value.try_reserve_exact(4)?;
            /// assert_eq!(value.capacity(), 4);
            /// # Ok(())
            /// # }
            /// ```
            pub fn try_reserve_exact(&mut self, additional: usize) -> Result<(), $crate::AllocError> {
                self.inner.try_reserve_exact(additional)
            }

            /// Release any unused capacity back to the chunk's bump cursor.
            ///
            /// O(1) when the backing buffer is at the chunk's bump cursor;
            /// otherwise a no-op. See [`crate::vec::Vec::shrink_to_fit`].
            /// ```
            /// use multitude::Arena;
            ///
            /// let arena = Arena::new();
            /// let mut value = arena.alloc_string_with_capacity(8);
            /// value.push_str("hi");
            /// value.shrink_to_fit();
            /// assert_eq!(value.capacity(), value.len());
            /// ```
            pub fn shrink_to_fit(&mut self) {
                self.inner.shrink_to_fit();
            }

            /// Shrink the capacity with a lower bound (in elements). See
            /// [`crate::vec::Vec::shrink_to`].
            /// ```
            /// use multitude::Arena;
            ///
            /// let arena = Arena::new();
            /// let mut value = arena.alloc_string_with_capacity(8);
            /// value.push_str("hi");
            /// value.shrink_to(4);
            /// assert!(value.capacity() >= 4);
            /// ```
            pub fn shrink_to(&mut self, min_capacity: usize) {
                self.inner.shrink_to(min_capacity);
            }

            /// Truncates this string, removing all contents.
            ///
            /// The capacity is preserved.
            /// ```
            /// use multitude::Arena;
            ///
            /// let arena = Arena::new();
            /// let mut value = arena.alloc_string();
            /// value.push_str("text");
            /// value.clear();
            /// assert!(value.is_empty());
            /// ```
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
