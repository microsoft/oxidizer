// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    clippy::type_repetition_in_bounds,
    reason = "trait-impl `where` clauses are kept uniform across all forwarding impls"
)]

use core::marker::PhantomData;
use core::mem::{self, MaybeUninit};
use core::pin::Pin;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::{Allocator, Global};
use ptr_meta::Pointee;

use crate::internal::chunk_ref::ChunkRef;
use crate::internal::constants::refcount_overflow_abort;
use crate::internal::thin_dst;
use crate::thin_smart_ptr_common::impl_thin_smart_ptr_common;
use crate::vec::Vec;

/// Strong-count saturation threshold. Cloning past this aborts the
/// process, mirroring `std::rc::Rc`'s `MAX_REFCOUNT` guard (using the
/// `u32` strong counter's half-range instead of `isize::MAX`).
const MAX_STRONG_REFCOUNT: u32 = u32::MAX >> 1;

/// A **single-thread**, non-atomic reference-counted smart pointer to a `T`
/// stored in an [`Arena`](crate::Arena).
///
/// `Rc` is the [`!Send`](Send)/[`!Sync`](Sync) sibling of [`Arc`](crate::Arc):
/// it shares the same 8-byte thin-pointer layout and the same ability to outlive
/// the arena, but its reference count is non-atomic. Compared to
/// [`Arc`](crate::Arc):
///
/// - **Cheaper clone/drop** — a non-atomic increment/decrement instead of an
///   atomic operation.
/// - **No `Send`/`Sync` bound on `T`** — `Rc` can wrap thread-affine,
///   `!Send`/`!Sync` values (e.g. `Rc<RefCell<…>>`) and still be shared (by
///   cloning) within a single thread and outlive the arena.
/// - **Tighter packing** — for `str` / `[u8]` and other sub-4-aligned payloads,
///   `Rc` uses a few bytes less than [`Arc`](crate::Arc).
///
/// Created via [`Arena::alloc_rc`](crate::Arena::alloc_rc). Cloning is **O(1)**.
/// The value survives [`Arena::reset`](crate::Arena::reset) and the `Rc` can
/// outlive the arena; `T::drop` runs eagerly when the last `Rc` clone is dropped.
///
/// # Pinning
///
/// `Rc` implements [`Unpin`] unconditionally (like `std::rc::Rc`).
///
/// # Example
///
/// ```
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let a = arena.alloc_rc(42_u32);
/// let b = a.clone();
/// assert_eq!(*a, *b);
/// ```
pub struct Rc<T: ?Sized + Pointee, A: Allocator + Clone = Global> {
    /// **Thin** pointer to the first byte of the contained value (see
    /// [`Arc`](crate::Arc) for the masking / metadata-prefix scheme). The
    /// strong count — an unaligned, non-atomic `u32` — sits in the prefix
    /// immediately before any metadata, and is read/written only through
    /// [`ptr::read_unaligned`] / [`ptr::write_unaligned`].
    ptr: NonNull<u8>,
    /// Variance + dropck marker. `NonNull<u8>` + the raw-pointer phantom keep
    /// `Rc` `!Send`/`!Sync` automatically (no `unsafe impl` is added).
    _phantom: PhantomData<(*const T, A)>,
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> Rc<T, A> {
    /// Builds an `Rc` from a thin payload pointer.
    ///
    /// # Safety
    ///
    /// - `thin` must reference the payload of a fully-initialized `T` whose
    ///   storage was bump-allocated from a [`Chunk<A>`](crate::internal::chunk::Chunk)
    ///   via the [`LocalStrong`](crate::internal::thin_dst::LocalStrong) allocator
    ///   path: a non-atomic `u32` strong count must already be initialized in the
    ///   chunk prefix, and for DST `T` the prefix must carry the matching
    ///   `T::Metadata`.
    /// - The caller must have just acquired a +1 refcount on that chunk for the
    ///   new `Rc` family, and the strong count must account for this handle.
    /// - `thin` must lie within the first `CHUNK_ALIGN` bytes of the chunk.
    #[inline]
    pub(crate) unsafe fn from_raw(thin: NonNull<u8>) -> Self {
        Self {
            ptr: thin,
            _phantom: PhantomData,
        }
    }

    /// Returns the thin chunk pointer (see [`crate::Arc::thin_ptr`]).
    #[inline]
    pub(crate) fn thin_ptr(&self) -> NonNull<u8> {
        self.ptr
    }

    /// True iff both handles point at the same address.
    #[inline]
    #[must_use]
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        ptr::addr_eq(a.ptr.as_ptr(), b.ptr.as_ptr())
    }
}

