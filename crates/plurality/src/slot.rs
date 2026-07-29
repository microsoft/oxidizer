// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "pointer-recovery and slot-lifecycle paths group tightly-coupled unsafe operations under a single documented safety invariant; one block per operation would duplicate that invariant and obscure it"
)]

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ptr::{NonNull, drop_in_place};

use crate::atomic::AtomicU32;

/// Free-list end-of-chain sentinel; the highest valid slot index is therefore
/// `u32::MAX - 1`.
pub(crate) const FREE_END: u32 = u32::MAX;

/// Largest pool whose live-allocation count plus the pool owner fits in
/// `AtomicUsize`, while every slot still has a non-sentinel `u32` index.
#[cfg(target_pointer_width = "64")]
pub(crate) const MAX_POOL_SLOTS: u64 = FREE_END as u64;

/// On narrow-pointer targets, the pool lifetime refcount is the tighter bound.
#[cfg(not(target_pointer_width = "64"))]
pub(crate) const MAX_POOL_SLOTS: u64 = (usize::MAX as u64).saturating_sub(1);

/// Refcount overflow guard, mirroring `alloc::sync::Arc`.
const MAX_REFCOUNT: u32 = i32::MAX as u32;

/// Aborts before the refcount exceeds [`MAX_REFCOUNT`].
#[inline]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)] // Overflow branch needs `old >= i32::MAX`, which a test cannot produce.
pub(crate) fn check_refcount_overflow(old: u32) {
    if old >= MAX_REFCOUNT {
        abort();
    }
}

/// Aborts on overflow via a double panic, including in `no_std`.
#[cold]
#[inline(never)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[expect(clippy::panic, reason = "deliberate double-panic to abort on refcount overflow in no_std")]
fn abort() -> ! {
    struct Bomb;
    impl Drop for Bomb {
        #[cfg_attr(test, mutants::skip)] // Unreachable without a `> i32::MAX` refcount; aborts the process, so a test can neither trigger nor catch it.
        fn drop(&mut self) {
            panic!("plurality: refcount overflow");
        }
    }
    let _bomb = Bomb;
    panic!("plurality: refcount overflow");
}

/// A single slot in a chunk: storage for one `T`, an atomic refcount that
/// doubles as the free-list link when the slot is free, and the slot's
/// immutable in-chunk index.
#[repr(C)]
pub(crate) struct SlotCell<T> {
    /// The value. Initialized only while the slot is occupied.
    pub(crate) value: UnsafeCell<MaybeUninit<T>>,
    /// Dual-role:
    /// - Occupied: the reference count (`>= 1`) for a shared handle (`Arc`/`Rc`);
    ///   `Box` and `Alloc` neither write nor read it, so their unique-owner
    ///   paths leave the stale free-list value in place — `push_free`
    ///   overwrites it on drop.
    /// - Free: the next free *global* index, or [`FREE_END`].
    ///
    /// `Arc` accesses this atomically; `Rc` accesses it non-atomically via
    /// `AtomicU32::as_ptr()` (sound because `Rc` is `!Send`). The free-list
    /// protocol always accesses it atomically.
    pub(crate) refcount: AtomicU32,
    /// Immutable in-chunk index (`0..N`), written once at chunk init. Drives the
    /// pointer-to-chunk-header recovery in [`crate::chunk`].
    pub(crate) index: u32,
}

/// Value accessors centralizing the raw-pointer access into the slot's
/// `UnsafeCell<MaybeUninit<T>>`, so every handle shares one audited `unsafe`
/// body.
impl<T> SlotCell<T> {
    /// Borrows the contained value.
    ///
    /// # Safety
    /// The slot must be occupied and its value initialized, and the borrow must
    /// respect Rust's aliasing rules for its returned lifetime.
    #[inline]
    pub(crate) unsafe fn value_ref<'a>(slot: NonNull<Self>) -> &'a T {
        // SAFETY: caller guarantees an occupied, initialized slot.
        unsafe { &*(*(*slot.as_ptr()).value.get()).as_ptr() }
    }

    /// Exclusively borrows the contained value.
    ///
    /// # Safety
    /// The slot must be occupied and initialized, and the caller must hold
    /// exclusive access to it for the returned lifetime.
    #[inline]
    pub(crate) unsafe fn value_mut<'a>(slot: NonNull<Self>) -> &'a mut T {
        // SAFETY: caller guarantees exclusive access to an initialized slot.
        unsafe { &mut *(*(*slot.as_ptr()).value.get()).as_mut_ptr() }
    }

    /// Writes a value into a freshly popped (exclusively owned) slot.
    ///
    /// # Safety
    /// The slot must have just been popped off the free list, so its value
    /// storage is logically uninitialized and exclusively owned.
    #[inline]
    pub(crate) unsafe fn write_value(slot: NonNull<Self>, value: T) {
        // SAFETY: caller guarantees exclusive ownership of an uninitialized slot.
        unsafe { (*(*slot.as_ptr()).value.get()).write(value) };
    }

    /// Runs the contained value's destructor in place.
    ///
    /// # Safety
    /// The slot must be occupied and initialized, and its value must not be
    /// accessed afterwards.
    #[inline]
    pub(crate) unsafe fn drop_value(slot: NonNull<Self>) {
        // SAFETY: caller guarantees an occupied, initialized slot dropped once.
        unsafe { drop_in_place((*(*slot.as_ptr()).value.get()).as_mut_ptr()) };
    }
}
