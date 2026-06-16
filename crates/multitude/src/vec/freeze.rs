// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
//! Freeze a transient builder into arena-owned `Arc` or `Box` slices.
//!
//! The infallible freezes are exposed as `From<Vec<…>>` impls on
//! [`Arc`](crate::Arc) / [`Box`](crate::Box) (mirroring `std`'s
//! `From<Vec<T>> for Box<[T]>` / `Arc<[T]>`) plus the `std`-named
//! [`Vec::into_boxed_slice`] / [`Vec::leak`] methods. Fallible variants
//! ([`Vec::try_into_arc`] / [`Vec::try_into_boxed_slice`]) have no `std`
//! counterpart and stay as inherent methods.

use core::mem::{self, ManuallyDrop};
use core::slice;

use allocator_api2::alloc::{AllocError, Allocator};

use super::Vec;
use crate::arc::Arc;
use crate::r#box::Box;

impl<'a, T, A: Allocator + Clone> Vec<'a, T, A> {
    /// Freeze into a [`Box<[T], A>`](crate::Box).
    ///
    /// **O(n)** — moves the elements into a fresh shared allocation
    /// (no `Copy`/`Clone` required). Mirrors
    /// [`std::vec::Vec::into_boxed_slice`]; [`Box::from`] is the trait form.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails, or — for `T: Drop` — if
    /// `len` exceeds `u16::MAX`.
    #[must_use]
    pub fn into_boxed_slice(self) -> Box<[T], A> {
        let arena = self.arena;
        let mut me = ManuallyDrop::new(self);
        let iter = me.buf.drain_all();
        let bx = arena.alloc_slice_fill_iter_box::<T, _>(iter);
        // `drain_all` set `buf.len = 0`, so `into_inner`'s normal `Drop`
        // only releases the (unused) backing buffer.
        drop(ManuallyDrop::into_inner(me));
        bx
    }

    /// Fallible variant of [`Self::into_boxed_slice`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing shared-flavor allocation
    /// fails. On error, `self` is consumed and any elements remaining
    /// after a partial move are dropped before this function returns.
    pub fn try_into_boxed_slice(self) -> Result<Box<[T], A>, AllocError> {
        let arena = self.arena;
        let mut me = ManuallyDrop::new(self);
        let iter = me.buf.drain_all();
        let result = arena.try_alloc_slice_fill_iter_box::<T, _>(iter);
        // See `into_boxed_slice`.
        drop(ManuallyDrop::into_inner(me));
        result
    }

    /// Fallible variant of the [`Arc<[T], A>`](crate::Arc) freeze
    /// ([`Arc::from`]).
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing shared-flavor allocation
    /// fails. On error, `self` is consumed and any elements remaining
    /// after a partial move are dropped before this function returns.
    pub fn try_into_arc(self) -> Result<Arc<[T], A>, AllocError>
    where
        T: Send + Sync,
        A: Send + Sync,
    {
        let arena = self.arena;
        let mut me = ManuallyDrop::new(self);
        let iter = me.buf.drain_all();
        let result = arena.try_alloc_slice_fill_iter_arc::<T, _>(iter);
        // See `into_boxed_slice`.
        drop(ManuallyDrop::into_inner(me));
        result
    }

    /// Consume the `Vec`, returning an arena-lifetime mutable slice
    /// reference `&'a mut [T]`. Mirrors [`std::vec::Vec::leak`].
    ///
    /// **O(1) and allocation-free**: the existing buffer is reinterpreted
    /// as a slice reference in place. No copy, no new allocation. The
    /// unused tail (`cap - len`) is reclaimed back to the chunk's bump
    /// cursor when this buffer is still the chunk's last allocation, so
    /// later allocations can reuse it; otherwise it is left in the chunk
    /// and reclaimed when the arena is dropped.
    ///
    /// Available only when `T` does not need `Drop` (compile-time
    /// asserted). For drop types, freeze via [`Box::from`] / [`Arc::from`].
    #[must_use]
    pub fn leak(mut self) -> &'a mut [T] {
        const {
            assert!(
                !mem::needs_drop::<T>(),
                "Vec::leak requires T not to need Drop; freeze via Box::from / Arc::from instead",
            );
        }
        // Hand the unused capacity tail back to the chunk before pinning
        // the live prefix as a slice. `[len, cap)` holds no initialized
        // element, so reclaiming it is sound; the retained `[0, len)`
        // prefix (and thus the returned slice) is untouched.
        let _ = self.reclaim_capacity_tail(self.buf.len());
        let mut me = ManuallyDrop::new(self);
        let ptr = me.buf.as_mut_ptr();
        let len = me.buf.len();
        // SAFETY: by `ArenaBuf`'s invariants, `ptr` addresses `len`
        // initialized `T`s in an arena chunk that outlives `'a`. We
        // `ManuallyDrop` the `Vec` so neither the `ArenaBuf` nor its
        // contained elements are dropped here. Since `T` does not need
        // `Drop` (const-asserted above), abandoning the buffer without
        // registering a chunk drop entry is sound — the chunk storage
        // itself is reclaimed at arena teardown.
        unsafe { slice::from_raw_parts_mut(ptr, len) }
    }

    /// Internal: shared body for the infallible `Arc<[T]>` freeze, used by
    /// `From<Vec<…>> for Arc<[T], A>`.
    pub(crate) fn freeze_into_arc(self) -> Arc<[T], A>
    where
        T: Send + Sync,
        A: Send + Sync,
    {
        let arena = self.arena;
        let mut me = ManuallyDrop::new(self);
        let iter = me.buf.drain_all();
        let arc = arena.alloc_slice_fill_iter_arc::<T, _>(iter);
        // See `into_boxed_slice`.
        drop(ManuallyDrop::into_inner(me));
        arc
    }
}
