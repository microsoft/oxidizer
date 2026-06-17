// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
//! Freeze a transient vector into arena-owned `Arc` or `Box` slices.
//!
//! Infallible freezes use `From<Vec<…>>` for [`Arc`](crate::Arc) /
//! [`Box`](crate::Box) plus [`Vec::into_boxed_slice`] / [`Vec::leak`].
//! Fallible freezes are [`Vec::try_into_arc`] and [`Vec::try_into_boxed_slice`].

use core::mem::{self, ManuallyDrop};
use core::slice;

use allocator_api2::alloc::{AllocError, Allocator};

use super::Vec;
use crate::Arena;
use crate::arc::Arc;
use crate::r#box::Box;
use crate::internal::arena_buf::DrainAll;

impl<'a, T, A: Allocator + Clone> Vec<'a, T, A> {
    /// Shared body of the `Box`/`Arc` freeze paths: drain every element
    /// into a fresh shared allocation built by `build`, then release this
    /// `Vec`'s now-empty backing buffer. The old buffer is dropped only
    /// *after* `build` consumes the drain iterator, so the moved-out
    /// elements stay readable for the duration of the freeze.
    #[inline]
    fn drain_freeze<R>(self, build: impl FnOnce(&'a Arena<A>, DrainAll<'a, T>) -> R) -> R {
        let arena = self.arena;
        let mut me = ManuallyDrop::new(self);
        let iter = me.buf.drain_all();
        let result = build(arena, iter);
        // `drain_all` set `buf.len = 0`, so this only releases the (unused)
        // backing buffer, never the moved-out elements.
        drop(ManuallyDrop::into_inner(me));
        result
    }

    /// Freeze into a [`Box<[T], A>`](crate::Box).
    ///
    /// **O(n)** — moves the elements into a fresh shared allocation
    /// (no `Copy`/`Clone` required). Mirrors
    /// [`std::vec::Vec::into_boxed_slice`]; [`Box::from`] is the trait form.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails.
    #[must_use]
    pub fn into_boxed_slice(self) -> Box<[T], A> {
        self.drain_freeze(Arena::alloc_slice_fill_iter_box::<T, _>)
    }

    /// Fallible variant of [`Self::into_boxed_slice`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing shared-flavor allocation
    /// fails. On error, `self` is consumed and any elements remaining
    /// after a partial move are dropped before this function returns.
    pub fn try_into_boxed_slice(self) -> Result<Box<[T], A>, AllocError> {
        self.drain_freeze(Arena::try_alloc_slice_fill_iter_box::<T, _>)
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
        self.drain_freeze(Arena::try_alloc_slice_fill_iter_arc::<T, _>)
    }

    /// Consume the `Vec`, returning an arena-lifetime mutable slice
    /// reference `&'a mut [T]`. Mirrors [`std::vec::Vec::leak`].
    ///
    /// **O(1) and allocation-free**: the existing buffer becomes the returned
    /// slice. The unused tail is reclaimed only while this buffer is still the
    /// chunk's last allocation; otherwise arena teardown reclaims it.
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
        // Reclaim the uninitialized capacity tail before pinning the live
        // prefix as the returned slice.
        let _ = self.reclaim_capacity_tail(self.buf.len());
        let mut me = ManuallyDrop::new(self);
        let ptr = me.buf.as_mut_ptr();
        let len = me.buf.len();
        // SAFETY: `ptr` addresses `len` initialized `T`s in an arena chunk
        // that outlives `'a`. `ManuallyDrop` prevents dropping the buffer or
        // elements here; `T: !Drop` (const-asserted above) lets arena teardown
        // reclaim the raw chunk storage without a drop entry.
        unsafe { slice::from_raw_parts_mut(ptr, len) }
    }

    /// Internal: shared body for the infallible `Arc<[T]>` freeze, used by
    /// `From<Vec<…>> for Arc<[T], A>`.
    pub(crate) fn freeze_into_arc(self) -> Arc<[T], A>
    where
        T: Send + Sync,
        A: Send + Sync,
    {
        self.drain_freeze(Arena::alloc_slice_fill_iter_arc::<T, _>)
    }
}
