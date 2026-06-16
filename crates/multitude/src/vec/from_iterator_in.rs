// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::{Allocator, Global};

use crate::Arena;

/// Build a collection from an iterator, allocating into a user-supplied arena.
///
/// The arena is passed as `&'a Arena<A>`. This is the arena-aware counterpart
/// to [`core::iter::FromIterator`]. Implemented for [`Vec`](crate::vec::Vec)
/// and [`String`](crate::strings::String).
pub trait FromIteratorIn<'a, T, A: Allocator + Clone = Global>: Sized {
    /// Build the collection from `iter`, allocating into `arena`.
    fn from_iter_in<I: IntoIterator<Item = T>>(iter: I, arena: &'a Arena<A>) -> Self;
}
