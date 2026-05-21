// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Low-level buffer management and the `ApiVec` bridge used by slow
//! paths such as `retain` and `dedup`.

use core::mem::ManuallyDrop;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::{AllocError, Allocator, Layout};
use allocator_api2::vec::Vec as ApiVec;

use super::Vec;
use crate::Arena;

impl<'a, T, A: Allocator + Clone> Vec<'a, T, A> {
    #[cold]
    #[inline(never)]
    pub(super) fn try_grow_amortized(&mut self, additional: usize) -> Result<(), AllocError> {
        let needed = self.len.checked_add(additional).ok_or(AllocError)?;
        if needed <= self.cap {
            return Ok(());
        }
        let doubled = self.cap.checked_mul(2).unwrap_or(usize::MAX);
        let new_cap = core::cmp::max(needed, core::cmp::max(doubled, 4));
        self.realloc(new_cap)
    }

    // EQUIVALENCE: `new_cap > self.cap` is already enforced by the
    // `debug_assert!`; `self.cap > 0` still falls back to allocate-copy when
    // the source is dangling; `self.len > 0` only changes a zero-count copy.
    #[cfg_attr(test, mutants::skip)]
    pub(super) fn realloc(&mut self, new_cap: usize) -> Result<(), AllocError> {
        // Safety precondition: copying `self.len` elements requires
        // `new_cap >= self.len`. Assert it so future callers cannot turn this
        // into UB.
        assert!(new_cap >= self.len, "Vec::realloc: new_cap < len would write past allocation");
        debug_assert!(new_cap != self.cap, "Vec::realloc: callers must ensure new_cap != cap");
        let elem_size = core::mem::size_of::<T>();
        if elem_size == 0 {
            self.cap = new_cap;
            self.data = NonNull::dangling();
            return Ok(());
        }
        if new_cap == 0 {
            Self::deallocate_buffer(self.arena, self.data, self.cap);
            self.data = NonNull::dangling();
            self.cap = 0;
            return Ok(());
        }
        // Shrink only reclaims cursor-adjacent tail space. Otherwise leave the
        // buffer alone; allocate-copy-deallocate would just waste arena space.
        if new_cap < self.cap {
            let reclaim_bytes = (self.cap - new_cap) * elem_size;
            // SAFETY: `self.cap > 0` (we returned for new_cap == 0
            // above and new_cap < self.cap implies self.cap > 0), so
            // `data + self.cap * elem_size` is one-past-the-end of
            // the buffer, within the chunk payload.
            let buffer_end = unsafe { self.data.as_ptr().cast::<u8>().add(self.cap * elem_size) };
            // SAFETY: `buffer_end` is `reclaim_bytes` past
            // `data + new_cap * elem_size`, which itself lies inside
            // the buffer — matching `try_shrink_at_cursor`'s contract.
            if unsafe { self.arena.try_shrink_at_cursor(buffer_end, reclaim_bytes) } {
                self.cap = new_cap;
            }
            return Ok(());
        }
        let new_layout = Layout::array::<T>(new_cap).map_err(|_e| AllocError)?;
        // Prefer in-place growth when this buffer still ends at the bump cursor.
        if new_cap > self.cap && self.cap > 0 {
            // `self.cap > 0` implies the old buffer was allocated via a
            // valid `Layout::array::<T>(self.cap)`, so re-deriving it
            // here cannot fail.
            let old_layout = Layout::array::<T>(self.cap).expect("self.cap was previously used to build a valid Layout::array");
            // SAFETY: `data` was allocated by `arena` with `old_layout`.
            if let Some(grown) = unsafe { self.arena.try_grow_in_place(self.data.cast::<u8>(), old_layout, new_layout) } {
                self.data = grown.cast::<T>();
                self.cap = new_cap;
                return Ok(());
            }
        }
        let new_data = self.arena.allocate(new_layout)?.cast::<T>();
        if self.len > 0 {
            // SAFETY: `new_data` points to `new_cap >= len` uninitialized elements; old range has `len` initialized elements.
            unsafe { ptr::copy_nonoverlapping(self.data.as_ptr(), new_data.as_ptr(), self.len) };
        }
        let old_data = self.data;
        let old_cap = self.cap;
        self.data = new_data;
        self.cap = new_cap;
        Self::deallocate_buffer(self.arena, old_data, old_cap);
        if old_cap > 0 {
            self.arena.bump_relocation();
        }
        Ok(())
    }

    /// Run `f` on a temporary `ApiVec` built from our raw parts.
    ///
    /// A Drop guard restores the parts on return or panic, so slow-path
    /// helpers inherit `ApiVec`'s semantics without custom handling.
    #[inline]
    pub(super) fn with_apivec<R, F: FnOnce(&mut ApiVec<T, &'a Arena<A>>) -> R>(&mut self, f: F) -> R {
        // Move the raw parts into a temporary `ApiVec`; `Restore` writes them
        // back even if `f` panics.
        let data = self.data;
        let len = self.len;
        let cap = self.cap;
        self.data = NonNull::dangling();
        self.len = 0;
        self.cap = 0;
        // SAFETY: `data`, `len`, and `cap` are this vector's raw parts allocated through `self.arena`.
        let api = unsafe { ApiVec::from_raw_parts_in(data.as_ptr(), len, cap, self.arena) };

        struct Restore<'s, 'a, T, A: Allocator + Clone> {
            dst: &'s mut Vec<'a, T, A>,
            api: ManuallyDrop<ApiVec<T, &'a Arena<A>>>,
        }
        impl<T, A: Allocator + Clone> Drop for Restore<'_, '_, T, A> {
            fn drop(&mut self) {
                // SAFETY: `api` is still intact; move its raw parts back to
                // `dst` and forget it so `dst` regains ownership.
                let mut api = unsafe { ManuallyDrop::take(&mut self.api) };
                let ptr = api.as_mut_ptr();
                let len = api.len();
                let cap = api.capacity();
                core::mem::forget(api);
                self.dst.data = NonNull::new(ptr).unwrap_or_else(NonNull::dangling);
                self.dst.len = len;
                self.dst.cap = cap;
            }
        }

        let mut restore = Restore {
            dst: self,
            api: ManuallyDrop::new(api),
        };
        let result = f(&mut restore.api);
        drop(restore);
        result
    }

    pub(super) fn deallocate_buffer(arena: &'a Arena<A>, data: NonNull<T>, cap: usize) {
        if cap == 0 || core::mem::size_of::<T>() == 0 {
            return;
        }
        // `cap` already passed `Layout::array::<T>` in `realloc`.
        let layout = Layout::array::<T>(cap).expect("self.cap was previously used to build a valid Layout::array");
        // SAFETY: `data` was allocated from `arena` with this layout, and this releases the allocation refcount.
        unsafe { arena.deallocate(data.cast(), layout) };
    }
}
