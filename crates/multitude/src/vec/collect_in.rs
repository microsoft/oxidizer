// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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
    /// Collect this iterator into `C`, using `allocator` as the backing
    /// allocator smart pointer.
    fn collect_in<C: FromIteratorIn<Self::Item>>(self, allocator: C::Allocator) -> C {
        C::from_iter_in(self, allocator)
    }
}

impl<I: IntoIterator + Sized> CollectIn for I {}
