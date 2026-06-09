// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
//! In-place mutation operations.

use core::mem;

use allocator_api2::alloc::{AllocError, Allocator};

use super::Vec;
use crate::internal::arena_buf::ArenaBuf;

/// Reclaim-byte arithmetic extracted from [`Vec::shrink_to_fit`].
#[inline]
#[cfg_attr(test, mutants::skip)] // arithmetic not observable via public API
fn shrink_reclaim_bytes(cap: usize, len: usize, elem: usize) -> usize {
    (cap - len) * elem
}

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
    // Mutation testing is suppressed on the `total_bytes > max_normal_alloc`
    // early-return: `>` with `==` / `>=` mutations only differ at the exact
    // boundary `total_bytes == max_normal_alloc`. At that boundary, the
    // Vec's `refill_hint` (which adds `align_of::<T>()`) exceeds
    // `max_normal_alloc`, so the Vec is allocated via the oversized path
    // and `try_reclaim_tail` returns `false` regardless of this check.
    // The check exists as a cheap pre-filter rather than a load-bearing
    // correctness gate.
    #[cfg_attr(test, mutants::skip)]
    pub fn shrink_to_fit(&mut self) {
        if const { mem::size_of::<T>() == 0 } {
            return;
        }
        let len = self.buf.len();
        let cap = self.buf.cap();
        if cap == len {
            return;
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
            return;
        }
        let end_addr = data_addr + total_bytes;
        let reclaim_bytes = shrink_reclaim_bytes(cap, len, elem);
        if self.arena.current_local().try_reclaim_tail(end_addr, reclaim_bytes) {
            // SAFETY: the chunk reclaimed `[len*elem, cap*elem)`, so this
            // buffer no longer owns that span; the live prefix `[0, len)`
            // is untouched and still initialized, and `len <= len`.
            unsafe { self.buf.set_cap(len) }
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
        // Walk the live elements; for each kept element, swap it down into
        // the write cursor. Dropped elements need to be removed; we record
        // the surviving prefix and then truncate.
        for read in 0..len {
            // SAFETY-FREE: rely on indexing; slice covers `[..len]`.
            // We need to mutate-in-place: borrow each element fresh.
            // To avoid borrow conflicts we work with raw indexing twice.
            let keep = f(&mut slice[read]);
            if keep {
                if write != read {
                    slice.swap(write, read);
                }
                write += 1;
            }
        }
        // After compaction, drop the tail.
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
        // Drain owns the elements; push them in order.
        for item in other.buf.drain_all() {
            // Capacity was reserved above.
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
