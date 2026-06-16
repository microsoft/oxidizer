// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `Vec::drain` and its iterator.

use core::iter::FusedIterator;
use core::marker::PhantomData;
use core::ops::{Bound, RangeBounds};
use core::ptr::{self, NonNull};
use core::{fmt, mem, slice};

use allocator_api2::alloc::Allocator;

use super::Vec;

impl<'a, T, A: Allocator + Clone> Vec<'a, T, A> {
    /// Remove the elements in `range` and return an iterator over them.
    ///
    /// Mirrors [`std::vec::Vec::drain`], including its **lazy** semantics: the
    /// removal is finalized when the returned [`Drain`] is dropped (the
    /// surviving tail is shifted into the gap then). The work happens entirely
    /// within the vector's own arena storage — there is no temporary heap
    /// allocation. As with `std`, `mem::forget`-ing the [`Drain`] leaks the
    /// un-restored tail.
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

        // SAFETY: `start <= end <= len`. Lowering the length to `start` moves
        // the drained range and the tail out of the vector's view so a leaked
        // `Drain` leaks them instead of double-dropping; the tail is restored
        // in `Drain::drop`. `[start, end)` is initialized, so the shared slice
        // and `vec` raw pointer are valid.
        unsafe {
            self.buf.set_len(start);
            let drained = slice::from_raw_parts(self.buf.as_ptr().add(start), end - start);
            Drain {
                tail_start: end,
                tail_len: len - end,
                iter: drained.iter(),
                vec: NonNull::from(self),
                _marker: PhantomData,
            }
        }
    }
}

/// Draining iterator returned from [`Vec::drain`].
///
/// Yields the removed elements (double-ended); the surviving tail is restored
/// in place on drop.
pub struct Drain<'d, 'a, T, A: Allocator + Clone> {
    /// Index where the surviving tail begins in the source vector.
    tail_start: usize,
    /// Number of surviving tail elements to restore on drop.
    tail_len: usize,
    /// Iterator over the not-yet-yielded drained elements `[start, end)`.
    iter: slice::Iter<'d, T>,
    /// Source vector. Its length was lowered to the drain start, so these
    /// fields own the drained range and the tail until drop.
    vec: NonNull<Vec<'a, T, A>>,
    /// Ties `Drain`'s variance and auto-traits to the exclusive `&'d mut Vec`
    /// borrow that produced it. Without this marker the `'d` lifetime would
    /// only appear behind the covariant `iter`/`vec` fields, leaving the borrow
    /// relationship implicit. The marker makes `Drain` invariant in the
    /// borrowed `Vec` and inherit its `!Send`/`!Sync` (multitude's `Vec` is
    /// thread-affine), so a live `Drain` can never migrate to another thread
    /// and restore the tail from there in `Drop`.
    _marker: PhantomData<&'d mut Vec<'a, T, A>>,
}

impl<T: fmt::Debug, A: Allocator + Clone> fmt::Debug for Drain<'_, '_, T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Drain").field("remaining", &self.iter.len()).finish()
    }
}

impl<T, A: Allocator + Clone> Iterator for Drain<'_, '_, T, A> {
    type Item = T;
    #[inline]
    fn next(&mut self) -> Option<T> {
        // SAFETY: each drained slot is yielded at most once; `ptr::read` moves
        // the element out and the slot is never read again.
        self.iter.next().map(|e| unsafe { ptr::read(e) })
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<T, A: Allocator + Clone> DoubleEndedIterator for Drain<'_, '_, T, A> {
    #[inline]
    fn next_back(&mut self) -> Option<T> {
        // SAFETY: see `next`.
        self.iter.next_back().map(|e| unsafe { ptr::read(e) })
    }
}

impl<T, A: Allocator + Clone> ExactSizeIterator for Drain<'_, '_, T, A> {}
impl<T, A: Allocator + Clone> FusedIterator for Drain<'_, '_, T, A> {}

impl<'a, T, A: Allocator + Clone> Drain<'_, 'a, T, A> {
    /// Move the surviving tail down to close the gap left by the drained range,
    /// restoring a contiguous vector, and disarm this `Drain`'s own tail
    /// restoration. Returns the source vector and the index where the tail now
    /// begins (the [`Splice`](super::Splice) insertion point).
    ///
    /// # Safety
    ///
    /// All drained elements must already have been moved out (the iterator
    /// fully consumed); otherwise they are leaked.
    pub(super) unsafe fn close_tail(&mut self) -> (NonNull<Vec<'a, T, A>>, usize) {
        // SAFETY: `self.vec` is the live source vector whose length is the
        // drain start; `[tail_start, tail_start + tail_len)` is initialized.
        // Both copy endpoints are derived from one `as_mut_ptr` (write
        // provenance over the whole buffer), and `start <= tail_start` keeps
        // the move within bounds.
        unsafe {
            let v = self.vec.as_mut();
            let start = v.buf.len();
            if self.tail_len > 0 && self.tail_start != start {
                let base = v.as_mut_ptr();
                ptr::copy(base.add(self.tail_start).cast_const(), base.add(start), self.tail_len);
            }
            v.buf.set_len(start + self.tail_len);
            self.tail_len = 0;
            self.tail_start = start;
            (self.vec, start)
        }
    }
}

/// Restores the surviving tail even if dropping the remaining drained elements
/// panics.
struct TailGuard<'r, 'd, 'a, T, A: Allocator + Clone>(&'r mut Drain<'d, 'a, T, A>);

impl<T, A: Allocator + Clone> Drop for TailGuard<'_, '_, '_, T, A> {
    fn drop(&mut self) {
        let d = &mut *self.0;
        // SAFETY: source vector is live; `start = vec.len` is the drain start;
        // `[tail_start, +tail_len)` is initialized; both copy endpoints share
        // one `as_mut_ptr` (write provenance) and the move stays in bounds.
        // `tail_len == 0` makes the copy and `set_len` no-ops.
        unsafe {
            let v = d.vec.as_mut();
            let start = v.buf.len();
            if d.tail_start != start {
                let base = v.as_mut_ptr();
                ptr::copy(base.add(d.tail_start).cast_const(), base.add(start), d.tail_len);
            }
            v.buf.set_len(start + d.tail_len);
        }
    }
}

impl<T, A: Allocator + Clone> Drop for Drain<'_, '_, T, A> {
    fn drop(&mut self) {
        // Detach the remaining drained elements so the guard can take `&mut
        // self`; the guard restores the tail even if a destructor panics.
        let iter = mem::take(&mut self.iter);
        let drop_ptr = iter.as_slice().as_ptr();
        let drop_len = iter.len();
        let mut vec = self.vec;
        let _guard = TailGuard(self);
        if drop_len > 0 {
            // SAFETY: re-derive a write-provenance pointer to the remaining
            // drained elements from the vector itself (`drop_ptr` only supplies
            // the offset within the same buffer), and drop each exactly once.
            unsafe {
                let v = vec.as_mut();
                let offset = drop_ptr.offset_from_unsigned(v.as_ptr());
                ptr::drop_in_place(ptr::slice_from_raw_parts_mut(v.as_mut_ptr().add(offset), drop_len));
            }
        }
    }
}
