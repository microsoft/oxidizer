// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "pointer-recovery and slot-lifecycle paths group tightly-coupled unsafe operations under a single documented safety invariant; one block per operation would duplicate that invariant and obscure it"
)]

use core::marker::PhantomData;
use core::mem::{MaybeUninit, forget};
use core::ops::Deref;
use core::pin::Pin;
use core::ptr::{NonNull, addr_eq};

use allocator_api2::alloc::{Allocator, Global};

use crate::atomic::Ordering::{Acquire, Relaxed, Release};
use crate::atomic::fence;
use crate::coerce::{self, Coercion};
use crate::common::impl_handle_common_unsized;
use crate::pool::{drop_and_free_val, refcount_ptr};
use crate::slot::{SlotCell, check_refcount_overflow};

/// A shared, atomically reference-counted handle to a value in a
/// [`Pool`](crate::Pool).
///
/// Derefs to `&T` (read-only — put a `Mutex`/`RefCell` inside `T` for interior
/// mutability). Cloning bumps an atomic refcount; the value is dropped and the
/// slot returned when the last `Arc` drops. `Arc` may outlive the `Pool` handle
/// and may be dropped from any thread.
///
/// Like [`Box`](crate::Box), `Arc` is generic over `T: ?Sized`, so it can share
/// an unsized value (a trait object or slice) obtained via [`Arc::unsize`]. A
/// sized `Arc` is one pointer wide; the unsized forms carry the pointer metadata.
///
/// Pinning follows [`alloc::sync::Arc::pin`]'s construction-time model. Use
/// [`Pool::alloc_arc_pin`](crate::Pool::alloc_arc_pin) for `!Unpin` values.
/// [`Pin::new`] can wrap an existing owner only when `T: Unpin`; Plurality
/// provides no safe conversion for an existing owner of a `!Unpin` value after
/// an ordinary alias may have escaped.
pub struct Arc<T: ?Sized, A: Allocator = Global> {
    /// Pointer to the **value** (field 0 of its `SlotCell<T>`); a fat pointer for
    /// unsized `T`. The refcount is recovered from the value's size on the shared
    /// clone/drop paths.
    slot: NonNull<T>,
    _marker: PhantomData<A>,
}

// SAFETY: refcounting is atomic and the value is shared immutably, so the usual
// `Arc` bounds apply, plus a thread-safe allocator for cross-thread teardown.
unsafe impl<T: ?Sized + Send + Sync, A: Allocator + Send + Sync> Send for Arc<T, A> {}
// SAFETY: as above.
unsafe impl<T: ?Sized + Send + Sync, A: Allocator + Send + Sync> Sync for Arc<T, A> {}

impl<T, A: Allocator> Arc<T, A> {
    #[inline]
    pub(crate) fn from_slot(slot: NonNull<SlotCell<T>>) -> Self {
        Self {
            // The value is field 0 of `SlotCell<T>`, so its address is the slot address.
            slot: slot.cast::<T>(),
            _marker: PhantomData,
        }
    }

    /// Erases a sized `Arc<T>` into an `Arc<U>` for an unsized `U` (e.g. a trait
    /// object or slice), transferring this handle's share of the slot.
    ///
    /// Like [`Box::unsize`](crate::Box::unsize), the
    /// [`Coercion`] token proves
    /// that the conversion preserves the slot's address and provenance:
    ///
    /// ```
    /// use core::fmt::Debug;
    ///
    /// use plurality::{Arc, Pool, coerce};
    ///
    /// let pool = Pool::<u32>::new();
    /// let a = pool.alloc_arc(7u32);
    /// let dyn_a: Arc<dyn Debug> = Arc::unsize::<dyn Debug>(a, coerce!(dyn Debug));
    /// assert_eq!(format!("{dyn_a:?}"), "7");
    /// ```
    ///
    /// The refcount is unchanged — other clones (which may still be `Arc<T>`) keep
    /// sharing the same slot; whichever handle drops last frees it.
    #[must_use]
    pub fn unsize<U: ?Sized>(this: Self, coercion: Coercion<T, U, impl FnOnce(*const T) -> *const U>) -> Arc<U, A> {
        let value = coerce::unsize(this.slot, coercion);
        // The returned handle inherits this handle's share of the slot.
        forget(this);
        Arc::from_value(value)
    }

