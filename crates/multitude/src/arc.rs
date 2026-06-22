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
use core::sync::atomic::{Ordering, fence};

use allocator_api2::alloc::{Allocator, Global};
use ptr_meta::Pointee;

use crate::internal::chunk_ref::ChunkRef;
use crate::internal::constants::refcount_overflow_abort;
use crate::internal::thin_dst;
use crate::thin_smart_ptr_common::impl_thin_smart_ptr_common;
use crate::vec::Vec;

/// Strong-count saturation threshold. Cloning past this aborts the
/// process, mirroring `std::sync::Arc`'s `MAX_REFCOUNT` guard (using
/// the `u32` strong counter's half-range instead of `isize::MAX`).
const MAX_STRONG_REFCOUNT: u32 = u32::MAX >> 1;

/// A thread-safe reference-counted smart pointer to a `T` stored in an [`Arena`](crate::Arena).
///
/// Safe to share across threads when `T: Send + Sync`.
///
/// Created via [`Arena::alloc_arc`](crate::Arena::alloc_arc). Cloning is
/// **O(1)** and uses a single Relaxed atomic increment of the `Arc`'s
/// own strong count (matching `std::sync::Arc`). Dropping a clone is one
/// Release decrement plus, on the final dec to zero, an Acquire fence,
/// the value's destructor (`T::drop`), and the release of the chunk
/// reference.
///
/// Each `Arc` carries its own strong reference count — an
/// [`AtomicU32`](core::sync::atomic::AtomicU32) stored in the chunk's
/// payload immediately before the value. The allocation also holds
/// **one** refcount on its containing chunk for the whole `Arc` family
/// (all clones share it); that chunk reference is released only when the
/// last `Arc` drops. This keeps the value alive across
/// [`Arena::reset`](crate::Arena::reset) and lets the `Arc` outlive the
/// arena, while running `T::drop` eagerly on the last drop — so nested
/// `Arc`s (e.g. `Arc<[Arc<T>]>`) release their storage promptly instead
/// of deferring to chunk teardown.
///
/// # Pinning
///
/// `Arc` implements [`Unpin`] unconditionally (like `std::sync::Arc`).
///
/// # Example
///
/// ```
/// use std::thread;
///
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let a = arena.alloc_arc(42_u32);
/// let b = a.clone();
/// let h = thread::spawn(move || *b);
/// assert_eq!(*a, h.join().unwrap());
/// ```
pub struct Arc<T: ?Sized + Pointee, A: Allocator + Clone = Global> {
    /// **Thin** pointer to the first byte of the contained value, which
    /// lives in a 64K-aligned [`SharedChunk`](crate::internal::shared_chunk::SharedChunk)'s
    /// payload. The chunk header is recovered by masking, and `T`'s
    /// pointer metadata (if any — `()` for `T: Sized`, `usize` for
    /// slice DSTs / `str`, vtable for trait objects) is stored in the
    /// `size_of::<T::Metadata>()` bytes immediately preceding the
    /// payload (read with [`core::ptr::read_unaligned`]).
    ///
    /// This makes `Arc<T>` 8 bytes uniformly, even for DST `T`.
    ptr: NonNull<u8>,
    /// Variance + dropck marker. Send/Sync are gated by explicit
    /// unsafe impls below.
    _phantom: PhantomData<(*const T, A)>,
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> Arc<T, A> {
    /// Builds an `Arc` from a thin payload pointer.
    ///
    /// For DST `T`, the metadata is recovered on demand from the chunk
    /// prefix at `thin - size_of::<T::Metadata>()` via `as_fat_ptr`; the
    /// caller must have already written it there at allocation time.
    /// For `T: Sized`, the prefix is zero-sized and no metadata is
    /// stored.
    ///
    /// # Safety
    ///
    /// - `thin` must reference the payload of a fully-initialized `T`
    ///   whose storage was bump-allocated from a [`SharedChunk<A>`] via
    ///   the strong-prefixed `Arc` allocator path: a per-`Arc`
    ///   [`AtomicU32`](core::sync::atomic::AtomicU32) strong count must
    ///   already be initialized in the chunk prefix (see
    ///   [`thin_dst::strong_ref`](crate::internal::thin_dst::strong_ref)),
    ///   and for DST `T` the prefix must also carry the matching
    ///   `T::Metadata`.
    /// - The caller must have just acquired a +1 refcount on that chunk
    ///   for the new `Arc` family, and the strong count must account for
    ///   this handle; the returned `Arc` owns that strong reference and
    ///   releases the chunk +1 (plus runs `T::drop`) when the strong
    ///   count reaches zero.
    /// - `thin` must lie within the first `CHUNK_ALIGN` bytes of the
    ///   chunk so the header-from-mask helper recovers the chunk
    ///   address correctly.
    #[inline]
    pub(crate) unsafe fn from_raw(thin: NonNull<u8>) -> Self {
        Self {
            ptr: thin,
            _phantom: PhantomData,
        }
    }

    /// Returns the thin chunk pointer — the byte address of the
    /// value's payload inside its hosting chunk. Carries chunk-wide
    /// provenance (no `&T` narrowing). Used by string-flavored
    /// conversions in `strings/str_impls.rs` to retag between
    /// `Arc<str>` and `Arc<[u8]>` without losing the chunk-recovery
    /// borrow-stack tag the smart pointer's `Drop` walks back through.
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

impl_thin_smart_ptr_common!(Arc);

impl<T, A: Allocator + Clone> Arc<MaybeUninit<T>, A> {
    /// Convert a handle to `MaybeUninit<T>` whose value is now
    /// initialized into a handle to `T`. O(1) — no copy or alloc.
    ///
    /// # Safety
    ///
    /// The `MaybeUninit<T>` must contain a fully-initialized, valid
    /// `T`. The allocation must come from
    /// [`Arena::alloc_uninit_arc`](crate::Arena::alloc_uninit_arc) or
    /// [`Arena::alloc_zeroed_arc`](crate::Arena::alloc_zeroed_arc).
    #[inline]
    #[must_use]
    pub unsafe fn assume_init(self) -> Arc<T, A> {
        let thin = self.ptr;
        mem::forget(self);
        // SAFETY: `thin` carries the strong-count prefix and the live
        // reference the consumed handle held; the value is now a valid
        // `T` per the caller's contract. `MaybeUninit<T>` and `T` share
        // size, alignment, and (empty) metadata, so the strong-prefix
        // chunk layout is identical and no rewrite is needed.
        unsafe { Arc::from_raw(thin) }
    }

    /// Pinned mirror of [`Self::assume_init`]. The pin is preserved
    /// across the cast because the value's address does not change.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::assume_init`].
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin(this: Pin<Self>) -> Pin<Arc<T, A>>
    where
        A: 'static,
    {
        // SAFETY: see `Pin::map_unchecked` + `Self::assume_init`; the
        // value's address is unchanged across this cast, and the
        // caller asserts the contents are a valid `T`.
        unsafe {
            let inner: Self = Pin::into_inner_unchecked(this);
            Arc::into_pin(inner.assume_init())
        }
    }
}

impl<T, A: Allocator + Clone> Arc<[MaybeUninit<T>], A> {
    /// Convert an initialized `Arc<[MaybeUninit<T>]>` into an `Arc<[T]>`.
    ///
    /// O(1) — reinterprets the existing handle in place.
    ///
    /// # Safety
    ///
    /// Every element of the slice must contain a fully-initialized,
    /// valid `T`. The allocation must come from
    /// [`Arena::alloc_uninit_slice_arc`](crate::Arena::alloc_uninit_slice_arc)
    /// or
    /// [`Arena::alloc_zeroed_slice_arc`](crate::Arena::alloc_zeroed_slice_arc).
    #[inline]
    #[must_use]
    pub unsafe fn assume_init(self) -> Arc<[T], A> {
        let thin = self.ptr;
        mem::forget(self);
        // SAFETY: `thin` carries the strong-count prefix and the live
        // reference the consumed handle held; every element is now a
        // valid `T`. `[MaybeUninit<T>]` and `[T]` share an identical
        // chunk prefix layout (the slice length, stored as `usize`), so
        // the metadata already in the prefix matches the new fat
        // pointer.
        unsafe { Arc::from_raw(thin) }
    }

    /// Pinned mirror of [`Self::assume_init`] for slices.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::assume_init`].
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin_slice(this: Pin<Self>) -> Pin<Arc<[T], A>>
    where
        A: 'static,
    {
        // SAFETY: see `Pin::map_unchecked` + `Self::assume_init`; the
        // value's address is unchanged across this cast, and the
        // caller asserts every element is a valid `T`.
        unsafe {
            let inner: Self = Pin::into_inner_unchecked(this);
            Arc::into_pin(inner.assume_init())
        }
    }
}

/// Saturation guard for [`Arc::clone`]: aborts the process when the
/// strong count would overflow, mirroring `std::sync::Arc`.
#[cfg_attr(coverage_nightly, coverage(off))]
#[inline(never)]
#[cold]
fn strong_overflow_abort() -> ! {
    refcount_overflow_abort()
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> Clone for Arc<T, A> {
    #[inline]
    fn clone(&self) -> Self {
        let value_align = mem::align_of_val::<T>(&**self);
        // SAFETY: `self` keeps the value (and its strong-count prefix)
        // alive, so the strong slot is live, aligned, and within the
        // chunk's provenance.
        let strong = unsafe { thin_dst::strong_ref::<T>(self.ptr, value_align) };
        // Relaxed suffices (as `std::sync::Arc`): the new handle need not
        // synchronize until it is dropped.
        let prev = strong.fetch_add(1, Ordering::Relaxed);
        if prev > MAX_STRONG_REFCOUNT {
            strong_overflow_abort();
        }
        Self {
            ptr: self.ptr,
            _phantom: PhantomData,
        }
    }
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> Drop for Arc<T, A> {
    #[inline]
    fn drop(&mut self) {
        let value_align = mem::align_of_val::<T>(&**self);
        // SAFETY: the value (and its strong-count prefix) is still live
        // while this handle exists; the strong slot is aligned and
        // within chunk provenance.
        let strong = unsafe { thin_dst::strong_ref::<T>(self.ptr, value_align) };
        // Release so prior accesses happen-before teardown (as `std::sync::Arc`).
        if strong.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        // Last strong reference: Acquire-fence so other handles' writes are
        // visible before we drop the value and release the chunk.
        fence(Ordering::Acquire);
        // Adopt the chunk's +1 *before* `T::drop` so a panicking destructor
        // still releases the chunk via `ChunkRef`'s `Drop` (the in-chunk slot
        // leaks, per the `alloc_arc*` panic semantics).
        //
        // SAFETY: `ptr` is hosted in a 64K-aligned `SharedChunk` that
        // holds exactly one outstanding +1 for this whole allocation;
        // `from_value_ptr` adopts it. The value is a valid `T` and is
        // dropped exactly once (only on the strong → 0 transition).
        unsafe {
            let _chunk: ChunkRef<A> = ChunkRef::from_value_ptr(self.ptr);
            let fat = self.as_fat_ptr();
            ptr::drop_in_place(fat.as_ptr());
        }
    }
}

// SAFETY: same cross-thread invariants as `std::sync::Arc`; the backing
// chunk refcount is atomic and sharing is gated on `T` and `A`.
unsafe impl<T: ?Sized + Pointee + Sync + Send, A: Allocator + Clone + Send + Sync> Send for Arc<T, A> {}
// SAFETY: same invariants as the `Send` impl.
unsafe impl<T: ?Sized + Pointee + Sync + Send, A: Allocator + Clone + Send + Sync> Sync for Arc<T, A> {}

impl<'a, T, A: Allocator + Clone> From<Vec<'a, T, A>> for Arc<[T], A>
where
    T: Send + Sync,
    A: Send + Sync,
{
    /// Freeze a [`Vec`](crate::vec::Vec) into an immutable
    /// [`Arc<[T], A>`](crate::Arc). Mirrors `std`'s `From<Vec<T>> for Arc<[T]>`.
    #[inline]
    fn from(v: Vec<'a, T, A>) -> Self {
        v.freeze_into_arc()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Arena;

    // Pins the saturation threshold to the `u32` half-range, killing the
    // mutant that swaps `>>` for `<<` in the constant (which would yield
    // `0xFFFF_FFFE`). Behavioral tests cannot reach this — the boundary
    // sits ~2 billion clones away — so assert the value directly.
    #[test]
    fn max_strong_refcount_is_u32_half_range() {
        assert_eq!(MAX_STRONG_REFCOUNT, u32::MAX >> 1);
        assert_eq!(MAX_STRONG_REFCOUNT, 0x7FFF_FFFF);
    }

    // `Arc::clone` checks `prev > MAX_STRONG_REFCOUNT` on the value
    // returned by `fetch_add` (the count *before* the increment), so a
    // clone observing `prev == MAX_STRONG_REFCOUNT` must NOT abort.
    // Driving the strong count to exactly the threshold and cloning kills
    // the `>` -> `==` and `>` -> `>=` mutants on that comparison: both
    // would abort the process here.
    #[test]
    fn clone_at_max_refcount_threshold_does_not_abort() {
        let arena = Arena::new();
        let arc = arena.alloc_arc(0xABCD_u32);
        // SAFETY: `arc` keeps the value and its strong-count prefix live,
        // so the strong slot is aligned and within chunk provenance.
        let strong = unsafe { thin_dst::strong_ref::<u32>(arc.thin_ptr(), mem::align_of::<u32>()) };
        // Force the next clone to observe `prev == MAX_STRONG_REFCOUNT`.
        strong.store(MAX_STRONG_REFCOUNT, Ordering::Relaxed);
        #[expect(
            clippy::redundant_clone,
            reason = "exercising Arc::clone's overflow guard at the threshold is the point of the test"
        )]
        let clone = arc.clone();
        assert_eq!(*clone, 0xABCD);
        // Restore the true live-handle count (`arc` + `clone`) so the two
        // drops tear the value and chunk down correctly instead of
        // leaking the strong count above 1 forever.
        strong.store(2, Ordering::Relaxed);
    }

    // A clone observing `prev > MAX_STRONG_REFCOUNT` MUST abort. Driving
    // the strong count one past the threshold reaches the
    // `strong_overflow_abort()` call site in `Arc::clone` (which panics
    // instead of aborting under `cfg(test)`), covering that guard and
    // killing the `>` -> `==` mutant (which would not fire here).
    #[test]
    #[should_panic(expected = "refcount overflow")]
    fn clone_above_max_refcount_threshold_aborts() {
        let arena = Arena::new();
        let arc = arena.alloc_arc(0xABCD_u32);
        // SAFETY: `arc` keeps the value and its strong-count prefix live,
        // so the strong slot is aligned and within chunk provenance.
        let strong = unsafe { thin_dst::strong_ref::<u32>(arc.thin_ptr(), mem::align_of::<u32>()) };
        strong.store(MAX_STRONG_REFCOUNT + 1, Ordering::Relaxed);
        // The clone panics in its overflow guard before returning, so no
        // clone is produced (but `fetch_add` already bumped the count).
        // Catch it, restore the real live-handle count (just `arc`) so
        // teardown releases the chunk instead of leaking (keeps Miri
        // happy), then resume so `should_panic` observes the panic.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _c = arc.clone();
        }));
        strong.store(1, Ordering::Relaxed);
        std::panic::resume_unwind(result.expect_err("clone past the threshold must panic"));
    }
}