impl_thin_smart_ptr_common!(Rc);

impl<T, A: Allocator + Clone> Rc<MaybeUninit<T>, A> {
    /// Convert a handle to `MaybeUninit<T>` whose value is now initialized into
    /// a handle to `T`. O(1) — no copy or alloc.
    ///
    /// # Safety
    ///
    /// The `MaybeUninit<T>` must contain a fully-initialized, valid `T`.
    #[inline]
    #[must_use]
    pub unsafe fn assume_init(self) -> Rc<T, A> {
        let thin = self.ptr;
        mem::forget(self);
        // SAFETY: the caller guarantees the `MaybeUninit<T>` holds an
        // initialized, valid `T`. `MaybeUninit<T>` and `T` share the same
        // strong-prefix chunk layout, so the thin pointer (whose chunk `+1` is
        // transferred via `mem::forget(self)`) reconstructs a valid `Rc<T>`.
        unsafe { Rc::from_raw(thin) }
    }

    /// Pinned mirror of [`Self::assume_init`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::assume_init`].
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin(this: Pin<Self>) -> Pin<Rc<T, A>>
    where
        A: 'static,
    {
        // SAFETY: `Pin::into_inner_unchecked` is sound because we immediately
        // re-pin the result; the caller's `assume_init` contract (every
        // `MaybeUninit<T>` is initialized) is forwarded unchanged, and the
        // pointee never moves.
        unsafe {
            let inner: Self = Pin::into_inner_unchecked(this);
            Rc::into_pin(inner.assume_init())
        }
    }
}

impl<T, A: Allocator + Clone> Rc<[MaybeUninit<T>], A> {
    /// Convert an initialized `Rc<[MaybeUninit<T>]>` into an `Rc<[T]>`. O(1).
    ///
    /// # Safety
    ///
    /// Every element of the slice must contain a fully-initialized, valid `T`.
    #[inline]
    #[must_use]
    pub unsafe fn assume_init(self) -> Rc<[T], A> {
        let thin = self.ptr;
        mem::forget(self);
        // SAFETY: the caller guarantees every element is an initialized, valid
        // `T`. `[MaybeUninit<T>]` and `[T]` share the same strong-prefix chunk
        // layout and length metadata, so the thin pointer (whose chunk `+1` is
        // transferred via `mem::forget(self)`) reconstructs a valid `Rc<[T]>`.
        unsafe { Rc::from_raw(thin) }
    }

    /// Pinned mirror of [`Self::assume_init`] for slices.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::assume_init`].
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin_slice(this: Pin<Self>) -> Pin<Rc<[T], A>>
    where
        A: 'static,
    {
        // SAFETY: `Pin::into_inner_unchecked` is sound because we immediately
        // re-pin the result; the caller's slice `assume_init` contract (every
        // element is initialized) is forwarded unchanged, and the pointee never
        // moves.
        unsafe {
            let inner: Self = Pin::into_inner_unchecked(this);
            Rc::into_pin(inner.assume_init())
        }
    }
}

