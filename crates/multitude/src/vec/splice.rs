// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`Vec::splice`] and its [`Splice`] iterator.

use core::fmt;
use core::iter::FusedIterator;
use core::ops::RangeBounds;

use allocator_api2::alloc::Allocator;

use super::Vec;

impl<'a, T, A: Allocator + Clone> Vec<'a, T, A> {
    /// Replace the elements in `range` with `replace_with`'s contents.
    ///
    /// Returns an iterator over the removed elements. Mirrors
    /// [`std::vec::Vec::splice`], including its **lazy** semantics: the removal
    /// is finalized and the replacement inserted when the returned [`Splice`]
    /// is dropped, and `replace_with` is consumed then. The work is done in
    /// the vector's own arena storage with no temporary heap allocation.
    ///
    /// # Panics
    ///
    /// Panics if the start of the range is greater than the end, or if the end
    /// is greater than `len`.
    ///
    /// Because the replacement is inserted lazily when the returned [`Splice`]
    /// is dropped, that drop also panics if the backing allocator fails while
    /// growing the vector to hold the replacement elements.
    pub fn splice<R, I>(&mut self, range: R, replace_with: I) -> Splice<'_, 'a, I::IntoIter, A>
    where
        R: RangeBounds<usize>,
        I: IntoIterator<Item = T>,
    {
        Splice {
            drain: self.drain(range),
            replace_with: replace_with.into_iter(),
        }
    }
}

/// Iterator over the elements removed by [`Vec::splice`].
///
/// Yields the removed elements (double-ended). The replacement is inserted
/// when this iterator is dropped.
pub struct Splice<'d, 'a, I: Iterator, A: Allocator + Clone> {
    drain: super::Drain<'d, 'a, I::Item, A>,
    replace_with: I,
}

impl<I: Iterator, A: Allocator + Clone> fmt::Debug for Splice<'_, '_, I, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Splice").field("remaining", &self.drain.len()).finish()
    }
}

impl<I: Iterator, A: Allocator + Clone> Iterator for Splice<'_, '_, I, A> {
    type Item = I::Item;

    #[inline]
    fn next(&mut self) -> Option<I::Item> {
        self.drain.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.drain.size_hint()
    }
}

impl<I: Iterator, A: Allocator + Clone> DoubleEndedIterator for Splice<'_, '_, I, A> {
    #[inline]
    fn next_back(&mut self) -> Option<I::Item> {
        self.drain.next_back()
    }
}

impl<I: Iterator, A: Allocator + Clone> ExactSizeIterator for Splice<'_, '_, I, A> {}
impl<I: Iterator, A: Allocator + Clone> FusedIterator for Splice<'_, '_, I, A> {}

impl<I: Iterator, A: Allocator + Clone> Drop for Splice<'_, '_, I, A> {
    fn drop(&mut self) {
        // 1. Discard any removed elements the caller didn't consume.
        self.drain.by_ref().for_each(drop);

        // 2. Close the gap so the vector is contiguous (`[0, insert_at) ++
        //    tail`); `insert_at` is where the replacement goes.
        // SAFETY: step 1 fully consumed the drained range.
        let (mut vec_ptr, insert_at) = unsafe { self.drain.close_tail() };
        // SAFETY: the source vector is live and uniquely borrowed through the
        // drain for the duration of this drop.
        let vec = unsafe { vec_ptr.as_mut() };

        // 3. Append the replacement after the tail (growing in place in the
        //    arena, which preserves the contiguous `[0, len)` prefix), then
        //    rotate it in front of the tail: `tail ++ repl` -> `repl ++ tail`.
        let before = vec.len();
        for item in self.replace_with.by_ref() {
            vec.push(item);
        }
        // `rotate_right(0)` is a no-op when the replacement was empty.
        let added = vec.len() - before;
        vec.as_mut_slice()[insert_at..].rotate_right(added);
    }
}
