// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Build a collection from an iterator, allocating into a user-supplied allocator.
///
/// The allocator is passed as a smart-pointer-shaped value (for our types,
/// `&'a Arena<A>`). This is the arena-aware counterpart to
/// [`core::iter::FromIterator`]. Implemented for [`Vec`](crate::vec::Vec)
/// and [`String`](crate::strings::String).
pub trait FromIteratorIn<T>: Sized {
    /// The allocator smart pointer this collection needs in order to be built.
    type Allocator;

    /// Build the collection from `iter`, allocating into `allocator`.
    fn from_iter_in<I: IntoIterator<Item = T>>(iter: I, allocator: Self::Allocator) -> Self;
}
