// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
//! In-place mutation operations.

use core::mem;

use allocator_api2::alloc::{AllocError, Allocator};

use super::Vec;
use crate::internal::arena_buf::ArenaBuf;

/// Rollback guard for `resize`/`resize_with`: if a user `clone` or
/// closure panics partway through a grow, the guard's `Drop` truncates
/// the buffer back to `old_len`, dropping every element written so far.
/// On the success path the caller disarms it via [`mem::forget`].
struct ResizeGuard<'b, 'a, T> {
    buf: &'b mut ArenaBuf<'a, T>,
    old_len: usize,
}

impl<T> Drop for ResizeGuard<'_, '_, T> {
    #[inline]
    fn drop(&mut self) {
        self.buf.truncate(self.old_len);
    }
}

impl<T, A: Allocator + Clone> Vec<'_, T, A> {
    /// Insert `value` at position `idx`, shifting subsequent elements right.
    ///
    /// # Panics
    ///
    /// Panics if `idx > len`, or if the backing allocator fails on growth.
    pub fn insert(&mut self, idx: usize, value: T) {
        let len = self.buf.len();
        assert!(idx <= len, "insertion index (is {idx}) should be <= len (is {len})");
        if self.buf.remaining_cap() == 0 && self.try_reserve(1).is_err() {
            crate::arena::panic_alloc!();
        }
        self.buf.insert_within_cap(idx, value);
    }

    /// Remove and return the element at position `idx`, shifting subsequent
    /// elements to the left.
    ///
    /// # Panics
    ///
    /// Panics if `idx >= len`.
    pub fn remove(&mut self, idx: usize) -> T {
        let len = self.buf.len();
        match self.buf.remove(idx) {
            Some(v) => v,
            None => panic!("removal index (is {idx}) should be < len (is {len})"),
        }
    }

    /// Swap-remove: O(1) but does not preserve order.
    ///
    /// # Panics
    ///
    /// Panics if `idx >= len`.
    pub fn swap_remove(&mut self, idx: usize) -> T {
        let len = self.buf.len();
        match self.buf.swap_remove(idx) {
            Some(v) => v,
            None => panic!("swap_remove index (is {idx}) should be < len (is {len})"),
        }
    }

    /// Shorten the vector to `new_len`, dropping the excess elements.
    #[inline]
    pub fn truncate(&mut self, new_len: usize) {
        self.buf.truncate(new_len);
    }

    /// Force the length of the vector to `new_len`.
    ///
    /// # Safety
    ///
    /// `new_len` must be `<= self.capacity()` and the elements at
    /// `old_len..new_len` must be initialized.
    #[inline]
    pub const unsafe fn set_len(&mut self, new_len: usize) {
        // SAFETY: forwarded to ArenaBuf::set_len; the caller's safety
        // obligations match.
        unsafe { self.buf.set_len(new_len) }
    }

    /// Shrink the capacity of the vector as much as possible.
    ///
    /// O(1) reclamation when the buffer sits at the current bump cursor
    /// of its chunk (no later allocation has moved the cursor past it):
    /// the unused tail is returned to the chunk and the data pointer is
    /// unchanged. Otherwise this is a no-op — the arena never relocates
    /// or copies to shrink, so capacity simply stays put.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // thin delegation; logic covered via `reclaim_capacity_tail`
    pub fn shrink_to_fit(&mut self) {
        self.shrink_to(0);
    }

    /// Shrink the capacity with a lower bound.
    ///
    /// The capacity will remain at least as large as both `self.len()` and
    /// `min_capacity`. Reclamation only succeeds while the buffer still sits
    /// at the chunk's bump cursor; otherwise this is a no-op (matching
    /// [`std::vec::Vec::shrink_to`]'s "best-effort" contract).
    #[cfg_attr(test, mutants::skip)]
    pub fn shrink_to(&mut self, min_capacity: usize) {
        if const { mem::size_of::<T>() == 0 } {
            return;
        }
        let target = self.buf.len().max(min_capacity);
        let _ = self.reclaim_capacity_tail(target);
    }

    /// Reclaim the capacity tail `[target_cap, cap)` back to the chunk's
    /// bump cursor when this buffer is still the chunk's last allocation
    /// (an O(1) cursor rewind — no copy, data pointer unchanged). Returns
    /// whether storage was reclaimed. A no-op when the buffer has been
    /// overtaken by a later allocation, sits in a retired or oversized
    /// chunk, or is a ZST.
    ///
    /// Callers must ensure the slots in `[target_cap, cap)` hold no live
    /// element (either never initialized, or already dropped): the
    /// reclaimed bytes return to the arena and may be overwritten by the
    /// next allocation.
    #[inline]
    // Mutation testing is suppressed on the `total_bytes > max_normal_alloc`
    // early-return: `>` with `==` / `>=` mutations only differ at the exact
    // boundary `total_bytes == max_normal_alloc`. At that boundary, the
    // Vec's `refill_hint` (which adds `align_of::<T>()`) exceeds
    // `max_normal_alloc`, so the Vec is allocated via the oversized path
    // and `try_reclaim_tail` returns `false` regardless of this check.
    // The check exists as a cheap pre-filter rather than a load-bearing
    // correctness gate.
    #[cfg_attr(test, mutants::skip)]
    pub(super) fn reclaim_capacity_tail(&mut self, target_cap: usize) -> bool {
        if const { mem::size_of::<T>() == 0 } {
            return false;
        }
        let cap = self.buf.cap();
        if cap <= target_cap {
            return false;
        }
        let elem = mem::size_of::<T>();
        let data_addr = self.buf.as_ptr() as usize;
        // One-past-the-end address of the current allocation. The product
        // is the buffer's real byte size, bounded by its chunk, so it
        // cannot overflow.
        let total_bytes = cap * elem;
        // Buffers large enough to have been served by an oversized chunk
        // are never at the `current_local` bump cursor; skip them so the
        // cheap cursor check below never spuriously reclaims a one-shot
        // chunk's storage.
        if total_bytes > self.arena.max_normal_alloc() {
            return false;
        }
        let end_addr = data_addr + total_bytes;
        let reclaim_bytes = (cap - target_cap) * elem;
        if self.arena.current_local().try_reclaim_tail(end_addr, reclaim_bytes) {
            // SAFETY: the chunk reclaimed `[target_cap*elem, cap*elem)`, so
            // this buffer no longer owns that span; the retained prefix
            // `[0, target_cap)` is untouched and the caller guarantees no
            // live element sits in the reclaimed range.
            unsafe { self.buf.set_cap(target_cap) };
            true
        } else {
            false
        }
    }

    /// Clone the elements in `src` and append them to the end.
    ///
    /// `src` is an index range into `self`. Mirrors
    /// [`std::vec::Vec::extend_from_within`].
    ///
    /// # Panics
    ///
    /// Panics if the range is out of bounds, or if the backing allocator
    /// fails while reserving.
    pub fn extend_from_within<R: core::ops::RangeBounds<usize>>(&mut self, src: R)
    where
        T: Clone,
    {
        let len = self.buf.len();
        let start = match src.start_bound() {
            core::ops::Bound::Included(&n) => n,
            core::ops::Bound::Excluded(&n) => n.checked_add(1).expect("extend_from_within: start bound overflows usize"),
            core::ops::Bound::Unbounded => 0,
        };
        let end = match src.end_bound() {
            core::ops::Bound::Included(&n) => n.checked_add(1).expect("extend_from_within: end bound overflows usize"),
            core::ops::Bound::Excluded(&n) => n,
            core::ops::Bound::Unbounded => len,
        };
        assert!(start <= end, "extend_from_within: start > end");
        assert!(end <= len, "extend_from_within: range end out of bounds");
        let count = end - start;
        // Reserve up front so the subsequent pushes cannot relocate the
        // buffer (which would invalidate the source indices we read from).
        self.reserve(count);
        for i in start..end {
            let cloned = self.buf.as_slice()[i].clone();
            self.push(cloned);
        }
    }

    /// Retain only elements for which the predicate returns `true`.
    pub fn retain<F: FnMut(&T) -> bool>(&mut self, mut f: F) {
        self.retain_mut(|t| f(t));
    }

    /// Retain (mutable predicate variant).
    pub fn retain_mut<F: FnMut(&mut T) -> bool>(&mut self, mut f: F) {
        let mut write = 0;
        let len = self.buf.len();
        let slice = self.buf.as_mut_slice();
        // Compact kept elements toward the front via swaps, then drop the tail.
        for read in 0..len {
            let keep = f(&mut slice[read]);
            if keep {
                if write != read {
                    slice.swap(write, read);
                }
                write += 1;
            }
        }
        self.buf.truncate(write);
    }

    /// Remove consecutive duplicates by `PartialEq`.
    pub fn dedup(&mut self)
    where
        T: PartialEq,
    {
        self.dedup_by(|a, b| a == b);
    }

    /// Remove consecutive duplicates by `same_bucket`.
    pub fn dedup_by<F: FnMut(&mut T, &mut T) -> bool>(&mut self, mut same_bucket: F) {
        let len = self.buf.len();
        if len < 2 {
            return;
        }
        let slice = self.buf.as_mut_slice();
        let mut write = 1;
        for read in 1..len {
            // Split so we can hold a `&mut` of the previous-kept element
            // (`prev`) and the candidate (`cur`) simultaneously.
            let (left, right) = slice.split_at_mut(read);
            let prev = &mut left[write - 1];
            let cur = &mut right[0];
            if !same_bucket(cur, prev) {
                if write != read {
                    slice.swap(write, read);
                }
                write += 1;
            }
        }
        self.buf.truncate(write);
    }

    /// Remove consecutive duplicates by key.
    pub fn dedup_by_key<K, F>(&mut self, mut key: F)
    where
        F: FnMut(&mut T) -> K,
        K: PartialEq,
    {
        self.dedup_by(|a, b| key(a) == key(b));
    }

    /// Move all elements of `other` into `self`, leaving `other` empty.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails on growth.
    pub fn append(&mut self, other: &mut Self) {
        let add = other.buf.len();
        if add == 0 {
            return;
        }
        // Zero-copy fast path: when `other`'s storage directly abuts the
        // end of a full `self`, absorb it instead of copying elements.
        if const { mem::size_of::<T>() != 0 } && self.buf.try_absorb_adjacent(&mut other.buf) {
            return;
        }
        self.reserve(add);
        for item in other.buf.drain_all() {
            self.buf.push_within_cap(item).ok().expect("capacity reserved above");
        }
    }

    /// Reserve the minimum capacity for at least `additional` more elements.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_reserve_exact`] for a fallible variant.
    #[inline]
    pub fn reserve_exact(&mut self, additional: usize) {
        // No tighter guarantee than `reserve`: the arena's slice
        // reservation policy already returns the requested capacity.
        self.reserve(additional);
    }

    /// Fallible variant of [`Self::reserve_exact`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data
    /// alignment is at least 32 KiB.
    #[inline]
    pub fn try_reserve_exact(&mut self, additional: usize) -> Result<(), AllocError> {
        self.try_reserve(additional)
    }

    /// Resize the vector to `new_len`, cloning `value` to fill new slots.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails on growth.
    pub fn resize(&mut self, new_len: usize, value: T)
    where
        T: Clone,
    {
        let len = self.buf.len();
        if new_len <= len {
            self.buf.truncate(new_len);
            return;
        }
        let added = new_len - len;
        self.reserve(added);
        // If a `clone` (or the final move) panics partway through, the
        // guard rolls the length back to `len`, dropping every element
        // written so far. This keeps the vector in a consistent state and
        // never leaks the partially-grown tail.
        let guard = ResizeGuard {
            buf: &mut self.buf,
            old_len: len,
        };
        for _ in 0..(added - 1) {
            guard.buf.push_within_cap(value.clone()).ok().expect("capacity reserved above");
        }
        // Last push consumes the original `value` to avoid an extra clone.
        guard.buf.push_within_cap(value).ok().expect("capacity reserved above");
        mem::forget(guard);
    }

    /// Resize the vector to `new_len`, calling `f` for new elements.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails on growth.
    pub fn resize_with<F: FnMut() -> T>(&mut self, new_len: usize, mut f: F) {
        let len = self.buf.len();
        if new_len <= len {
            self.buf.truncate(new_len);
            return;
        }
        let added = new_len - len;
        self.reserve(added);
        // See `resize`: roll back on a panic in `f` so the elements
        // written before the panic are dropped and the length is restored.
        let guard = ResizeGuard {
            buf: &mut self.buf,
            old_len: len,
        };
        for _ in 0..added {
            guard.buf.push_within_cap(f()).ok().expect("capacity reserved above");
        }
        mem::forget(guard);
    }

    /// Split the vector at `at`, returning a new vector containing `[at, len)`.
    ///
    /// # Panics
    ///
    /// Panics if `at > len`.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // routing mutations produce externally indistinguishable empty tails
    pub fn split_off(&mut self, at: usize) -> Self {
        let len = self.buf.len();
        assert!(at <= len, "split index out of bounds (at is {at}, len is {len})");
        let tail_len = len - at;
        // Copy/empty path for ZSTs, an unallocated head, or an empty
        // tail: produce an independent tail and leave the head's storage
        // (and capacity) intact.
        if const { mem::size_of::<T>() == 0 } || self.buf.cap() == 0 || tail_len == 0 {
            let mut tail = Self::with_capacity_in(tail_len, self.arena);
            // Move the `[at, len)` suffix into `tail`, preserving order:
            // pop into a staging buffer (reverse order) then push back.
            let mut staging = allocator_api2::vec::Vec::with_capacity(tail_len);
            for _ in 0..tail_len {
                staging.push(self.buf.pop().expect("tail length matches"));
            }
            while let Some(v) = staging.pop() {
                tail.buf.push_within_cap(v).ok().expect("capacity reserved above");
            }
            return tail;
        }
        // Zero-copy split: the tail shares the same chunk storage as the
        // head (storage is reclaimed only at arena teardown, which
        // outlives both halves), so no elements are copied.
        let tail_buf = self.buf.split_off_buf(at);
        Self::from_buf(tail_buf, self.arena)
    }

    /// Pop the last element if the predicate returns `true`.
    pub fn pop_if<F: FnOnce(&mut T) -> bool>(&mut self, predicate: F) -> Option<T> {
        let slice = self.buf.as_mut_slice();
        let last = slice.last_mut()?;
        if predicate(last) { self.buf.pop() } else { None }
    }
}

