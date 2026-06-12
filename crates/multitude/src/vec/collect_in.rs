// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::Allocator;

use crate::Arena;
use crate::vec::FromIteratorIn;

/// Extension trait on iterators that lets you collect directly into an
/// arena-backed collection.
///
/// Blanket-implemented for every `IntoIterator`. Usage typically annotates
/// the result type so the compiler picks the right `C`:
///
/// ```
/// use multitude::Arena;
/// use multitude::vec::{CollectIn, Vec};
///
/// let arena = Arena::new();
/// let v: Vec<u32, _> = (1..=10).collect_in(&arena);
/// assert_eq!(v.len(), 10);
/// ```
pub trait CollectIn: IntoIterator + Sized {
    /// Collect this iterator into `C`, allocating into `arena`.
    fn collect_in<'a, A: Allocator + Clone, C: FromIteratorIn<'a, Self::Item, A>>(self, arena: &'a Arena<A>) -> C {
        C::from_iter_in(self, arena)
    }
}

impl<I: IntoIterator + Sized> CollectIn for I {}
