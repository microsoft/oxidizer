// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::marker::PhantomData;
use core::ptr::NonNull;

use allocator_api2::alloc::{Allocator, Global};
use widestring::Utf16Str;

use crate::internal::chunk_ref::ChunkRef;
use crate::strings::utf16_str_common::impl_utf16_str_common;

/// An immutable, single-pointer reference-counted UTF-16 string stored
/// in an [`Arena`](crate::Arena), safe to share across threads.
///
/// **8 bytes** on 64-bit (one pointer). The pointer addresses the first
/// `u16` of the UTF-16 payload inside a 64K-aligned shared chunk; the
/// element count is stored as a `usize` immediately before the payload
/// (read with [`core::ptr::read_unaligned`], no usize-alignment padding
/// imposed on the chunk).
///
/// Cloning is **O(1)** — one atomic refcount bump on the hosting
/// chunk. Lengths and indexing are in `u16` code units.
pub struct ArcUtf16Str<A: Allocator + Clone = Global> {
    /// Thin pointer to the first `u16` of the payload. The element
    /// count lives in the `usize` immediately preceding the payload
    /// (read with `read_unaligned`).
    ptr: NonNull<u16>,
    _phantom: PhantomData<(*const Utf16Str, A)>,
}

// SAFETY: thin pointer into an atomically-refcounted shared chunk;
// `Utf16Str` is `Send + Sync`.
unsafe impl<A: Allocator + Clone + Send + Sync> Send for ArcUtf16Str<A> {}
// SAFETY: exposes only `&Utf16Str` (shared, immutable) — no interior
// mutability, no `&mut`-yielding API.
unsafe impl<A: Allocator + Clone + Send + Sync> Sync for ArcUtf16Str<A> {}

impl<A: Allocator + Clone> ArcUtf16Str<A> {
    /// Builds an `ArcUtf16Str` from a raw length-prefixed payload pointer.
    ///
    /// # Safety
    ///
    /// - `ptr` must point at the first `u16` of a length-prefixed UTF-16
    ///   payload bump-allocated from a `SharedChunk<A>` (via
    ///   [`Arena::impl_alloc_prefixed_shared`](crate::Arena)).
    /// - The caller must have just acquired a +1 refcount on that chunk
    ///   in the new `ArcUtf16Str`'s name; the returned value owns that
    ///   +1 and releases it in [`Drop`].
    /// - `ptr` must lie within the first `CHUNK_ALIGN` bytes of the
    ///   chunk so the header-from-mask helper recovers the chunk address.
    #[inline]
    pub(crate) unsafe fn from_raw(ptr: NonNull<u16>) -> Self {
        Self {
            ptr,
            _phantom: PhantomData,
        }
    }
}

impl_utf16_str_common!(ArcUtf16Str);

impl<A: Allocator + Clone> Clone for ArcUtf16Str<A> {
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: `self` keeps the payload (and its strong-count prefix)
        // alive; the strong slot is aligned and within chunk provenance.
        // The conceptual value type is `[u16]` (element align 2,
        // `usize` metadata), matching the allocator's strong-prefix
        // layout.
        let strong = unsafe { crate::internal::thin_dst::strong_ref::<[u16]>(self.ptr.cast::<u8>(), core::mem::align_of::<u16>()) };
        let prev = strong.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if prev > (u32::MAX >> 1) {
            crate::internal::constants::refcount_overflow_abort();
        }
        Self {
            ptr: self.ptr,
            _phantom: PhantomData,
        }
    }
}

impl<A: Allocator + Clone> Drop for ArcUtf16Str<A> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: the payload (and its strong-count prefix) is live while
        // this handle exists; the strong slot is aligned and in chunk
        // provenance (conceptual value type `[u16]`).
        let strong = unsafe { crate::internal::thin_dst::strong_ref::<[u16]>(self.ptr.cast::<u8>(), core::mem::align_of::<u16>()) };
        if strong.fetch_sub(1, core::sync::atomic::Ordering::Release) != 1 {
            return;
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
        // Last strong reference: release the chunk +1. The `[u16]`
        // payload has no element destructor to run.
        //
        // SAFETY: `ptr` is hosted in a 64K-aligned `SharedChunk` holding
        // exactly one outstanding +1 for this `Arc` family;
        // `from_value_ptr` adopts and releases it.
        unsafe {
            let _ref: ChunkRef<A> = ChunkRef::from_value_ptr(self.ptr);
        }
    }
}