/// Saturation guard for [`Rc::clone`]: aborts the process when the strong count
/// would overflow, mirroring `std::rc::Rc`.
#[cfg_attr(coverage_nightly, coverage(off))]
#[inline(never)]
#[cold]
fn strong_overflow_abort() -> ! {
    refcount_overflow_abort()
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> Clone for Rc<T, A> {
    #[inline]
    fn clone(&self) -> Self {
        let value_align = mem::align_of_val::<T>(&**self);
        // SAFETY: `self` keeps the value (and its strong-count prefix) alive, so
        // the unaligned count slot is live and within the chunk's provenance.
        let strong = unsafe { thin_dst::local_strong_ptr::<T>(self.ptr, value_align) };
        // SAFETY: non-atomic, single-thread; the slot may be unaligned, so use
        // unaligned load/store. `Rc` is `!Send`/`!Sync`, so no other thread can
        // race this read-modify-write.
        unsafe {
            let prev = ptr::read_unaligned(strong);
            // `>=`: aborting when the count is *at* the threshold guarantees the
            // post-increment value never exceeds `MAX_STRONG_REFCOUNT`. (Unlike
            // `Arc`, whose atomic `fetch_add` post-increments before the check,
            // `Rc` reads first and can refuse the increment outright.)
            if prev >= MAX_STRONG_REFCOUNT {
                strong_overflow_abort();
            }
            ptr::write_unaligned(strong, prev + 1);
        }
        Self {
            ptr: self.ptr,
            _phantom: PhantomData,
        }
    }
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> Drop for Rc<T, A> {
    #[inline]
    fn drop(&mut self) {
        let value_align = mem::align_of_val::<T>(&**self);
        // SAFETY: the value (and its strong-count prefix) is still live while
        // this handle exists; the unaligned count slot is within chunk
        // provenance.
        let strong = unsafe { thin_dst::local_strong_ptr::<T>(self.ptr, value_align) };
        // SAFETY: non-atomic, single-thread unaligned read-modify-write.
        let prev = unsafe { ptr::read_unaligned(strong) };
        if prev != 1 {
            // SAFETY: `prev >= 2`, so decrementing keeps the count positive.
            unsafe { ptr::write_unaligned(strong, prev - 1) };
            return;
        }
        // Last strong reference. No fence is needed (single-thread). Adopt the
        // chunk's +1 *before* `T::drop` so a panicking destructor still releases
        // the chunk via `ChunkRef`'s `Drop`.
        //
        // SAFETY: `ptr` is hosted in a 64K-aligned `Chunk` holding exactly one
        // outstanding +1 for this whole allocation; `from_value_ptr` adopts it.
        // The value is a valid `T` and is dropped exactly once (on the strong →
        // 0 transition).
        unsafe {
            let _chunk: ChunkRef<A> = ChunkRef::from_value_ptr(self.ptr);
            let fat = self.as_fat_ptr();
            ptr::drop_in_place(fat.as_ptr());
        }
    }
}

// NOTE: `Rc` is intentionally `!Send` and `!Sync` (no `unsafe impl`s): its
// strong count is non-atomic, so handles must never cross threads.

impl<'a, T, A: Allocator + Clone> From<Vec<'a, T, A>> for Rc<[T], A> {
    /// Freeze a [`Vec`](crate::vec::Vec) into an immutable
    /// [`Rc<[T], A>`](crate::Rc). Mirrors `std`'s `From<Vec<T>> for Rc<[T]>`.
    ///
    /// Generally **O(1)** (reuses the existing storage with no copy), except in
    /// rare edge cases where it falls back to an **O(n)** element move.
    #[inline]
    fn from(v: Vec<'a, T, A>) -> Self {
        v.into_rc_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Arena;

    #[test]
    fn max_strong_refcount_is_u32_half_range() {
        assert_eq!(MAX_STRONG_REFCOUNT, u32::MAX >> 1);
        assert_eq!(MAX_STRONG_REFCOUNT, 0x7FFF_FFFF);
    }

    // A clone observing `prev == MAX_STRONG_REFCOUNT - 1` (the last value below
    // the threshold) must NOT abort: it increments to exactly the threshold.
    #[test]
    fn clone_below_max_refcount_threshold_does_not_abort() {
        let arena = Arena::new();
        let rc = arena.alloc_rc(0xABCD_u32);
        // SAFETY: `rc` keeps the value and its strong-count prefix live.
        let strong = unsafe { thin_dst::local_strong_ptr::<u32>(rc.thin_ptr(), mem::align_of::<u32>()) };
        // SAFETY: unaligned, single-thread access.
        unsafe { ptr::write_unaligned(strong, MAX_STRONG_REFCOUNT - 1) };
        #[expect(clippy::redundant_clone, reason = "exercising the overflow guard at the threshold is the point")]
        let clone = rc.clone();
        assert_eq!(*clone, 0xABCD);
        // Restore the true live-handle count so the two drops tear down cleanly.
        // SAFETY: `strong` points at `rc`'s live, single-thread strong-count
        // prefix; the unaligned write restores the true live-handle count (2).
        unsafe { ptr::write_unaligned(strong, 2) };
    }

    // A clone observing `prev == MAX_STRONG_REFCOUNT` (the threshold) MUST abort
    // (panics under cfg(test)): incrementing would push the count past it.
    #[test]
    #[should_panic(expected = "refcount overflow")]
    fn clone_at_max_refcount_threshold_aborts() {
        let arena = Arena::new();
        let rc = arena.alloc_rc(0xABCD_u32);
        // SAFETY: `rc` keeps the value and its strong-count prefix live.
        let strong = unsafe { thin_dst::local_strong_ptr::<u32>(rc.thin_ptr(), mem::align_of::<u32>()) };
        // SAFETY: unaligned, single-thread access.
        unsafe { ptr::write_unaligned(strong, MAX_STRONG_REFCOUNT) };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _c = rc.clone();
        }));
        // SAFETY: restore the real live-handle count before resuming.
        unsafe { ptr::write_unaligned(strong, 1) };
        std::panic::resume_unwind(result.expect_err("clone at the threshold must panic"));
    }
}
