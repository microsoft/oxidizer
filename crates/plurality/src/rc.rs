// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ops::Deref;
use core::pin::Pin;
use core::ptr::NonNull;

use allocator_api2::alloc::{Allocator, Global};

use crate::common::impl_handle_common;
use crate::pool::drop_and_free;
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
/// the last `Rc` drops. `Rc` may outlive the `Pool` handle.
pub struct Rc<T, A: Allocator = Global> {
    slot: NonNull<SlotCell<T>>,
    _marker: PhantomData<(T, A)>,
}

// `Rc` is `!Send + !Sync` automatically: its only field is a
// `NonNull<SlotCell<T>>`, and raw pointers are neither `Send` nor `Sync`. We add
// no `unsafe impl Send`/`Sync`, so no explicit negative impl is needed; a
// compile-time `assert_not_impl_any!` in `tests/smart_ptr.rs` locks the property
// in against future refactors.
//
// Being `!Send + !Sync` is what makes a non-atomic refcount sound: an occupied
// slot is never on the free list, and a single-threaded handle has exclusive
// access to it, so the count can never be reached from another thread.
//
// The count nevertheless lives in an `AtomicU32`, because that field is shared
// storage with other roles: while the slot is free it holds the free-list link,
// and the (cross-thread) `Arc` path and the free-list protocol access it
// atomically. The `Rc` helpers below sidestep those atomics by reaching the
// integer directly through `AtomicU32::as_ptr()` and doing plain, non-atomic
// increments/decrements.
//
// `loom` builds are the one exception. Under `--cfg loom` the atomic is
// `loom::sync::atomic::AtomicU32`, loom's instrumented model used only by the
// concurrency tests. It deliberately has no `as_ptr()`, because loom must see
// every access through its own API to explore thread interleavings — a raw
// pointer write would be invisible to it. So the loom variants of these helpers
// fall back to loom's `fetch_add`/`fetch_sub`/`load` with `Relaxed` ordering.
// This does not weaken shipped code (loom builds are never released), and since
// `Rc` access is single-threaded the relaxed atomic behaves exactly like the
// non-atomic op it stands in for.

#[cfg(not(loom))]
#[inline]
unsafe fn inc_count<T>(slot: NonNull<SlotCell<T>>) -> u32 {
    // SAFETY: exclusive single-thread access to an occupied slot's refcount.
    unsafe {
        let p = (*slot.as_ptr()).refcount.as_ptr();
        let old = *p;
        *p = old.wrapping_add(1);
        old
    }
}

#[cfg(loom)]
#[inline]
unsafe fn inc_count<T>(slot: NonNull<SlotCell<T>>) -> u32 {
    use crate::atomic::Ordering::Relaxed;
    // SAFETY: occupied slot; single-thread access.
    unsafe { (*slot.as_ptr()).refcount.fetch_add(1, Relaxed) }
}

#[cfg(not(loom))]
#[inline]
unsafe fn dec_count<T>(slot: NonNull<SlotCell<T>>) -> u32 {
    // SAFETY: exclusive single-thread access to an occupied slot's refcount.
    unsafe {
        let p = (*slot.as_ptr()).refcount.as_ptr();
        let old = *p;
        *p = old.wrapping_sub(1);
        old
    }
}

#[cfg(loom)]
#[inline]
unsafe fn dec_count<T>(slot: NonNull<SlotCell<T>>) -> u32 {
    use crate::atomic::Ordering::Relaxed;
    // SAFETY: occupied slot; single-thread access.
    unsafe { (*slot.as_ptr()).refcount.fetch_sub(1, Relaxed) }
}

#[cfg(not(loom))]
#[inline]
unsafe fn read_count<T>(slot: NonNull<SlotCell<T>>) -> u32 {
    // SAFETY: exclusive single-thread access to an occupied slot's refcount.
    unsafe { *(*slot.as_ptr()).refcount.as_ptr() }
}

#[cfg(loom)]
#[inline]
unsafe fn read_count<T>(slot: NonNull<SlotCell<T>>) -> u32 {
    use crate::atomic::Ordering::Relaxed;
    // SAFETY: occupied slot; single-thread access.
    unsafe { (*slot.as_ptr()).refcount.load(Relaxed) }
}

impl<T, A: Allocator> Rc<T, A> {
    #[inline]
    pub(crate) fn from_slot(slot: NonNull<SlotCell<T>>) -> Self {
        Self {
            slot,
            _marker: PhantomData,
        }
    }

    /// Returns a mutable reference to the value if this `Rc` is the only handle
    /// to its slot, otherwise `None`. Mirrors [`alloc::rc::Rc::get_mut`].
    #[must_use]
    pub fn get_mut(this: &mut Self) -> Option<&mut T> {
        // SAFETY: occupied slot; `Rc` is single-threaded.
        let unique = unsafe { read_count(this.slot) == 1 };
        // SAFETY: a unique handle, so `&mut` to the slot value is exclusive.
        unique.then(|| unsafe { SlotCell::value_mut(this.slot) })
    }

    /// Returns `true` if both handles point to the same slot (the same
    /// allocation). Mirrors [`alloc::rc::Rc::ptr_eq`].
    #[must_use]
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        a.slot == b.slot
    }
}

impl<T, A: Allocator> Rc<MaybeUninit<T>, A> {
    /// Converts an uninitialized rc into an initialized one.
    ///
    /// # Safety
    /// The value must have been fully initialized before calling. If other `Rc`
    /// clones to this slot exist, they must observe an initialized value.
    #[must_use]
    pub unsafe fn assume_init(self) -> Rc<T, A> {
        let slot = self.slot.cast::<SlotCell<T>>();
        // Don't run the uninit handle's destructor; transfer the slot as-is.
        core::mem::forget(self);
        Rc::from_slot(slot)
    }

    /// Converts a pinned, uninitialized rc into a pinned, initialized one.
    ///
    /// # Safety
    /// The value must have been fully initialized before calling. If other `Rc`
    /// clones to this slot exist, they must observe an initialized value.
    #[must_use]
    pub unsafe fn assume_init_pin(this: Pin<Self>) -> Pin<Rc<T, A>> {
        // SAFETY: the caller guarantees initialization; the slot address is
        // unchanged, so re-pinning is sound.
        unsafe {
            let inner = Pin::into_inner_unchecked(this);
            Pin::new_unchecked(inner.assume_init())
        }
    }
}

impl<T, A: Allocator> Clone for Rc<T, A> {
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: `Rc` is `!Send + !Sync`, so this occupied slot's refcount is
        // exclusive to this thread; the non-atomic increment is sound.
        let old = unsafe { inc_count(self.slot) };
        check_refcount_overflow(old);
        Self::from_slot(self.slot)
    }
}

impl<T, A: Allocator> Deref for Rc<T, A> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the slot is occupied for as long as this `Rc` is alive.
        unsafe { SlotCell::value_ref(self.slot) }
    }
}

impl<T, A: Allocator> Drop for Rc<T, A> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: exclusive single-thread access; non-atomic decrement is sound.
        let prev = unsafe { dec_count(self.slot) };
        if prev != 1 {
            return;
        }
        // SAFETY: last reference — drop the value once, then return the slot.
        // `free_slot` publishes the link with an atomic `Release`, so the next
        // (atomic) reader sees a coherent slot.
        unsafe { drop_and_free::<T, A>(self.slot) };
    }
}

impl_handle_common!(Rc);