impl<'a, T, A: Allocator + Clone, const N: usize> Vec<'a, [T; N], A> {
    /// Flatten a `Vec<[T; N]>` into a `Vec<T>` in place (no copy). Mirrors
    /// [`std::vec::Vec::into_flattened`].
    ///
    /// # Panics
    ///
    /// Panics on the (practically unreachable) `len * N` / `cap * N` overflow.
    #[must_use]
    pub fn into_flattened(self) -> Vec<'a, T, A> {
        let arena = self.arena;
        let mut me = core::mem::ManuallyDrop::new(self);
        let len = me.buf.len();
        let cap = me.buf.cap();
        let ptr = me.buf.as_mut_ptr().cast::<T>();
        let new_len = len.checked_mul(N).expect("Vec::into_flattened: length overflow");
        let new_cap = if mem::size_of::<T>() == 0 {
            usize::MAX
        } else {
            cap.checked_mul(N).expect("Vec::into_flattened: capacity overflow")
        };
        // SAFETY: `[T; N]` and a run of `N` `T`s share layout and alignment,
        // so the buffer holds `len * N` initialized `T`s within `cap * N`
        // slots of the same arena chunk that outlives `'a`. `ptr` is non-null
        // (it came from a `NonNull` buffer base) and `T`-aligned (alignment of
        // `[T; N]` equals that of `T`). `ManuallyDrop` keeps the source buffer
        // and its elements from being dropped here; ownership of the elements
        // transfers wholesale to the returned `Vec<T>`.
        let buf = unsafe { ArenaBuf::from_raw_parts(core::ptr::NonNull::new_unchecked(ptr), new_len, new_cap) };
        Vec::from_buf(buf, arena)
    }
}
