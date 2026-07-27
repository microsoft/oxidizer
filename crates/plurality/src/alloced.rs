// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::marker::PhantomData;
use core::mem::{MaybeUninit, forget};
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;

use allocator_api2::alloc::{Allocator, Global};

use crate::common::{impl_handle_common, impl_handle_mut};
use crate::pool::{Pool, drop_and_free_local};
use crate::slot::SlotCell;

/// A unique, owning handle to a value in a [`Pool`](crate::Pool) that **borrows**
/// the pool.
///
/// `Alloc` is the cheapest handle: because its `'pool` lifetime statically proves
/// the pool outlives it, allocating and freeing skip the pool's reference count
/// entirely. The trade-off is that it is `!Send` and cannot outlive the pool or
/// be stored in `'static` data — use [`Box`](crate::Box) for that.
///
/// Derefs to `&T`/`&mut T`; dropping it runs `T`'s destructor and returns the
/// slot to the pool.
///
/// `Alloc` cannot be pinned because forgetting it may end the pool borrow
/// without keeping the backing storage alive.
pub struct Alloc<'pool, T, A: Allocator = Global> {
    slot: NonNull<SlotCell<T>>,
    // Ties the handle to the pool borrow (so it can't outlive the pool) and, via
    // `&Pool` being `!Send`/`!Sync`, keeps the handle on one thread.
    _pool: PhantomData<&'pool Pool<T, A>>,
}

impl<T, A: Allocator> Alloc<'_, T, A> {
    #[inline]
    pub(crate) fn from_slot(slot: NonNull<SlotCell<T>>) -> Self {
        Self { slot, _pool: PhantomData }
    }
}

impl<'pool, T, A: Allocator> Alloc<'pool, MaybeUninit<T>, A> {
    /// Converts an uninitialized handle into an initialized one.
    ///
    /// # Safety
    /// The value must have been fully initialized before calling.
    #[must_use]
    pub unsafe fn assume_init(self) -> Alloc<'pool, T, A> {
        let slot = self.slot.cast::<SlotCell<T>>();
        // Don't run the uninit handle's destructor; transfer the slot as-is.
        forget(self);
        Alloc::from_slot(slot)
    }
}

impl<T, A: Allocator> Deref for Alloc<'_, T, A> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the slot is occupied for as long as this `Alloc` is alive.
        unsafe { SlotCell::value_ref(self.slot) }
    }
}

impl<T, A: Allocator> DerefMut for Alloc<'_, T, A> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: this `Alloc` is the unique owner of the occupied slot.
        unsafe { SlotCell::value_mut(self.slot) }
    }
}

impl<T, A: Allocator> Drop for Alloc<'_, T, A> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: unique owner of the occupied slot. No `pool_refcount` work —
        // the `'pool` borrow proves the pool is alive.
        unsafe { drop_and_free_local::<T>(self.slot) };
    }
}

impl_handle_common!(Alloc, 'pool);
impl_handle_mut!(Alloc, 'pool);