    /// Erases a pinned sized owner while preserving its pinning guarantee.
    ///
    /// The allocation stays fixed and no ordinary owner is exposed.
    #[must_use]
    pub fn unsize_pin<U: ?Sized>(this: Pin<Self>, coercion: Coercion<T, U, impl FnOnce(*const T) -> *const U>) -> Pin<Arc<U, A>> {
        // SAFETY: the ordinary owner exists only inside this method. `unsize`
        // changes pointer metadata without moving or exposing the value, and
        // the resulting owner is re-pinned before it can escape.
        unsafe {
            let owner = Pin::into_inner_unchecked(this);
            let erased = Self::unsize(owner, coercion);
            Arc::into_pin_fresh(erased)
        }
    }
}

impl<T: ?Sized, A: Allocator> Arc<T, A> {
    #[inline]
    pub(crate) fn from_value(value: NonNull<T>) -> Self {
        Self {
            slot: value,
            _marker: PhantomData,
        }
    }

    /// Pins a newly constructed owner representation before it can escape.
    ///
    /// # Safety
    /// No ordinary, unpinned alias to this allocation may exist. Existing
    /// aliases, if any, must already be pinned owners.
    #[inline]
    pub(crate) unsafe fn into_pin_fresh(this: Self) -> Pin<Self> {
        // SAFETY: the caller guarantees that no ordinary alias exists, and the
        // owner retains the occupied slot at a stable address.
        unsafe { Pin::new_unchecked(this) }
    }

    /// Returns a mutable reference to the value if this `Arc` is the only handle
    /// to its slot, otherwise `None`. Mirrors [`alloc::sync::Arc::get_mut`].
    #[must_use]
    #[expect(
        clippy::unnecessary_lazy_evaluations,
        reason = "the unsafe deref must stay lazy: materializing `&mut T` when the handle is not unique would alias existing shared references"
    )]
    pub fn get_mut(this: &mut Self) -> Option<&mut T> {
        // SAFETY: the slot is occupied while this `Arc` is alive.
        let unique = unsafe { (*refcount_ptr(this.slot)).load(Acquire) == 1 };
        // SAFETY: a unique handle, so `&mut` to the slot value is exclusive.
        unique.then(|| unsafe { this.slot.as_mut() })
    }

    /// Returns `true` if both handles point to the same slot (the same
    /// allocation). Mirrors [`alloc::sync::Arc::ptr_eq`].
    #[must_use]
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        addr_eq(a.slot.as_ptr(), b.slot.as_ptr())
    }
}

impl<T, A: Allocator> Arc<MaybeUninit<T>, A> {
    #[inline]
    pub(crate) fn write_value(&mut self, value: T) {
        // SAFETY: allocation APIs call this only for a freshly reserved,
        // uniquely owned uninitialized slot.
        unsafe { self.slot.as_mut().write(value) };
    }

    /// Converts an uninitialized arc into an initialized one.
    ///
    /// # Safety
    /// The value must have been fully initialized before calling. If other
    /// `Arc` clones to this slot exist, they must observe an initialized value.
    #[must_use]
    pub unsafe fn assume_init(self) -> Arc<T, A> {
        let value = self.slot.cast::<T>();
        // Don't run the uninit arc's destructor; transfer the slot as-is
        // (the refcount is preserved unchanged).
        forget(self);
        Arc::from_value(value)
    }
}

impl<T: ?Sized, A: Allocator> Clone for Arc<T, A> {
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: the slot is occupied while this `Arc` is alive.
        let old = unsafe { (*refcount_ptr(self.slot)).fetch_add(1, Relaxed) };
        check_refcount_overflow(old);
        Self::from_value(self.slot)
    }
}

impl<T: ?Sized, A: Allocator> Deref for Arc<T, A> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the slot is occupied for as long as this `Arc` is alive.
        unsafe { self.slot.as_ref() }
    }
}

impl<T: ?Sized, A: Allocator> Drop for Arc<T, A> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: this handle owns one reference to the occupied slot.
        let prev = unsafe { (*refcount_ptr(self.slot)).fetch_sub(1, Release) };
        if prev != 1 {
            return;
        }
        fence(Acquire);
        // SAFETY: last reference — drop the value once, then return the slot.
        unsafe { drop_and_free_val::<T>(self.slot) };
    }
}

impl_handle_common_unsized!(Arc);
