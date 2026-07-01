// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::ptr::NonNull;

use allocator_api2::alloc::{Allocator, Global};

use crate::common::{impl_handle_common, impl_handle_mut};
use crate::pool::drop_and_free;
use crate::slot::SlotCell;

/// A unique, owning handle to a value in a [`Pool`](crate::Pool).
///
/// One pointer wide. Derefs to `&T`/`&mut T`; dropping it runs `T`'s destructor
/// and returns the slot to the pool. `Box` may outlive the `Pool` handle.
pub struct Box<T, A: Allocator = Global> {
    slot: NonNull<SlotCell<T>>,
    _marker: PhantomData<(T, A)>,
}

// SAFETY: a `Box` is the unique owner of its slot; the pool machinery it touches
// on drop is atomic. Sending requires `T: Send` and a thread-safe allocator.
unsafe impl<T: Send, A: Allocator + Send + Sync> Send for Box<T, A> {}
// SAFETY: `&Box` only exposes `&T`, so sharing needs `T: Sync`.
unsafe impl<T: Sync, A: Allocator + Send + Sync> Sync for Box<T, A> {}

impl<T, A: Allocator> Box<T, A> {
    #[inline]
    pub(crate) fn from_slot(slot: NonNull<SlotCell<T>>) -> Self {
        Self {
            slot,
            _marker: PhantomData,
        }
    }
}

impl<T, A: Allocator> Box<MaybeUninit<T>, A> {
    /// Converts an uninitialized box into an initialized one.
    ///
    /// # Safety
    /// The value must have been fully initialized (e.g. via
    /// [`MaybeUninit::write`]) before calling.
    #[must_use]
    pub unsafe fn assume_init(self) -> Box<T, A> {
        let slot = self.slot.cast::<SlotCell<T>>();
        // Don't run the uninit box's destructor; transfer the slot as-is.
        core::mem::forget(self);
        Box::from_slot(slot)
    }

    /// Converts a pinned, uninitialized box into a pinned, initialized one.
    ///
    /// # Safety
    /// The value must have been fully initialized before calling.
    #[must_use]
    pub unsafe fn assume_init_pin(this: Pin<Self>) -> Pin<Box<T, A>> {
        // SAFETY: the caller guarantees the value is initialized; the slot
        // address is unchanged, so re-pinning is sound.
        unsafe {
            let inner = Pin::into_inner_unchecked(this);
            Pin::new_unchecked(inner.assume_init())
        }
    }
}

impl<T, A: Allocator> Deref for Box<T, A> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the slot is occupied for as long as this `Box` is alive.
        unsafe { SlotCell::value_ref(self.slot) }
    }
}

impl<T, A: Allocator> DerefMut for Box<T, A> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: this `Box` is the unique owner of the occupied slot.
        unsafe { SlotCell::value_mut(self.slot) }
    }
}

impl<T, A: Allocator> Drop for Box<T, A> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: unique owner; the last (only) handle to the occupied slot.
        unsafe { drop_and_free::<T, A>(self.slot) };
    }
}

impl_handle_common!(Box);
impl_handle_mut!(Box);
