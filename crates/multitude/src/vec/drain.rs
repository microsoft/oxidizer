// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `Vec::drain` and its iterator.

use core::ops::{Bound, RangeBounds};
use core::ptr::{self, NonNull};
use core::{fmt, slice};

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
        let len = self.len;
        let start = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n.checked_add(1).expect("excluded drain start bound overflowed usize"),
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&n) => n.checked_add(1).expect("included drain end bound overflowed usize"),
            Bound::Excluded(&n) => n,
            Bound::Unbounded => len,
        };
        assert!(start <= end, "drain range start exceeds end");
        assert!(end <= len, "drain range end out of bounds");
        self.len = start;
        Drain {
            vec: NonNull::from(self),
            drain_start: start,
            drain_end: end,
            front: start,
            back: end,
            original_len: len,
            _marker: core::marker::PhantomData,
        }
    }
}

/// Draining iterator returned from [`Vec::drain`].
pub struct Drain<'d, 'a, T, A: Allocator + Clone> {
    vec: NonNull<Vec<'a, T, A>>,
    drain_start: usize,
    drain_end: usize,
    front: usize,
    back: usize,
    original_len: usize,
    _marker: core::marker::PhantomData<&'d mut Vec<'a, T, A>>,
}

impl<T: fmt::Debug, A: Allocator + Clone> fmt::Debug for Drain<'_, '_, T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: `front..back` is the not-yet-yielded initialized part of the drained range.
        let remaining = unsafe {
            let vec = self.vec.as_ref();
            slice::from_raw_parts(vec.data.as_ptr().add(self.front), self.back - self.front)
        };
        f.debug_struct("Drain").field("remaining", &remaining).finish()
    }
}

// `Drain` is `!Send`/`!Sync` for the same reason as `Vec`: it borrows a
// vector tied to one arena thread.

impl<T, A: Allocator + Clone> Iterator for Drain<'_, '_, T, A> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.front == self.back {
            return None;
        }
        // SAFETY: `front < back` points to an initialized, not-yet-yielded drained element.
        let value = unsafe { self.vec.as_ref().data.as_ptr().add(self.front).read() };
        self.front = self
            .front
            .checked_add(1)
            .expect("front < back ensures increment stays within the drained range");
        Some(value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.back - self.front;
        (len, Some(len))
    }
}

impl<T, A: Allocator + Clone> DoubleEndedIterator for Drain<'_, '_, T, A> {
    fn next_back(&mut self) -> Option<T> {
        if self.front == self.back {
            return None;
        }
        self.back -= 1;
        // SAFETY: the decremented `back` points to an initialized, not-yet-yielded drained element.
        Some(unsafe { self.vec.as_ref().data.as_ptr().add(self.back).read() })
    }
}

impl<T, A: Allocator + Clone> ExactSizeIterator for Drain<'_, '_, T, A> {}
impl<T, A: Allocator + Clone> core::iter::FusedIterator for Drain<'_, '_, T, A> {}

impl<T, A: Allocator + Clone> Drop for Drain<'_, '_, T, A> {
    // EQUIVALENCE: `remaining > 0` to `>=` is the same because dropping an
    // empty slice is a no-op.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // The tail shift and `vec.len` fix-up must still run if dropping a
        // drained element panics, or the tail becomes unreachable and leaks.
        struct TailFix<'x, 'd, 'a, T, A: Allocator + Clone> {
            d: &'x mut Drain<'d, 'a, T, A>,
        }
        impl<T, A: Allocator + Clone> Drop for TailFix<'_, '_, '_, T, A> {
            // EQUIVALENCE: `tail_len > 0` to `>=` only changes a zero-count
            // `ptr::copy`, which is a no-op.
            #[cfg_attr(test, mutants::skip)]
            fn drop(&mut self) {
                // SAFETY: `vec` is exclusively borrowed for the drain's lifetime.
                let vec = unsafe { self.d.vec.as_mut() };
                let tail_len = self.d.original_len - self.d.drain_end;
                if tail_len > 0 {
                    // SAFETY: source tail is initialized and may overlap with the destination hole.
                    unsafe {
                        ptr::copy(
                            vec.data.as_ptr().add(self.d.drain_end),
                            vec.data.as_ptr().add(self.d.drain_start),
                            tail_len,
                        );
                    }
                }
                vec.len = self.d.drain_start + tail_len;
            }
        }
        let g = TailFix { d: self };
        let remaining = g.d.back - g.d.front;
        if remaining > 0 {
            // SAFETY: `front..back` is the remaining initialized drained range.
            // Drop the slice at once so panic behavior matches `std::vec::Drain`.
            unsafe {
                ptr::drop_in_place(ptr::slice_from_raw_parts_mut(
                    g.d.vec.as_ref().data.as_ptr().add(g.d.front),
                    remaining,
                ));
            }
        }
        // `g.drop()` always runs the tail shift.
    }
}
