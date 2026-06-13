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
            Bound::Excluded(&i) => i.checked_add(1).expect("drain: start bound overflows usize"),
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&i) => i.checked_add(1).expect("drain: end bound overflows usize"),
            Bound::Excluded(&i) => i,
            Bound::Unbounded => len,
        };
        assert!(start <= end, "drain: start > end");
        assert!(end <= len, "drain: end > len");

        // Eager hole-closing: pop the whole tail `[start, len)`, keep the
        // drained prefix in a heap-backed staging vec, and push the surviving
        // suffix back — all at construction time. So `Drain`'s `Drop` is a
        // no-op and even a forgotten/leaked `Drain` leaves `self.buf`
        // consistent. A `std`-allocated staging vec avoids arena chunk churn.
        let drained_count = end - start;
        let mut staged: allocator_api2::vec::Vec<T> = allocator_api2::vec::Vec::with_capacity(drained_count);
        // `T` isn't `Default`, so we can't punch a hole in place: pop the
        // whole tail after `start`, then push the kept suffix back.
        let tail_count = len - start;
        let mut tail_staging: allocator_api2::vec::Vec<T> = allocator_api2::vec::Vec::with_capacity(tail_count);
        // Pop produces reverse order; collect then reverse.
        for _ in 0..tail_count {
            tail_staging.push(self.buf.pop().expect("tail length matches len-start"));
        }
        tail_staging.reverse();
        let mut iter = tail_staging.into_iter();
        for _ in 0..drained_count {
            staged.push(iter.next().expect("drained_count <= tail"));
        }
        // Surviving suffix goes back; pop didn't free capacity.
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
