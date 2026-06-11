// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
//! Freeze a transient builder into arena-owned `Arc` or `Box` slices.

use core::mem::ManuallyDrop;

use allocator_api2::alloc::{AllocError, Allocator};

use super::Vec;
use crate::arc::Arc;
use crate::r#box::Box;

impl<T, A: Allocator + Clone> Vec<'_, T, A> {
    /// Freeze into an [`Arc<[T], A>`](crate::Arc).
    ///
    /// The contents are moved into a fresh shared-flavor arena
    /// allocation.
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
        let arena = self.arena;
        let mut me = ManuallyDrop::new(self);
        let iter = me.buf.drain_all();
        let arc = arena.alloc_slice_fill_iter_arc::<T, _>(iter);
        // `drain_all` set `buf.len = 0`, so `into_inner`'s normal `Drop`
        // only releases the (unused) backing buffer.
        drop(ManuallyDrop::into_inner(me));
        arc
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
        let arena = self.arena;
        let mut me = ManuallyDrop::new(self);
        let iter = me.buf.drain_all();
        let result = arena.try_alloc_slice_fill_iter_arc::<T, _>(iter);
        // See `into_arena_arc`.
        drop(ManuallyDrop::into_inner(me));
        result
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
    pub fn into_arena_box(self) -> Box<[T], A> {
        let arena = self.arena;
        let mut me = ManuallyDrop::new(self);
        let iter = me.buf.drain_all();
        let bx = arena.alloc_slice_fill_iter_box::<T, _>(iter);
        // See `into_arena_arc`.
        drop(ManuallyDrop::into_inner(me));
        bx
    }
}
