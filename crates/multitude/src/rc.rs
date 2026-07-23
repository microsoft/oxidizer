// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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

/// A single-thread, non-atomic smart pointer to an arena-backed `T`.
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
/// # Abort
///
/// Cloning aborts the process if the strong reference count reaches its
/// saturation guard. This prevents reference-count wraparound from allowing
/// the allocation to be freed while handles remain live.
///
/// # Pinning
///
/// Use [`Arena::alloc_rc_pin`](crate::Arena::alloc_rc_pin) to pin a `!Unpin`
/// value during construction. [`Pin::new`] can wrap an existing owner only when
/// `T: Unpin`. Multitude provides no safe conversion for an existing owner of a
/// `!Unpin` value because an ordinary alias may later become unique through
/// [`Rc::get_mut`].
///
/// ```compile_fail
/// use core::marker::PhantomPinned;
/// use core::pin::Pin;
/// use multitude::{Arena, Rc};
///
/// let arena = Arena::new();
/// let value = arena.alloc_rc(PhantomPinned);
/// let _: Pin<Rc<PhantomPinned>> = value.into();
/// ```
///
/// ```compile_fail
/// use core::marker::PhantomPinned;
/// use core::pin::Pin;
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let _ = Pin::new(arena.alloc_rc(PhantomPinned));
/// ```
///
/// Multitude intentionally provides no `assume_init_pin` conversion for
/// `Pin<Rc<MaybeUninit<T>>>`. Initialize first; an address-sensitive value
/// still cannot then be pinned through an existing shared owner:
///
/// ```compile_fail
/// use core::marker::PhantomPinned;
/// use core::pin::Pin;
/// use multitude::{Arena, Rc};
///
/// let arena = Arena::new();
/// let value = arena.alloc_zeroed_rc::<PhantomPinned>();
/// // SAFETY: PhantomPinned is an inhabited zero-sized type.
/// let value: Rc<PhantomPinned> = unsafe { value.assume_init() };
/// let _: Pin<Rc<PhantomPinned>> = Pin::new(value);
/// ```
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

    /// Pins a freshly-created owner before any ordinary alias can escape.
    ///
    /// # Safety
    ///
    /// No unpinned alias to this allocation may exist or be created.
    #[inline]
    pub(crate) unsafe fn pin_fresh(this: Self) -> Pin<Self> {
        // SAFETY: guaranteed by the caller.
        unsafe { Pin::new_unchecked(this) }
    }

    /// Returns mutable access when this is the only strong owner.
    ///
    /// ```
    /// use multitude::{Arena, Rc};
    ///
    /// let arena = Arena::new();
    /// let mut value = arena.alloc_rc(1_u32);
    /// *Rc::get_mut(&mut value).expect("the owner is unique") = 2;
    /// assert_eq!(*value, 2);
    /// ```
    #[inline]
    pub fn get_mut(this: &mut Self) -> Option<&mut T> {
        let value_align = mem::align_of_val::<T>(&**this);
        // SAFETY: `this` keeps the unaligned strong-count prefix alive.
        let strong = unsafe { thin_dst::local_strong_ptr::<T>(this.ptr, value_align) };
        // SAFETY: `Rc` is single-threaded and the count slot is live.
        if unsafe { ptr::read_unaligned(strong) } != 1 {
            return None;
        }
        let mut value = this.as_fat_ptr();
        // SAFETY: a strong count of one means no other owner can expose the
        // pointee, and no weak-owner API exists.
        Some(unsafe { value.as_mut() })
    }

    /// True iff both handles point at the same address.
    ///
    /// ```
    /// use multitude::{Arena, Rc};
    ///
    /// let arena = Arena::new();
    /// let first = arena.alloc_rc(7_u32);
    /// let clone = first.clone();
    /// let other = arena.alloc_rc(7_u32);
    /// assert!(Rc::ptr_eq(&first, &clone));
    /// assert!(!Rc::ptr_eq(&first, &other));
    /// ```
    #[inline]
    #[must_use]
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        ptr::addr_eq(a.ptr.as_ptr(), b.ptr.as_ptr())
    }
}

impl_thin_smart_ptr_common!(Rc);

