// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `Vec::drain` and its iterator.

use core::iter::FusedIterator;
use core::marker::PhantomData;
use core::ops::{Bound, RangeBounds};
use core::{fmt, mem};

use allocator_api2::alloc::Allocator;

use super::Vec;

impl<'a, T, A: Allocator + Clone> Vec<'a, T, A> {
    /// Drain a range of elements.
    ///
    /// # Panics
    ///
    /// Panics if the start of the range is greater than the end, or if the
    /// end is greater than `len`.
    pub fn drain<R: RangeBounds<usize>>(&mut self, range: R) -> Drain<'_, 'a, T, A> {
        let len = self.buf.len();
        let start = match range.start_bound() {
            Bound::Included(&i) => i,
            Bound::Excluded(&i) => i + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&i) => i + 1,
            Bound::Excluded(&i) => i,
            Bound::Unbounded => len,
        };
        assert!(start <= end, "drain: start > end");
        assert!(end <= len, "drain: end > len");

        // Strategy: move the [start, end) elements into a heap-backed
        // staging vec, leaving a "hole" in `self.buf`. The Drain iterator
        // yields from the staging vec; on drop, it closes the hole by
        // sliding the tail (already-initialized) elements left and
        // truncating `self.buf` to `len - drained`.
        //
        // We use safe `swap_remove` semantics implemented via element-wise
        // shifting: pull out the drained values, then compact.
        //
        // For the staging vec we use a `std`-allocated Vec to keep the
        // drain iterator simple and avoid arena chunk churn.

        // 1. Move out the drained slice in order.
        let drained_count = end - start;
        let mut staged: allocator_api2::vec::Vec<T> = allocator_api2::vec::Vec::with_capacity(drained_count);
        // Take each element by index; we rebuild the vector after.
        // We do this via mem::replace on the slot using a "ghost" value:
        // since `T` isn't `Default`, we can't fill — so instead, we
        // truncate-then-rebuild. We pop the whole tail (after `start`)
        // into a temp, then push the kept tail back.
        let tail_count = len - start;
        let mut tail_staging: allocator_api2::vec::Vec<T> = allocator_api2::vec::Vec::with_capacity(tail_count);
        // Pop produces reverse order; collect then reverse.
        for _ in 0..tail_count {
            tail_staging.push(self.buf.pop().expect("tail length matches len-start"));
        }
        tail_staging.reverse();
        // `tail_staging` now contains [start, len) in original order.
        // First `drained_count` go into `staged`; remainder go back.
        let mut iter = tail_staging.into_iter();
        for _ in 0..drained_count {
            staged.push(iter.next().expect("drained_count <= tail"));
        }
        // Remaining items go back into `self.buf`. We may need capacity,
        // but `self.buf` still has its existing capacity; pop didn't free
        // it.
        for item in iter {
            self.buf
                .push_within_cap(item)
                .ok()
                .expect("returning original tail to existing capacity");
        }

        Drain {
            staged: staged.into_iter(),
            _marker: PhantomData,
        }
    }
}

/// Draining iterator returned from [`Vec::drain`].
pub struct Drain<'d, 'a, T, A: Allocator + Clone> {
    staged: allocator_api2::vec::IntoIter<T>,
    _marker: PhantomData<&'d mut Vec<'a, T, A>>,
}

impl<T: fmt::Debug, A: Allocator + Clone> fmt::Debug for Drain<'_, '_, T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Drain").field("remaining", &self.staged.len()).finish()
    }
}

// `Drain` is `!Send`/`!Sync` for the same reason as `Vec`: it borrows a
// vector tied to one arena thread.

impl<T, A: Allocator + Clone> Iterator for Drain<'_, '_, T, A> {
    type Item = T;
    #[inline]
    fn next(&mut self) -> Option<T> {
        self.staged.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.staged.size_hint()
    }
}

impl<T, A: Allocator + Clone> DoubleEndedIterator for Drain<'_, '_, T, A> {
    #[inline]
    fn next_back(&mut self) -> Option<T> {
        self.staged.next_back()
    }
}

impl<T, A: Allocator + Clone> ExactSizeIterator for Drain<'_, '_, T, A> {}
impl<T, A: Allocator + Clone> FusedIterator for Drain<'_, '_, T, A> {}

impl<T, A: Allocator + Clone> Drop for Drain<'_, '_, T, A> {
    #[inline]
    #[cfg_attr(test, mutants::skip)] // body has no observable side effects
    fn drop(&mut self) {
        // Remaining staged items are dropped by `allocator_api2::vec::IntoIter`'s
        // own `Drop`. Hole has already been closed at construction time.
        let _ = &mut self.staged;
        let _ = mem::size_of::<T>();
    }
}
