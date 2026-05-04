// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Freeze a transient builder into arena-owned `Rc`, `Arc`, or `Box`
//! slices.

use core::mem::ManuallyDrop;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use super::Vec;
use crate::arc::Arc;
use crate::r#box::Box;
use crate::internal::drop_list::{drop_shim_slice, noop_drop_shim};
use crate::rc::Rc;

impl<T, A: Allocator + Clone> Vec<'_, T, A> {
    /// Freeze into an [`Rc<[T], A>`](crate::Rc).
    ///
    /// **O(1)** when possible: the existing buffer is handed to the
    /// `Rc` directly (no copy, no fresh allocation). Falls back to a
    /// copy when the in-place transfer is impossible (zero-sized
    /// elements, an unallocated builder, or a `T: Drop` payload that
    /// is too large or whose chunk has no remaining slot for the
    /// trailing slice drop entry).
    ///
    /// If the buffer's tail still sits at the chunk's bump cursor (the
    /// common case when no other allocation interleaved), the unused
    /// tail (`cap - len`) is returned to the cursor so subsequent
    /// allocations pack adjacently.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails on the copy fallback,
    /// or if `T: Drop` and `align_of::<T>() >= 32 KiB` (alignments at
    /// or above half the chunk size cannot be safely dropped through a
    /// smart pointer; see the crate-level docs).
    #[must_use]
    // EQUIVALENCE: the `u16::MAX` guard cannot fire on an in-place-eligible
    // buffer; `len > 0` to `>=` only installs a zero-length drop entry; and
    // `cap > len` to `>=` only changes a zero-byte reclaim.
    #[cfg_attr(test, mutants::skip)]
    pub fn into_arena_rc(self) -> Rc<[T], A> {
        // Copy when there is no transferable buffer or the slice-drop entry
        // cannot encode this length.
        let elem_size = core::mem::size_of::<T>();
        let needs_drop = core::mem::needs_drop::<T>();
        if elem_size == 0 || self.cap == 0 || (needs_drop && self.len > u16::MAX as usize) {
            return Self::into_arena_rc_copy(self);
        }

        // The Vec's chunk ref becomes the Rc's chunk ref; suppress Vec drop.
        let me = ManuallyDrop::new(self);
        let len = me.len;
        let cap = me.cap;
        let data = me.data;
        let arena = me.arena;

        if needs_drop && len > 0 {
            // Install a slice drop entry in-place; otherwise fall back to copy.
            // SAFETY: `data` was returned by an arena allocation, and
            // the buffer holds exactly `len` initialized `T`s.
            let installed = unsafe {
                arena.try_install_slice_drop_entry(
                    data.cast::<u8>(),
                    drop_shim_slice::<T>,
                    u16::try_from(len).expect("guarded by len > u16::MAX check above"),
                )
            };
            if !installed {
                // Restore the Vec unchanged for the copy fallback.
                let restored = ManuallyDrop::into_inner(me);
                return Self::into_arena_rc_copy(restored);
            }
        }

        // Best-effort reclaim of any unused tail space.
        if cap > len {
            let reclaim_bytes = (cap - len) * elem_size;
            // SAFETY: `data + cap` is one past the buffer end, within
            // (or one past) the original allocation.
            let buffer_end = unsafe { data.as_ptr().add(cap).cast::<u8>() };
            // SAFETY: we computed `buffer_end = data + cap*elem_size`
            // from a live arena allocation; reclaim_bytes is the
            // matching unused-tail size.
            let _ = unsafe { arena.try_shrink_at_cursor(buffer_end, reclaim_bytes) };
        }

        // Rewrap the existing buffer as `Rc<[T]>`.
        let fat = core::ptr::slice_from_raw_parts_mut(data.as_ptr(), len);
        // SAFETY: `fat` is non-null, the chunk ref now belongs to the Rc, and
        // any installed slice drop entry will run exactly once.
        unsafe { Rc::from_value_ptr(NonNull::new_unchecked(fat)) }
    }

    /// Copy fallback for `into_arena_rc`.
    #[cold]
    #[inline(never)]
    fn into_arena_rc_copy(self) -> Rc<[T], A> {
        let mut me = ManuallyDrop::new(self);
        let len = me.len;
        let data = me.data;
        let cap = me.cap;
        let arena = me.arena;
        let consumed_cell: core::cell::Cell<usize> = core::cell::Cell::new(0);
        let result = arena.try_alloc_slice_fill_with_rc(len, |_| {
            let idx = consumed_cell.get();
            // SAFETY: `idx < len` for each call, and each element is read exactly once.
            let value = unsafe { data.as_ptr().add(idx).read() };
            consumed_cell.set(idx + 1);
            value
        });
        match result {
            Ok(rc) => {
                me.len = 0;
                Self::deallocate_buffer(arena, data, cap);
                rc
            }
            Err(_) => {
                // Move any surviving tail down so `ManuallyDrop::drop` only
                // sees live elements.
                Self::cleanup_after_partial_move(&mut me, consumed_cell.get());
                // SAFETY: `me.len` was updated to the remaining live count.
                unsafe { ManuallyDrop::drop(&mut me) };
                panic!("multitude: allocator returned AllocError");
            }
        }
    }

    /// Freeze into an [`Arc<[T], A>`](crate::Arc).
    ///
    /// The contents are moved into a fresh shared-flavor arena
    /// allocation. Unlike [`Self::into_arena_rc`], no in-place fast
    /// path is available because the builder's buffer lives in a
    /// local (single-thread) chunk, while `Arc<[T]>` must reference a
    /// shared-flavor chunk.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails.
    #[must_use]
    pub fn into_arena_arc(self) -> Arc<[T], A>
    where
        T: Send + Sync,
        A: Send + Sync,
    {
        match self.try_into_arena_arc() {
            Ok(a) => a,
            Err(_) => panic!("multitude: allocator returned AllocError"),
        }
    }

    /// Fallible variant of [`Self::into_arena_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing shared-flavor allocation
    /// fails. On error, `self` is consumed and any elements remaining
    /// after a partial move are dropped before this function returns.
    pub fn try_into_arena_arc(self) -> Result<Arc<[T], A>, AllocError>
    where
        T: Send + Sync,
        A: Send + Sync,
    {
        let mut me = ManuallyDrop::new(self);
        let len = me.len;
        let data = me.data;
        let cap = me.cap;
        let arena = me.arena;
        // Track how many elements the fill closure has already moved.
        let consumed_cell: core::cell::Cell<usize> = core::cell::Cell::new(0);
        let result = arena.try_alloc_slice_fill_with_arc(len, |_| {
            let idx = consumed_cell.get();
            // SAFETY: `idx < len` for each call (the helper invokes
            // the closure exactly `len` times on success); each element
            // is read exactly once.
            let value = unsafe { data.as_ptr().add(idx).read() };
            consumed_cell.set(idx + 1);
            value
        });
        match result {
            Ok(arc) => {
                // Ownership fully moved into the new allocation.
                me.len = 0;
                Self::deallocate_buffer(arena, data, cap);
                Ok(arc)
            }
            Err(e) => {
                // Move any surviving tail down so `ManuallyDrop::drop` only
                // sees live elements.
                Self::cleanup_after_partial_move(&mut me, consumed_cell.get());
                // SAFETY: `me.len` was updated to the remaining live count.
                unsafe { ManuallyDrop::drop(&mut me) };
                Err(e)
            }
        }
    }

    /// Shift a partially moved tail down so Vec drop only sees live elements.
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)] // Current callers fail before consuming elements; keep the future panic-safety path covered.
    fn cleanup_after_partial_move(me: &mut ManuallyDrop<Self>, consumed: usize) {
        let original_len = me.len;
        let tail_len = original_len.saturating_sub(consumed);
        if tail_len > 0 && consumed > 0 {
            // SAFETY: both ranges lie within the same allocation;
            // `ptr::copy` handles overlap.
            unsafe {
                let base = me.data.as_ptr();
                core::ptr::copy(base.add(consumed), base, tail_len);
            }
        }
        me.len = tail_len;
    }

    /// Freeze into a [`Box<[T], A>`](crate::Box).
    ///
    /// **O(1)**: the existing buffer is handed to the `Box` directly.
    /// `Box<[T]>`'s `Drop` runs `drop_in_place::<[T]>` over the slice
    /// (using its fat-pointer length) when the box is dropped, so no
    /// trailing chunk drop entry is needed regardless of `T: Drop`.
    ///
    /// If the buffer's tail still sits at the chunk's bump cursor, the
    /// unused tail (`cap - len`) is returned to the cursor.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails on the copy fallback
    /// for the ZST / empty-builder cases.
    #[must_use]
    // EQUIVALENCE: the extra `||` cases only install noop or empty drop
    // entries; the `u16::MAX` guard cannot fire on an in-place-eligible
    // buffer; and `cap > len` to `>=` only changes a zero-byte reclaim.
    #[cfg_attr(test, mutants::skip)]
    pub fn into_arena_box(self) -> Box<[T], A> {
        let elem_size = core::mem::size_of::<T>();
        let needs_drop = core::mem::needs_drop::<T>();
        // Copy when there is no transferable buffer or the slice-drop entry
        // cannot encode this length. Keep the `u16::MAX` guard as defense in
        // depth even though the in-place path cannot currently reach it.
        if elem_size == 0 || self.cap == 0 || (needs_drop && self.len > u16::MAX as usize) {
            return Self::into_arena_box_copy(self);
        }

        // `Vec -> Box -> Rc` for `T: Drop` needs a preinstalled noop entry so
        // later retargeting can switch it to `drop_shim_slice`. If that cannot
        // be installed here, fall back to the normal copy path.
        if needs_drop && self.len > 0 {
            let me = ManuallyDrop::new(self);
            let arena = me.arena;
            let data = me.data;
            // SAFETY: `data` was returned by an arena allocation and
            // the Vec holds the chunk's +1.
            let installed = unsafe {
                arena.try_install_slice_drop_entry(
                    data.cast::<u8>(),
                    noop_drop_shim,
                    u16::try_from(me.len).expect("guarded by len > u16::MAX check above"),
                )
            };
            if !installed {
                let restored = ManuallyDrop::into_inner(me);
                return Self::into_arena_box_copy(restored);
            }
            let len = me.len;
            let cap = me.cap;
            if cap > len {
                let reclaim_bytes = (cap - len) * elem_size;
                // SAFETY: `data + cap` is one past the buffer end.
                let buffer_end = unsafe { data.as_ptr().add(cap).cast::<u8>() };
                // SAFETY: see `into_arena_rc`.
                let _ = unsafe { arena.try_shrink_at_cursor(buffer_end, reclaim_bytes) };
            }
            let fat = core::ptr::slice_from_raw_parts_mut(data.as_ptr(), len);
            // SAFETY: `fat` is non-null; the chunk +1 the Vec held now
            // belongs to the Box.
            return unsafe { Box::from_raw_unsized(NonNull::new_unchecked(fat)) };
        }

        let me = ManuallyDrop::new(self);
        let len = me.len;
        let cap = me.cap;
        let data = me.data;
        let arena = me.arena;

        if cap > len {
            let reclaim_bytes = (cap - len) * elem_size;
            // SAFETY: `data + cap` is one past the buffer end.
            let buffer_end = unsafe { data.as_ptr().add(cap).cast::<u8>() };
            // SAFETY: see `into_arena_rc`.
            let _ = unsafe { arena.try_shrink_at_cursor(buffer_end, reclaim_bytes) };
        }

        // Keep `me` alive only to suppress Vec drop; the chunk ref transfers.
        let _ = me;

        let fat = core::ptr::slice_from_raw_parts_mut(data.as_ptr(), len);
        // SAFETY: `fat` is non-null; the chunk +1 the Vec held now
        // belongs to the Box. `Box<[T]>`'s `Drop` will call
        // `drop_in_place::<[T]>` over the slice, dropping each `T`.
        unsafe { Box::from_raw_unsized(NonNull::new_unchecked(fat)) }
    }

    /// Copy fallback for `into_arena_box` (ZST / empty-builder cases).
    #[cold]
    #[inline(never)]
    // EQUIVALENCE: this fallback only runs for ZSTs or empty builders, so
    // changing `idx + 1` cannot affect observable behavior.
    #[cfg_attr(test, mutants::skip)]
    fn into_arena_box_copy(self) -> Box<[T], A> {
        let mut me = ManuallyDrop::new(self);
        let len = me.len;
        let data = me.data;
        let cap = me.cap;
        let arena = me.arena;
        let consumed_cell: core::cell::Cell<usize> = core::cell::Cell::new(0);
        let result = arena.try_alloc_slice_fill_with_box(len, |_| {
            let idx = consumed_cell.get();
            // SAFETY: `idx < len` for each call, and each element is read exactly once.
            let value = unsafe { data.as_ptr().add(idx).read() };
            consumed_cell.set(idx + 1);
            value
        });
        match result {
            Ok(b) => {
                me.len = 0;
                Self::deallocate_buffer(arena, data, cap);
                b
            }
            Err(_) => {
                Self::cleanup_after_partial_move(&mut me, consumed_cell.get());
                // SAFETY: `me.len` was updated to the remaining live count.
                unsafe { ManuallyDrop::drop(&mut me) };
                panic!("multitude: allocator returned AllocError");
            }
        }
    }
}
