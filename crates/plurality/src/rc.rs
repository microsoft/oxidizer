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

use crate::atomic::AtomicU32;
use crate::coerce::{self, Coercion};
use crate::common::impl_handle_common_unsized;
use crate::pool::{drop_and_free_val, refcount_ptr};
use crate::slot::{SlotCell, check_refcount_overflow};

/// A shared, **non-atomically** reference-counted handle to a value in a
/// [`Pool`](crate::Pool).
///
/// `Rc` is `Arc` without the atomic refcount: cloning and dropping use plain
/// integer increments/decrements instead of locked atomics, which is cheaper for
/// clone/drop-heavy single-threaded sharing (shared-subtree ASTs, DAGs). The
/// cost is that `Rc` is `!Send + !Sync` — it can never leave its thread.
///
/// Derefs to `&T` (read-only); the value is dropped and the slot returned when
/// the last `Rc` drops. `Rc` may outlive the `Pool` handle. Like [`Box`](crate::Box),
/// it is generic over `T: ?Sized` and can share an unsized value via [`Rc::unsize`].
///
/// Pinning follows [`alloc::rc::Rc::pin`]'s construction-time model. Use
/// [`Pool::alloc_rc_pin`](crate::Pool::alloc_rc_pin) for `!Unpin` values.
/// [`Pin::new`] can wrap an existing owner only when `T: Unpin`; Plurality
/// provides no safe conversion for an existing owner of a `!Unpin` value after
/// an ordinary alias may have escaped.
pub struct Rc<T: ?Sized, A: Allocator = Global> {
    /// Pointer to the **value** (field 0 of its `SlotCell<T>`); a fat pointer for
    /// unsized `T`. The refcount is recovered from the value's size.
    slot: NonNull<T>,
    _marker: PhantomData<A>,
    _not_send_sync: PhantomData<alloc::rc::Rc<()>>,
}

// The `alloc::rc::Rc` marker makes `Rc` unconditionally `!Send + !Sync`, which
// permits non-atomic refcount access while occupied. The same field is atomic
// storage because free slots use it as a cross-thread free-list link.
// Loom lacks `AtomicU32::as_ptr`, so model builds use instrumented relaxed
// operations; single-threaded access gives them the same semantics here.

#[cfg(not(loom))]
#[inline]
unsafe fn inc_count(refcount: *mut AtomicU32) -> u32 {
    // SAFETY: exclusive single-thread access to an occupied slot's refcount.
    unsafe {
        let p = (*refcount).as_ptr();
        let old = *p;
        *p = old.wrapping_add(1);
        old
    }
}

#[cfg(loom)]
#[inline]
unsafe fn inc_count(refcount: *mut AtomicU32) -> u32 {
    use crate::atomic::Ordering::Relaxed;
    // SAFETY: occupied slot; single-thread access.
    unsafe { (*refcount).fetch_add(1, Relaxed) }
}

#[cfg(not(loom))]
#[inline]
unsafe fn dec_count(refcount: *mut AtomicU32) -> u32 {
    // SAFETY: exclusive single-thread access to an occupied slot's refcount.
    unsafe {
        let p = (*refcount).as_ptr();
        let old = *p;
        *p = old.wrapping_sub(1);
        old
    }
}

#[cfg(loom)]
#[inline]
unsafe fn dec_count(refcount: *mut AtomicU32) -> u32 {
    use crate::atomic::Ordering::Relaxed;
    // SAFETY: occupied slot; single-thread access.
    unsafe { (*refcount).fetch_sub(1, Relaxed) }
}

#[cfg(not(loom))]
#[inline]
unsafe fn read_count(refcount: *mut AtomicU32) -> u32 {
    // SAFETY: exclusive single-thread access to an occupied slot's refcount.
    unsafe { *(*refcount).as_ptr() }
}

#[cfg(loom)]
#[inline]
unsafe fn read_count(refcount: *mut AtomicU32) -> u32 {
    use crate::atomic::Ordering::Relaxed;
    // SAFETY: occupied slot; single-thread access.
    unsafe { (*refcount).load(Relaxed) }
}

impl<T, A: Allocator> Rc<T, A> {
    #[inline]
    pub(crate) fn from_slot(slot: NonNull<SlotCell<T>>) -> Self {
        Self {
            // The value is field 0 of `SlotCell<T>`, so its address is the slot address.
            slot: slot.cast::<T>(),
            _marker: PhantomData,
            _not_send_sync: PhantomData,
        }
    }

