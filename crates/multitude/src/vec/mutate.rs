// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-place mutation operations.

use core::ptr::{self, NonNull};

use allocator_api2::alloc::{AllocError, Allocator};
use allocator_api2::vec::Vec as ApiVec;

use super::Vec;

impl<T, A: Allocator + Clone> Vec<'_, T, A> {
    /// Insert `value` at position `idx`, shifting subsequent elements right.
    ///
    /// # Panics
    ///
    /// Panics if `idx > len`, or if the backing allocator fails on growth.
    #[cfg_attr(test, mutants::skip)] // Copy-count mutations are only distinguishable by UB tooling.
    pub fn insert(&mut self, idx: usize, value: T) {
        assert!(idx <= self.len, "insertion index out of bounds");
        if self.len == self.cap {
            self.grow_one();
        }
        // SAFETY: `idx <= len < cap`; shifting uses overlapping copy into initialized/uninitialized tail.
        unsafe {
            let ptr = self.data.as_ptr().add(idx);
            ptr::copy(ptr, ptr.add(1), self.len - idx);
            ptr.write(value);
        }
        self.len += 1;
    }

    /// Remove and return the element at position `idx`, shifting subsequent
    /// elements to the left.
    ///
    /// # Panics
    ///
    /// Panics if `idx >= len`.
    #[cfg_attr(test, mutants::skip)] // Copy-count mutations are only distinguishable by UB tooling.
    pub fn remove(&mut self, idx: usize) -> T {
        assert!(idx < self.len, "removal index out of bounds");
        // SAFETY: `idx < len`; read moves the element out, then overlapping copy shifts the initialized tail left.
        unsafe {
            let ptr = self.data.as_ptr().add(idx);
            let value = ptr.read();
            ptr::copy(ptr.add(1), ptr, self.len - idx - 1);
            self.len -= 1;
            value
        }
    }

    /// Swap-remove: O(1) but does not preserve order.
    ///
    /// # Panics
    ///
    /// Panics if `idx >= len`.
    pub fn swap_remove(&mut self, idx: usize) -> T {
        assert!(idx < self.len, "swap_remove index out of bounds");
        self.len -= 1;
        // SAFETY: `idx` and the old last element are initialized. The last is moved into the hole when needed.
        unsafe {
            let ptr = self.data.as_ptr().add(idx);
            let value = ptr.read();
            if idx != self.len {
                ptr::copy_nonoverlapping(self.data.as_ptr().add(self.len), ptr, 1);
            }
            value
        }
    }

    /// Shorten the vector to `new_len`, dropping the excess elements.
    pub fn truncate(&mut self, new_len: usize) {
        if new_len >= self.len {
            return;
        }
        let old_len = self.len;
        self.len = new_len;
        // SAFETY: elements in `new_len..old_len` were initialized and are no longer part of the vector.
        unsafe {
            let slice: *mut [T] = ptr::slice_from_raw_parts_mut(self.data.as_ptr().add(new_len), old_len - new_len);
            ptr::drop_in_place(slice);
        }
    }

    /// Force the length of the vector to `new_len`.
    ///
    /// # Safety
    ///
    /// `new_len` must be `<= self.capacity()` and the elements at
    /// `old_len..new_len` must be initialized.
    pub const unsafe fn set_len(&mut self, new_len: usize) {
        self.len = new_len;
    }

    /// Shrink the capacity of the vector as much as possible.
    ///
    /// Behaves like a no-op when the buffer doesn't end at the
    /// chunk's bump cursor: the arena cannot reclaim partial
    /// allocations, so an allocate-copy-deallocate "shrink" would
    /// only churn space. When the buffer **is** at the cursor, the
    /// excess capacity is returned to the cursor in O(1) and `cap`
    /// drops to `len`. The special case `len == 0` always releases
    /// the chunk reference outright.
    pub fn shrink_to_fit(&mut self) {
        // ZST capacity is meaningless, so shrinking is a no-op.
        if core::mem::size_of::<T>() == 0 {
            return;
        }
        if self.len == self.cap || self.cap == 0 {
            return;
        }
        // The shrink branch of `realloc` is infallible; keep the `expect`
        // so future refactors preserve that contract.
        self.realloc(self.len)
            .expect("Vec::shrink_to_fit: realloc on the shrink path never fails");
    }

    /// Retain only elements for which the predicate returns `true`.
    pub fn retain<F: FnMut(&T) -> bool>(&mut self, f: F) {
        self.with_apivec(|v| v.retain(f));
    }

    /// Retain (mutable predicate variant).
    pub fn retain_mut<F: FnMut(&mut T) -> bool>(&mut self, f: F) {
        self.with_apivec(|v| v.retain_mut(f));
    }

    /// Remove consecutive duplicates by `PartialEq`.
    pub fn dedup(&mut self)
    where
        T: PartialEq,
    {
        self.with_apivec(ApiVec::dedup);
    }

    /// Remove consecutive duplicates by `same_bucket`.
    pub fn dedup_by<F: FnMut(&mut T, &mut T) -> bool>(&mut self, same_bucket: F) {
        self.with_apivec(|v| v.dedup_by(same_bucket));
    }

    /// Remove consecutive duplicates by key.
    pub fn dedup_by_key<K, F>(&mut self, key: F)
    where
        F: FnMut(&mut T) -> K,
        K: PartialEq,
    {
        self.with_apivec(|v| v.dedup_by_key(key));
    }

    /// Move all elements of `other` into `self`, leaving `other` empty.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails on growth.
    pub fn append(&mut self, other: &mut Self) {
        let elem_size = core::mem::size_of::<T>();
        // Fast path: if `other` starts exactly where `self`'s allocation ends
        // and `self` has no spare tail (`len == cap`), both halves already sit
        // back-to-back in one chunk, so we can absorb `other` without copying.
        // The omitted `*_cap > 0` checks are implied by the existing guards.
        if elem_size != 0 && other.len != 0 && self.len == self.cap {
            // SAFETY: `data + cap` is a valid end pointer for a real
            // allocation, and `dangling.add(0)` is harmless for the empty case.
            let self_end = unsafe { self.data.as_ptr().add(self.cap) };
            if core::ptr::eq(self_end, other.data.as_ptr()) {
                // The initialized prefix is now the concatenation of both halves.
                self.len += other.len;
                self.cap += other.cap;
                // Release `other`'s chunk ref and zero its raw parts so drop is a no-op.
                Self::deallocate_buffer(self.arena, other.data, other.cap);
                other.data = NonNull::dangling();
                other.len = 0;
                other.cap = 0;
                return;
            }
        }
        // Fallback copy path.
        self.reserve(other.len);
        // SAFETY: destination has enough uninitialized tail capacity; source range is initialized and non-overlapping.
        unsafe { ptr::copy_nonoverlapping(other.data.as_ptr(), self.data.as_ptr().add(self.len), other.len) };
        self.len += other.len;
        other.len = 0;
    }

    /// Reserve the minimum capacity for at least `additional` more elements.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_reserve_exact`] for a fallible variant.
    pub fn reserve_exact(&mut self, additional: usize) {
        if self.try_reserve_exact(additional).is_err() {
            panic!("multitude: allocator returned AllocError");
        }
    }

    /// Fallible variant of [`Self::reserve_exact`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data
    /// alignment is at least 32 KiB.
    pub fn try_reserve_exact(&mut self, additional: usize) -> Result<(), AllocError> {
        let needed = self.len.checked_add(additional).ok_or(AllocError)?;
        if needed > self.cap {
            self.realloc(needed)?;
        }
        Ok(())
    }

    /// Resize the vector to `new_len`, cloning `value` to fill new slots.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails on growth.
    // EQUIVALENCE: `added > 0` to `>=` only changes an empty-slice drop.
    pub fn resize(&mut self, new_len: usize, value: T)
    where
        T: Clone,
    {
        if new_len <= self.len {
            self.truncate(new_len);
        } else {
            self.reserve(new_len - self.len);
            let old_len = self.len;
            // Roll back partially written elements on panic.
            struct Guard<'v, 'a, T, A: Allocator + Clone> {
                vec: &'v mut Vec<'a, T, A>,
                old_len: usize,
            }
            impl<T, A: Allocator + Clone> Drop for Guard<'_, '_, T, A> {
                // EQUIVALENCE: `added > 0` to `>=` only changes an empty-slice
                // drop, which is a no-op.
                #[cfg_attr(test, mutants::skip)]
                fn drop(&mut self) {
                    let added = self.vec.len - self.old_len;
                    if added > 0 {
                        let tail = unsafe { core::slice::from_raw_parts_mut(self.vec.data.as_ptr().add(self.old_len), added) };
                        unsafe { ptr::drop_in_place(tail) };
                    }
                    self.vec.len = self.old_len;
                }
            }
            let guard = Guard { vec: self, old_len };
            // Clone into all but the last new slot, then move `value` into the
            // last one to avoid one extra clone.
            while guard.vec.len < new_len - 1 {
                let val = value.clone();
                unsafe { guard.vec.data.as_ptr().add(guard.vec.len).write(val) };
                guard.vec.len += 1;
            }
            unsafe { guard.vec.data.as_ptr().add(guard.vec.len).write(value) };
            guard.vec.len += 1;
            core::mem::forget(guard);
        }
    }

    /// Resize the vector to `new_len`, calling `f` for new elements.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails on growth.
    pub fn resize_with<F: FnMut() -> T>(&mut self, new_len: usize, mut f: F) {
        if new_len <= self.len {
            self.truncate(new_len);
        } else {
            self.reserve(new_len - self.len);
            let old_len = self.len;
            struct Guard<'v, 'a, T, A: Allocator + Clone> {
                vec: &'v mut Vec<'a, T, A>,
                old_len: usize,
            }
            impl<T, A: Allocator + Clone> Drop for Guard<'_, '_, T, A> {
                // EQUIVALENCE: `added > 0` to `>=` only changes an empty-slice
                // drop, which is a no-op.
                #[cfg_attr(test, mutants::skip)]
                fn drop(&mut self) {
                    let added = self.vec.len - self.old_len;
                    if added > 0 {
                        let tail = unsafe { core::slice::from_raw_parts_mut(self.vec.data.as_ptr().add(self.old_len), added) };
                        unsafe { ptr::drop_in_place(tail) };
                    }
                    self.vec.len = self.old_len;
                }
            }
            let guard = Guard { vec: self, old_len };
            while guard.vec.len < new_len {
                let val = f();
                unsafe { guard.vec.data.as_ptr().add(guard.vec.len).write(val) };
                guard.vec.len += 1;
            }
            core::mem::forget(guard);
        }
    }

    /// Split the vector at `at`, returning a new vector containing `[at, len)`.
    ///
    /// # Panics
    ///
    /// Panics if `at > len`.
    #[must_use]
    pub fn split_off(&mut self, at: usize) -> Self {
        assert!(at <= self.len, "split index out of bounds");
        // `at == 0` transfers the whole allocation. Handle it up front so we
        // do not strand the chunk ref by later setting `cap = 0` in place.
        if at == 0 {
            return core::mem::replace(self, Self::new_in(self.arena));
        }
        let tail_len = self.len - at;
        let elem_size = core::mem::size_of::<T>();

        // Fall back to allocate-and-copy when there is no real shared buffer
        // to split or the tail is empty. In these cases the copy count is 0,
        // so even dangling pointers are fine.
        if elem_size == 0 || self.cap == 0 || tail_len == 0 {
            let other = Self::with_capacity_in(tail_len, self.arena);
            // SAFETY: this copy moves 0 bytes in every reachable case.
            unsafe { ptr::copy_nonoverlapping(self.data.as_ptr().add(at), other.data.as_ptr(), tail_len) };
            // `with_capacity_in(tail_len)` gives enough capacity, and the copy
            // above already initialized the tail elements when needed.
            let mut other = other;
            other.len = tail_len;
            self.len = at;
            return other;
        }

        // In-place split: take a second chunk ref and point the tail half into
        // the same allocation so both halves can later drop independently.
        // SAFETY: `self.cap > 0` implies `self.data` was returned by
        // an arena allocation and the chunk still holds at least our
        // `+1`, satisfying `inc_ref_for_buffer`'s preconditions.
        unsafe { self.arena.inc_ref_for_buffer(self.data.cast::<u8>()) };

        // SAFETY: `at <= self.len <= self.cap`, so `data + at` stays in-bounds.
        let tail_data = unsafe { NonNull::new_unchecked(self.data.as_ptr().add(at)) };

        let other = Self {
            arena: self.arena,
            data: tail_data,
            len: tail_len,
            cap: self.cap - at,
        };
        // Cap the head at `at` so later growth cannot write into the tail's region.
        self.cap = at;
        self.len = at;
        other
    }

    /// Pop the last element if the predicate returns `true`.
    pub fn pop_if<F: FnOnce(&mut T) -> bool>(&mut self, predicate: F) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        // SAFETY: len > 0, so the last element is initialized and uniquely borrowed through `&mut self`.
        let last = unsafe { &mut *self.data.as_ptr().add(self.len - 1) };
        if predicate(last) { self.pop() } else { None }
    }
}
