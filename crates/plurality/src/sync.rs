// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ops::Deref;
use core::pin::Pin;
use core::ptr::NonNull;

use allocator_api2::alloc::{Allocator, Global};

use crate::atomic::Ordering::{Acquire, Relaxed, Release};
use crate::atomic::fence;
use crate::common::impl_handle_common;
use crate::pool::drop_and_free;
use crate::slot::{SlotCell, check_refcount_overflow};

/// A shared, atomically reference-counted handle to a value in a
/// [`Pool`](crate::Pool).
///
/// One pointer wide. Derefs to `&T` (read-only — put a `Mutex`/`RefCell` inside
/// `T` for interior mutability). Cloning bumps an atomic refcount; the value is
/// dropped and the slot returned when the last `Arc` drops. `Arc` may outlive
/// the `Pool` handle and may be dropped from any thread.
pub struct Arc<T, A: Allocator = Global> {
    slot: NonNull<SlotCell<T>>,
    _marker: PhantomData<(T, A)>,
}

// SAFETY: refcounting is atomic and the value is shared immutably, so the usual
// `Arc` bounds apply, plus a thread-safe allocator for cross-thread teardown.
unsafe impl<T: Send + Sync, A: Allocator + Send + Sync> Send for Arc<T, A> {}
// SAFETY: as above.
unsafe impl<T: Send + Sync, A: Allocator + Send + Sync> Sync for Arc<T, A> {}

impl<T, A: Allocator> Arc<T, A> {
    #[inline]
    pub(crate) fn from_slot(slot: NonNull<SlotCell<T>>) -> Self {
        Self {
            slot,
            _marker: PhantomData,
        }
    }

    /// Returns a mutable reference to the value if this `Arc` is the only handle
    /// to its slot, otherwise `None`. Mirrors [`alloc::sync::Arc::get_mut`].
    #[must_use]
    pub fn get_mut(this: &mut Self) -> Option<&mut T> {
        // SAFETY: the slot is occupied while this `Arc` is alive.
        let unique = unsafe { (*this.slot.as_ptr()).refcount.load(Acquire) == 1 };
        // SAFETY: a unique handle, so `&mut` to the slot value is exclusive.
        unique.then(|| unsafe { SlotCell::value_mut(this.slot) })
    }

    /// Returns `true` if both handles point to the same slot (the same
    /// allocation). Mirrors [`alloc::sync::Arc::ptr_eq`].
    #[must_use]
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        a.slot == b.slot
    }
}

impl<T, A: Allocator> Arc<MaybeUninit<T>, A> {
    /// Converts an uninitialized arc into an initialized one.
    ///
    /// # Safety
    /// The value must have been fully initialized before calling. If other
    /// `Arc` clones to this slot exist, they must observe an initialized value.
    #[must_use]
    pub unsafe fn assume_init(self) -> Arc<T, A> {
        let slot = self.slot.cast::<SlotCell<T>>();
        // Don't run the uninit arc's destructor; transfer the slot as-is
        // (the refcount is preserved unchanged).
        core::mem::forget(self);
        Arc::from_slot(slot)
    }

    /// Converts a pinned, uninitialized arc into a pinned, initialized one.
    ///
    /// # Safety
    /// The value must have been fully initialized before calling.
    #[must_use]
    pub unsafe fn assume_init_pin(this: Pin<Self>) -> Pin<Arc<T, A>> {
        // SAFETY: the caller guarantees initialization; the slot address is
        // unchanged, so re-pinning is sound.
        unsafe {
            let inner = Pin::into_inner_unchecked(this);
            Pin::new_unchecked(inner.assume_init())
        }
    }
}

impl<T, A: Allocator> Clone for Arc<T, A> {
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: the slot is occupied while this `Arc` is alive.
        let old = unsafe { (*self.slot.as_ptr()).refcount.fetch_add(1, Relaxed) };
        check_refcount_overflow(old);
        Self::from_slot(self.slot)
    }
}

impl<T, A: Allocator> Deref for Arc<T, A> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the slot is occupied for as long as this `Arc` is alive.
        unsafe { SlotCell::value_ref(self.slot) }
    }
}

impl<T, A: Allocator> Drop for Arc<T, A> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: the slot is occupied; we hold one of its references.
        let prev = unsafe { (*self.slot.as_ptr()).refcount.fetch_sub(1, Release) };
        if prev != 1 {
            return;
        }
        fence(Acquire);
        // SAFETY: last reference — drop the value once, then return the slot.
        unsafe { drop_and_free::<T, A>(self.slot) };
    }
}

impl_handle_common!(Arc);
