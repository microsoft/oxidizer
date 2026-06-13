// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`Vec::splice`] and its [`Splice`] iterator.

use core::fmt;
use core::iter::FusedIterator;
use core::marker::PhantomData;
use core::ops::{Bound, RangeBounds};

use allocator_api2::alloc::Allocator;

use super::Vec;

impl<'a, T, A: Allocator + Clone> Vec<'a, T, A> {
    /// Replace the elements in `range` with the contents of `replace_with`,
    /// returning an iterator over the removed elements. Mirrors
    /// [`std::vec::Vec::splice`].
    ///
    /// # Divergence from `std`
    ///
    /// `std::vec::Vec::splice` is lazy: it consumes `replace_with` and inserts
    /// the replacement only when the returned iterator is dropped. This
    /// implementation is **eager** — `replace_with` is fully consumed and both
    /// the removal and the insertion happen before `splice` returns; the
    /// returned [`Splice`] then only yields the already-removed elements.
    ///
    /// The final contents of the vector are the same, but the observable side
    /// effects differ: `replace_with`'s iterator (and any of its side effects
    /// or panics) runs eagerly rather than on drop, the vector is mutated
    /// immediately rather than on drop, and dropping the returned [`Splice`]
    /// without consuming it still leaves the replacement in place.
    ///
    /// # Panics
    ///
    /// Panics if the start of the range is greater than the end, or if the end
    /// is greater than `len`.
    pub fn splice<R, I>(&mut self, range: R, replace_with: I) -> Splice<'_, 'a, T, A>
    where
        R: RangeBounds<usize>,
        I: IntoIterator<Item = T>,
    {
        let len = self.buf.len();
        let start = match range.start_bound() {
            Bound::Included(&i) => i,
            Bound::Excluded(&i) => i.checked_add(1).expect("splice: start bound overflows usize"),
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&i) => i.checked_add(1).expect("splice: end bound overflows usize"),
            Bound::Excluded(&i) => i,
            Bound::Unbounded => len,
        };
        assert!(start <= end, "splice: start > end");
        assert!(end <= len, "splice: end > len");

        // Collect the replacement first: this is the only step that can panic
        // (allocation aborts aside), and at this point `self` is untouched.
        let replacement: allocator_api2::vec::Vec<T> = replace_with.into_iter().collect();

        // Pop the whole tail `[start, len)` off (reverse order, so reverse it
        // back), then split it into the removed prefix `[start, end)` and the
        // kept suffix `[end, len)`.
        let tail_count = len - start;
        let mut tail: allocator_api2::vec::Vec<T> = allocator_api2::vec::Vec::with_capacity(tail_count);
        for _ in 0..tail_count {
            tail.push(self.buf.pop().expect("tail length matches len - start"));
        }
        tail.reverse();
        let removed_count = end - start;
        let mut tail_iter = tail.into_iter();
        let removed: allocator_api2::vec::Vec<T> = tail_iter.by_ref().take(removed_count).collect();
        let kept: allocator_api2::vec::Vec<T> = tail_iter.collect();

        // `self.buf` is now the prefix `[0, start)`. Append the replacement,
        // then the kept suffix.
        self.reserve(
            replacement
                .len()
                .checked_add(kept.len())
                .expect("splice: replacement + kept length overflows usize"),
        );
        for elem in replacement {
            self.push(elem);
        }
        for elem in kept {
            self.push(elem);
        }

        Splice {
            removed: removed.into_iter(),
            _marker: PhantomData,
        }
    }
}

/// Splicing iterator returned from [`Vec::splice`]; yields the removed
/// elements (double-ended). The replacement has already been inserted.
pub struct Splice<'d, 'a, T, A: Allocator + Clone> {
    removed: allocator_api2::vec::IntoIter<T>,
    _marker: PhantomData<&'d mut Vec<'a, T, A>>,
}

impl<T: fmt::Debug, A: Allocator + Clone> fmt::Debug for Splice<'_, '_, T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Splice").field("remaining", &self.removed.len()).finish()
    }
}

impl<T, A: Allocator + Clone> Iterator for Splice<'_, '_, T, A> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<T> {
        self.removed.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.removed.size_hint()
    }
}

impl<T, A: Allocator + Clone> DoubleEndedIterator for Splice<'_, '_, T, A> {
    #[inline]
    fn next_back(&mut self) -> Option<T> {
        self.removed.next_back()
    }
}

impl<T, A: Allocator + Clone> ExactSizeIterator for Splice<'_, '_, T, A> {}
impl<T, A: Allocator + Clone> FusedIterator for Splice<'_, '_, T, A> {}