    /// Erases a sized `Rc<T>` into an `Rc<U>` for an unsized `U` (e.g. a trait
    /// object or slice), transferring this handle's share of the slot.
    ///
    /// Like [`Box::unsize`](crate::Box::unsize), the
    /// [`Coercion`] token proves
    /// that the conversion preserves the slot's address and provenance:
    ///
    /// ```
    /// use core::fmt::Debug;
    ///
    /// use plurality::{Pool, Rc, coerce};
    ///
    /// let pool = Pool::<u32>::new();
    /// let r = pool.alloc_rc(7u32);
    /// let dyn_r: Rc<dyn Debug> = Rc::unsize::<dyn Debug>(r, coerce!(dyn Debug));
    /// assert_eq!(format!("{dyn_r:?}"), "7");
    /// ```
    ///
    /// The refcount is unchanged — other clones (which may still be `Rc<T>`) keep
    /// sharing the same slot; whichever handle drops last frees it.
    #[must_use]
    pub fn unsize<U: ?Sized>(this: Self, coercion: Coercion<T, U, impl FnOnce(*const T) -> *const U>) -> Rc<U, A> {
        let value = coerce::unsize(this.slot, coercion);
        // The returned handle inherits this handle's share of the slot.
        forget(this);
        Rc::from_value(value)
    }

    /// Erases a pinned sized owner while preserving its pinning guarantee.
    ///
    /// The allocation stays fixed and no ordinary owner is exposed.
    #[must_use]
    pub fn unsize_pin<U: ?Sized>(this: Pin<Self>, coercion: Coercion<T, U, impl FnOnce(*const T) -> *const U>) -> Pin<Rc<U, A>> {
        // SAFETY: the ordinary owner exists only inside this method. `unsize`
        // changes pointer metadata without moving or exposing the value, and
        // the resulting owner is re-pinned before it can escape.
        unsafe {
            let owner = Pin::into_inner_unchecked(this);
            let erased = Self::unsize(owner, coercion);
            Rc::into_pin_fresh(erased)
        }
    }
}

impl<T: ?Sized, A: Allocator> Rc<T, A> {
    #[inline]
    pub(crate) fn from_value(value: NonNull<T>) -> Self {
        Self {
            slot: value,
            _marker: PhantomData,
            _not_send_sync: PhantomData,
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

    /// Returns a mutable reference to the value if this `Rc` is the only handle
    /// to its slot, otherwise `None`. Mirrors [`alloc::rc::Rc::get_mut`].
    #[must_use]
    #[expect(
        clippy::unnecessary_lazy_evaluations,
        reason = "the unsafe deref must stay lazy: materializing `&mut T` when the handle is not unique would alias existing shared references"
    )]
    pub fn get_mut(this: &mut Self) -> Option<&mut T> {
        // SAFETY: occupied slot; `Rc` is single-threaded.
        let unique = unsafe { read_count(refcount_ptr(this.slot)) == 1 };
        // SAFETY: a unique handle, so `&mut` to the slot value is exclusive.
        unique.then(|| unsafe { this.slot.as_mut() })
    }

    /// Returns `true` if both handles point to the same slot (the same
    /// allocation). Mirrors [`alloc::rc::Rc::ptr_eq`].
    #[must_use]
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        addr_eq(a.slot.as_ptr(), b.slot.as_ptr())
    }
}

impl<T, A: Allocator> Rc<MaybeUninit<T>, A> {
    #[inline]
    pub(crate) fn write_value(&mut self, value: T) {
        // SAFETY: allocation APIs call this only for a freshly reserved,
        // uniquely owned uninitialized slot.
        unsafe { self.slot.as_mut().write(value) };
    }

    /// Converts an uninitialized rc into an initialized one.
    ///
    /// # Safety
    /// The value must have been fully initialized before calling. If other `Rc`
    /// clones to this slot exist, they must observe an initialized value.
    #[must_use]
    pub unsafe fn assume_init(self) -> Rc<T, A> {
        let value = self.slot.cast::<T>();
        // Don't run the uninit handle's destructor; transfer the slot as-is.
        forget(self);
        Rc::from_value(value)
    }
}

impl<T: ?Sized, A: Allocator> Clone for Rc<T, A> {
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: `Rc` is `!Send + !Sync`, so this occupied slot's refcount is
        // exclusive to this thread; the non-atomic increment is sound.
        let old = unsafe { inc_count(refcount_ptr(self.slot)) };
        check_refcount_overflow(old);
        Self::from_value(self.slot)
    }
}

impl<T: ?Sized, A: Allocator> Deref for Rc<T, A> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the slot is occupied for as long as this `Rc` is alive.
        unsafe { self.slot.as_ref() }
    }
}

impl<T: ?Sized, A: Allocator> Drop for Rc<T, A> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: exclusive single-thread access; non-atomic decrement is sound.
        let prev = unsafe { dec_count(refcount_ptr(self.slot)) };
        if prev != 1 {
            return;
        }
        // SAFETY: last reference — drop the value once, then return the slot.
        // `free_slot` publishes the link with an atomic `Release`, so the next
        // (atomic) reader sees a coherent slot.
        unsafe { drop_and_free_val::<T>(self.slot) };
    }
}

impl_handle_common_unsized!(Rc);
