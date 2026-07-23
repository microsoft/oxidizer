// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::{Allocator, Global};

use crate::Arena;

/// Build a collection from an iterator, allocating into a user-supplied arena.
///
/// The arena is passed as `&'a Arena<A>`. This is the arena-aware counterpart
/// to [`core::iter::FromIterator`]. Implemented for [`Vec`](crate::vec::Vec)
/// and [`String`](crate::strings::String).
/// ```
/// use multitude::Arena;
/// use multitude::vec::{FromIteratorIn, Vec};
///
/// let arena = Arena::new();
/// let values = Vec::from_iter_in(1..=3, &arena);
/// assert_eq!(values.as_slice(), &[1, 2, 3]);
/// ```
pub trait FromIteratorIn<'a, T, A: Allocator + Clone = Global>: Sized {
    /// Build the collection from `iter`, allocating into `arena`.
    /// ```
    /// use multitude::Arena;
    /// use multitude::vec::{FromIteratorIn, Vec};
    ///
    /// let arena = Arena::new();
    /// let values = Vec::from_iter_in([2, 4], &arena);
    /// assert_eq!(values.as_slice(), &[2, 4]);
    /// ```
    fn from_iter_in<I: IntoIterator<Item = T>>(iter: I, arena: &'a Arena<A>) -> Self;
}
