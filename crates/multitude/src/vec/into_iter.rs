// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Owning iterator returned by [`Vec::into_iter`].

use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::{fmt, iter};

use allocator_api2::alloc::Allocator;

use super::Vec;
use crate::Arena;
use crate::internal::arena_buf::DrainAll;

/// Owning iterator over the elements of an arena-backed [`Vec`].
///
/// The iterator extracts the underlying buffer from the consumed `Vec`,
/// then yields ownership of each element via an internal drain. The
/// buffer's backing storage is reclaimed by the arena when the arena
/// itself is torn down.
pub struct IntoIter<'a, T, A: Allocator + Clone> {
    inner: DrainAll<'a, T>,
    // Hold the arena reference so the iterator's lifetime is tied to it.
    _arena: PhantomData<&'a Arena<A>>,
}

impl<'a, T, A: Allocator + Clone> IntoIter<'a, T, A> {
    /// Consume `vec` to construct the owning iterator.
    #[inline]
    pub(super) fn new(vec: Vec<'a, T, A>) -> Self {
        let mut me = ManuallyDrop::new(vec);
        let inner = me.buf.drain_all();
        Self {
            inner,
            _arena: PhantomData,
        }
    }
}

impl<T, A: Allocator + Clone> Iterator for IntoIter<'_, T, A> {
    type Item = T;
    #[inline]
    fn next(&mut self) -> Option<T> {
        self.inner.next()
    }
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<T, A: Allocator + Clone> DoubleEndedIterator for IntoIter<'_, T, A> {
    #[inline]
    fn next_back(&mut self) -> Option<T> {
        self.inner.next_back()
    }
}

impl<T, A: Allocator + Clone> ExactSizeIterator for IntoIter<'_, T, A> {}
impl<T, A: Allocator + Clone> iter::FusedIterator for IntoIter<'_, T, A> {}

impl<T: fmt::Debug, A: Allocator + Clone> fmt::Debug for IntoIter<'_, T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntoIter").field("remaining", &self.inner.size_hint().0).finish()
    }
}
