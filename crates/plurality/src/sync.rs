// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "pointer-recovery and slot-lifecycle paths group tightly-coupled unsafe operations under a single documented safety invariant; one block per operation would duplicate that invariant and obscure it"
)]

use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ops::Deref;
use core::pin::Pin;
use core::ptr::NonNull;

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
    /// [`Coercion`](struct@Coercion) token proves
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
        core::mem::forget(this);
        Arc::from_value(value)
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
        core::ptr::addr_eq(a.slot.as_ptr(), b.slot.as_ptr())
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
        core::mem::forget(self);
        Arc::from_value(value)
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
        // SAFETY: the slot is occupied; we hold one of its references.
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