impl<T, A: Allocator + Clone> Rc<MaybeUninit<T>, A> {
    /// Convert an initialized `MaybeUninit<T>` handle into a `T` handle.
    ///
    /// This is O(1) — no copy or allocation.
    ///
    /// # Safety
    ///
    /// The `MaybeUninit<T>` must contain a fully-initialized, valid `T`.
    ///
    /// If this allocation has clones, every clone must be converted before
    /// the last handle drops, or a converted `Rc<T>` must be the last handle.
    /// Otherwise the last `Rc<MaybeUninit<T>>` drops no `T` and leaks it.
    ///
    /// ```
    /// use multitude::{Arena, Rc};
    ///
    /// let arena = Arena::new();
    /// let value = arena.alloc_zeroed_rc::<u32>();
    /// // SAFETY: zero is a valid `u32` representation.
    /// let value: Rc<u32> = unsafe { value.assume_init() };
    /// assert_eq!(*value, 0);
    /// ```
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
}

impl<T, A: Allocator + Clone> Rc<[MaybeUninit<T>], A> {
    /// Convert an initialized `Rc<[MaybeUninit<T>]>` into an `Rc<[T]>`. O(1).
    ///
    /// # Safety
    ///
    /// Every element of the slice must contain a fully-initialized, valid `T`.
    ///
    /// If this allocation has clones, every clone must be converted before
    /// the last handle drops, or a converted `Rc<[T]>` must be the last
    /// handle. Otherwise the last `Rc<[MaybeUninit<T>]>` drops no elements
    /// and leaks them.
    ///
    /// ```
    /// use multitude::{Arena, Rc};
    ///
    /// let arena = Arena::new();
    /// let values = arena.alloc_zeroed_slice_rc::<u16>(3);
    /// // SAFETY: zero is a valid `u16` representation.
    /// let values: Rc<[u16]> = unsafe { values.assume_init() };
    /// assert_eq!(&*values, &[0, 0, 0]);
    /// ```
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
        //
        // Store before the cold saturation check so the backend can fuse the
        // update into one memory increment. This reduces the clone benchmark
        // from 16,043 to 13,043 instructions (18.7%).
        unsafe {
            let next = ptr::read_unaligned(strong).wrapping_add(1);
            ptr::write_unaligned(strong, next);
            if next > MAX_STRONG_REFCOUNT {
                // Tests model process abort as a catchable panic, so restore
                // the count before unwinding. Production never returns.
                #[cfg(test)]
                ptr::write_unaligned(strong, next.wrapping_sub(1));
                strong_overflow_abort();
            }
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
        // Decrement unconditionally and detect the last reference by the zero
        // result, mirroring `Arc`'s single fused `dec`. This lets the backend
        // fold the unaligned load/sub/store into one memory decrement instead
        // of a read + compare that leaves an extra instruction on the
        // last-drop path. Writing `0` on the final drop is harmless: the slot
        // is reclaimed immediately below and never read again.
        // SAFETY: non-atomic, single-thread unaligned read-modify-write.
        let previous = unsafe { ptr::read_unaligned(strong) };
        debug_assert!(previous != 0, "a live Rc handle must have a nonzero strong count");
        let next = previous - 1;
        // SAFETY: same slot; the count stays valid (positive until the final
        // drop, then `0`).
        unsafe { ptr::write_unaligned(strong, next) };
        if next != 0 {
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

// The non-atomic strong count makes `Rc` intentionally `!Send` and `!Sync`.

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

    #[test]
    fn clone_at_max_refcount_threshold_preserves_count() {
        let arena = Arena::new();
        let rc = arena.alloc_rc(0xABCD_u32);
        // SAFETY: `rc` keeps the value and its strong-count prefix live.
        let strong = unsafe { thin_dst::local_strong_ptr::<u32>(rc.thin_ptr(), mem::align_of::<u32>()) };
        // SAFETY: unaligned, single-thread access.
        unsafe { ptr::write_unaligned(strong, MAX_STRONG_REFCOUNT) };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _c = rc.clone();
        }));
        let _panic = result.expect_err("clone at the threshold must panic");
        // SAFETY: unaligned, single-thread access while `rc` keeps the prefix live.
        assert_eq!(unsafe { ptr::read_unaligned(strong) }, MAX_STRONG_REFCOUNT);
        // SAFETY: restore the real live-handle count before teardown.
        unsafe { ptr::write_unaligned(strong, 1) };
    }
}