impl<A: Allocator + Clone> From<ArcUtf16Str<A>> for crate::Arc<[u16], A> {
    /// Convert an [`ArcUtf16Str<A>`] into an [`Arc<[u16], A>`](crate::Arc).
    ///
    /// `ArcUtf16Str` is a thin 8-byte pointer to a length-prefixed
    /// UTF-16 payload in a shared chunk; this reads the length from
    /// the chunk prefix, reconstructs a `NonNull<[u16]>` over the same
    /// payload, and transfers the chunk +1 into the fat
    /// `Arc<[u16], A>`. O(1), no copy.
    #[inline]
    fn from(s: ArcUtf16Str<A>) -> Self {
        use core::mem::ManuallyDrop;
        let me = ManuallyDrop::new(s);
        // SAFETY: `me.ptr` was produced by
        // `Arena::impl_alloc_prefixed_shared::<u16>`; it carries
        // chunk-wide provenance, the prefix word stores the u16
        // element count, and `ManuallyDrop` transfers the chunk +1
        // into the new `Arc<[u16]>` (whose `as_fat_ptr` recovers the
        // length from the same prefix on demand).
        unsafe { Self::from_raw(me.ptr.cast::<u8>()) }
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::Arena;
    use crate::internal::thin_dst::strong_ref;

    // The per-string strong count lives in the chunk prefix, accessed as
    // an `[u16]` strong reference (element align 2) — exactly as the
    // `Clone`/`Drop` impls do.
    fn strong_of<A: Allocator + Clone>(s: &ArcUtf16Str<A>) -> &AtomicU32 {
        // SAFETY: `s` keeps the payload and its strong-count prefix live,
        // so the strong slot is aligned and within chunk provenance.
        unsafe { strong_ref::<[u16]>(s.ptr.cast::<u8>(), core::mem::align_of::<u16>()) }
    }

    // `Drop` must decrement the per-string strong count (and release the
    // chunk on the last handle). Kills the `drop -> ()` mutant: cloning
    // bumps the count, so dropping the clone must bring it back down.
    #[test]
    fn drop_decrements_strong_count() {
        let arena = Arena::new();
        let s = arena.alloc_utf16_str_arc_from_str("hi");
        let strong = strong_of(&s);
        let base = strong.load(Ordering::Relaxed);
        let s2 = s.clone();
        assert_eq!(strong.load(Ordering::Relaxed), base + 1, "clone must bump the strong count");
        drop(s2);
        assert_eq!(strong.load(Ordering::Relaxed), base, "drop must decrement the strong count");
        // `s` (still live) holds the chunk; it drops normally at scope end.
    }

    // `Clone` checks `prev > (u32::MAX >> 1)` on the value returned by
    // `fetch_add` (the count *before* the increment), so a clone
    // observing `prev == u32::MAX >> 1` must NOT abort. Kills the
    // `>` -> `==` and `>` -> `>=` mutants on that comparison.
    #[test]
    fn clone_at_max_refcount_threshold_does_not_abort() {
        let arena = Arena::new();
        let s = arena.alloc_utf16_str_arc_from_str("hi");
        let strong = strong_of(&s);
        strong.store(u32::MAX >> 1, Ordering::Relaxed);
        let clone = s.clone();
        // Reached here without panic. Restore the true live-handle count
        // (`s` + `clone`) so teardown releases the chunk instead of
        // leaking the strong count above 1 forever.
        strong.store(2, Ordering::Relaxed);
        drop(clone);
    }

    // A clone observing `prev > u32::MAX >> 1` MUST abort. Driving the
    // strong count one past the threshold kills the `>` -> `==` mutant
    // (it would not fire) and the `>>` -> `<<` mutant (which raises the
    // threshold to `0xFFFF_FFFE`, so the guard would not fire here).
    #[test]
    #[should_panic(expected = "refcount overflow")]
    fn clone_above_max_refcount_threshold_aborts() {
        let arena = Arena::new();
        let s = arena.alloc_utf16_str_arc_from_str("hi");
        let strong = strong_of(&s);
        strong.store((u32::MAX >> 1) + 1, Ordering::Relaxed);
        // The clone panics in its overflow guard before returning, so no
        // clone is produced. Catch it, restore the real live-handle count
        // (just `s`) so teardown releases the chunk instead of leaking
        // (keeps Miri happy), then resume so `should_panic` sees it.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _c = s.clone();
        }));
        strong.store(1, Ordering::Relaxed);
        std::panic::resume_unwind(result.expect_err("clone past the threshold must panic"));
    }
}
