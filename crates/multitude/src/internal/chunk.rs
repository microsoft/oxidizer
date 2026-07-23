// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reference-counted arena chunk.
//!
//! A chunk backs every allocation style: arena-lifetime references (the
//! [`Alloc`](crate::Alloc) handles, which run their destructor eagerly) and the
//! escape-capable smart pointers (`Box` / `Arc` / `Rc`, which also drop eagerly and
//! take a per-handle chunk refcount). Refcounts and cache-list links are atomic
//! so handles released from any thread can race the arena's own teardown, and
//! the chunk holds a `Weak` provider back-pointer plus a shared allocator handle
//! so a smart pointer that outlives the arena can free the chunk itself.

// Raw-memory methods are `unsafe fn` with item-level safety contracts; inner
// unsafe blocks would not add a boundary here.
#![expect(unsafe_op_in_unsafe_fn, reason = "see module doc: inner unsafe blocks in unsafe fn add noise here")]
#![expect(clippy::unnecessary_safety_comment, reason = "safety rationale documented at function level")]

use alloc::sync::{Arc, Weak};
use core::cell::UnsafeCell;
use core::mem;
use core::ptr::{self, NonNull};
#[cfg(feature = "stats")]
use core::sync::atomic::AtomicU32;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering, fence};

use allocator_api2::alloc::Allocator;

use super::chunk_alloc::chunk_alloc_size;
use super::chunk_provider::ChunkProvider;
use super::constants::{CHUNK_ALIGN, refcount_overflow_abort};
use crate::AllocError;

/// A bump-allocation chunk whose allocations can outlive the arena.
///
/// Reference counts and cache-list links are atomic so that handles released
/// from any thread can safely race with the arena's own teardown path. The
/// header is followed in memory by `capacity` bytes of payload; see
/// [`payload_ptr`](Self::payload_ptr).
#[repr(C)]
pub(crate) struct Chunk<A: Allocator + Clone> {
    allocator: Arc<A>,
    provider: Weak<ChunkProvider<A>>,
    capacity: usize,
    ref_count: AtomicUsize,
    /// Intrusive cache-freelist / retired-list link. Atomic because releases
    /// can push from any thread; null when not linked.
    next: AtomicPtr<u8>,
    /// Wasted tail recorded when a `ChunkMutator` retires this chunk; released
    /// by [`ChunkProvider::release`].
    ///
    /// Release/acquire ordering makes the recorded value visible after
    /// refcount reaches zero.
    #[cfg(feature = "stats")]
    wasted_at_retire: AtomicU32,
    /// Bump-payload tail. `[UnsafeCell<u8>]` permits payload writes through
    /// chunk borrows and keeps fat-pointer provenance over the full
    /// allocation. Bump allocations grow up from the front.
    data: [UnsafeCell<u8>],
}

impl<A: Allocator + Clone> Chunk<A> {
    /// Reads the wasted-tail count stashed at retire time.
    #[cfg(feature = "stats")]
    #[inline]
    pub(in crate::internal) fn wasted_at_retire(&self) -> u32 {
        // Acquire pairs with `set_wasted_at_retire`'s Release store; release
        // may run on a different thread than retire.
        self.wasted_at_retire.load(Ordering::Acquire)
    }

    /// Stashes wasted-tail bytes for release-time stats subtraction.
    ///
    /// `Release` pairs with release-time acquire after refcount reaches zero.
    #[cfg(feature = "stats")]
    #[inline]
    fn set_wasted_at_retire(&self, n: u32) {
        self.wasted_at_retire.store(n, Ordering::Release);
    }

    #[inline]
    // This is the exact fixed header size for either feature layout.
    #[cfg_attr(test, mutants::skip)]
    pub(in crate::internal) const fn header_size() -> usize {
        // Under `stats`, `wasted_at_retire` is the last fixed-size field;
        // otherwise it's `next`. The `[UnsafeCell<u8>]` tail has align 1 and
        // sits flush against whichever it is.
        #[cfg(feature = "stats")]
        {
            mem::offset_of!(Self, wasted_at_retire) + mem::size_of::<AtomicU32>()
        }
        #[cfg(not(feature = "stats"))]
        {
            mem::offset_of!(Self, next) + mem::size_of::<AtomicPtr<u8>>()
        }
    }

    #[inline]
    #[cfg_attr(test, mutants::skip)]
    const fn struct_align() -> usize {
        let base = Self::value_align();
        if base >= CHUNK_ALIGN { base } else { CHUNK_ALIGN }
    }

    /// The chunk type's own alignment, used to round allocation size. This is
    /// separate from [`Self::struct_align`], the base-address alignment.
    #[inline]
    #[cfg_attr(test, mutants::skip)]
    const fn value_align() -> usize {
        let a = mem::align_of::<Arc<A>>();
        let b = mem::align_of::<usize>();
        if a >= b { a } else { b }
    }

    /// Recovers a thin chunk-header pointer from an in-payload pointer.
    ///
    /// Uses [`NonNull::byte_sub`] to preserve provenance.
    #[inline]
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn header_from_value_ptr(value: NonNull<u8>) -> NonNull<u8> {
        let offset_within_chunk = (value.as_ptr() as usize) & (CHUNK_ALIGN - 1);
        // SAFETY: the smart-pointer invariant guarantees `value` lies
        // within the first `CHUNK_ALIGN` bytes of its chunk, so
        // `byte_sub(offset_within_chunk)` lands exactly on the chunk
        // header and stays within the original allocation's
        // provenance.
        unsafe { value.byte_sub(offset_within_chunk) }
    }

    /// Reconstructs the fat DST `*mut Self` from a thin header pointer
    /// by reading the chunk's `capacity` field.
    ///
    /// # Safety
    ///
    /// `header` must carry full chunk-allocation provenance.
    #[inline]
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "chunk header is over-aligned; capacity offset is a multiple of usize alignment"
    )]
    pub(crate) unsafe fn header_to_fat(header: *mut u8) -> *mut Self {
        let cap_field_offset = mem::offset_of!(Self, capacity);
        let cap = ptr::read(header.add(cap_field_offset).cast::<usize>());
        ptr::slice_from_raw_parts_mut(header, cap) as *mut Self
    }

    // A chunk allocation must remain within pointer-offset limits.
    #[cfg_attr(test, mutants::skip)]
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "alloc_chunk_raw returns storage aligned to struct_align(), which is at least Chunk's value alignment"
    )]
    pub(in crate::internal) fn allocate(
        allocator: Arc<A>,
        provider: Weak<ChunkProvider<A>>,
        payload_size: usize,
    ) -> Result<NonNull<Self>, AllocError> {
        let (raw_u8_ptr, _layout) = crate::internal::chunk_alloc::alloc_chunk_raw(
            allocator.as_ref(),
            Self::header_size(),
            payload_size,
            Self::value_align(),
            Self::struct_align(),
        )?;
        let fat: *mut Self = ptr::slice_from_raw_parts_mut(raw_u8_ptr, payload_size) as *mut Self;
        // SAFETY: `fat` points at freshly-allocated storage aligned for the
        // header. Each header field is initialized through a projected raw
        // pointer; the slice tail stays uninitialized for the bump allocator.
        unsafe {
            ptr::write(&raw mut (*fat).allocator, allocator);
            ptr::write(&raw mut (*fat).provider, provider);
            ptr::write(&raw mut (*fat).capacity, payload_size);
            ptr::write(&raw mut (*fat).ref_count, AtomicUsize::new(1));
            ptr::write(&raw mut (*fat).next, AtomicPtr::new(ptr::null_mut()));
            #[cfg(feature = "stats")]
            ptr::write(&raw mut (*fat).wasted_at_retire, AtomicU32::new(0));
            Ok(NonNull::new_unchecked(fat))
        }
    }

    /// Rounded backing-allocation size (`Layout::size()`) of a chunk whose
    /// payload holds `payload` bytes. The single source of truth for chunk
    /// byte accounting: every reserve / release / cache path routes through
    /// here so the rounded footprint stays balanced.
    #[inline]
    pub(in crate::internal) fn footprint(payload: usize) -> Result<usize, AllocError> {
        chunk_alloc_size(Self::header_size(), payload, Self::value_align())
    }

    #[inline]
    pub(in crate::internal) unsafe fn payload_ptr(chunk: NonNull<Self>) -> NonNull<u8> {
        // SAFETY: project through the DST tail so the pointer keeps payload
        // provenance; `data_slice_ptr` is non-null and points at the first
        // payload byte.
        let data_slice_ptr: *mut [UnsafeCell<u8>] = &raw mut (*chunk.as_ptr()).data;
        NonNull::new_unchecked(data_slice_ptr.cast::<u8>())
    }

    /// # Safety
    ///
    /// Caller must hold the unique remaining reference (refcount observed
    /// zero with an acquire fence covering all prior releases).
    pub(in crate::internal) unsafe fn destroy(chunk: NonNull<Self>) {
        let header = Self::header_size();
        let header_ref = &*chunk.as_ptr();
        let capacity = header_ref.capacity;
        let allocator: Arc<A> = ptr::read(&raw const (*chunk.as_ptr()).allocator);
        ptr::drop_in_place(&raw mut (*chunk.as_ptr()).provider);
        let layout = crate::internal::chunk_alloc::chunk_layout(header, capacity, Self::value_align(), Self::struct_align())
            .expect("matches allocate(); header+capacity stayed within isize::MAX");
        let raw_ptr = chunk.as_ptr().cast::<u8>();
        allocator.deallocate(NonNull::new_unchecked(raw_ptr), layout);
        drop(allocator);
    }

    /// Pointer to the intrusive cache-freelist link storing a thin header
    /// pointer.
    ///
    /// # Safety
    ///
    /// Chunk must be allocated (header live); ordering of accesses
    /// through the returned pointer is the caller's responsibility.
    #[inline]
    pub(in crate::internal) unsafe fn cache_link(chunk: NonNull<Self>) -> *const AtomicPtr<u8> {
        &raw const (*chunk.as_ptr()).next
    }

    /// Reads the intrusive `next` link (shared by the retired list and the
    /// cache freelist; the phases are mutually exclusive). The retired list
    /// is touched only by the owning thread, so `Relaxed` suffices there.
    ///
    /// # Safety
    ///
    /// Chunk must be live; the caller must hold exclusive access for the
    /// duration of the call (owning-thread invariant for the retired list).
    #[inline]
    pub(crate) unsafe fn next(chunk: NonNull<Self>) -> *mut u8 {
        chunk.as_ref().next.load(Ordering::Relaxed)
    }

    /// Writes the intrusive `next` link. See [`Self::next`].
    ///
    /// # Safety
    ///
    /// Same as [`Self::next`].
    #[inline]
    pub(crate) unsafe fn set_next(chunk: NonNull<Self>, next: *mut u8) {
        chunk.as_ref().next.store(next, Ordering::Relaxed);
    }

    /// Re-initializes a chunk popped from the cache: refcount → 1. The caller
    /// becomes the +1 holder.
    ///
    /// # Safety
    ///
    /// `chunk` must be a freshly popped, refcount-zero, uniquely-owned
    /// chunk; the cache link is invalidated by this call.
    #[inline]
    pub(in crate::internal) unsafe fn reinit_for_acquire(chunk: NonNull<Self>) {
        // SAFETY: caller owns the unique reference; the store is safe to
        // issue unconditionally.
        let r = &*chunk.as_ptr();
        r.ref_count.store(1, Ordering::Relaxed);
    }

    /// Overwrites the refcount. Test-only seam so unit tests can drive
    /// refcount-dependent paths (e.g. the overflow guard) without poking
    /// the field directly.
    #[cfg(test)]
    fn set_ref_count_for_test(&self, count: usize) {
        self.ref_count.store(count, Ordering::Relaxed);
    }

    /// Releases one strong ref and routes zero-ref chunks through
    /// [`teardown_and_release`](super::chunk_ops::ChunkOps::teardown_and_release).
    ///
    /// # Safety
    ///
    /// Caller must hold exactly one strong reference to `chunk` that
    /// is being released by this call; after the call returns the
    /// caller must not dereference `chunk` again.
    #[inline]
    pub(in crate::internal) unsafe fn release_one_ref(chunk: NonNull<Self>) {
        // SAFETY: caller holds a +1 we are releasing. `dec_ref`
        // observes the previous refcount; if it returns true the
        // caller holds the unique remaining reference and we route
        // through `teardown_and_release`.
        let chunk_ref = chunk.as_ref();
        if chunk_ref.dec_ref() {
            Self::teardown_and_release(chunk);
        }
    }

    /// Atomically reserves `n` additional strong references. Aborts on
    /// overflow.
    ///
    /// Used by arena surplus pre-credit; unused refs are returned through
    /// [`Self::refund_refs`] when the chunk is retired.
    #[inline]
    pub(crate) fn pre_credit_refs(&self, n: usize) {
        #[cfg_attr(coverage_nightly, coverage(off))]
        #[inline(never)]
        #[cold]
        fn overflow() -> ! {
            refcount_overflow_abort()
        }
        if n == 0 {
            return;
        }
        let prev = self.ref_count.fetch_add(n, Ordering::Relaxed);
        if prev.checked_add(n).is_none() {
            overflow();
        }
    }

    /// Atomically returns `n` pre-credited but unused refs with `Release`
    /// ordering, matching [`Chunk::dec_ref`](super::Chunk::dec_ref).
    ///
    /// # Safety
    ///
    /// Caller must own exactly `n` previously-credited (unhanded-out)
    /// strong references on this chunk. After this call those `n`
    /// references no longer exist.
    #[inline]
    pub(crate) unsafe fn refund_refs(&self, n: usize) {
        if n == 0 {
            return;
        }
        let prev = self.ref_count.fetch_sub(n, Ordering::Release);
        debug_assert!(prev >= n, "refund_refs underflow: prev={prev} n={n}");
    }

    /// Returns the chunk's payload capacity in bytes (i.e. `data.len()`).
    #[inline]
    // Returning an incorrect capacity can make allocation retries infinite.
    #[cfg_attr(test, mutants::skip)]
    pub(in crate::internal) fn capacity(&self) -> usize {
        self.capacity
    }

    /// Increments the chunk's strong reference count by one (smart-pointer
    /// `clone` of a family handle). Aborts the process on overflow.
    #[inline]
    pub(crate) fn inc_ref(&self) {
        #[cfg_attr(coverage_nightly, coverage(off))]
        #[inline(never)]
        #[cold]
        fn overflow() -> ! {
            refcount_overflow_abort()
        }
        let prev = self.ref_count.fetch_add(1, Ordering::Relaxed);
        if prev == usize::MAX {
            overflow();
        }
    }

    /// Decrements the chunk's strong reference count by one. Returns `true`
    /// if the count reached zero, signaling the caller must tear the chunk
    /// down.
    #[inline]
    pub(crate) fn dec_ref(&self) -> bool {
        let prev = self.ref_count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            fence(Ordering::Acquire);
            true
        } else {
            false
        }
    }

    /// Routes `chunk` (refcount zero) back to the provider cache, or
    /// deallocates it if the provider is gone.
    ///
    /// # Safety
    ///
    /// Caller must hold the unique remaining reference to `chunk`.
    #[cold]
    #[inline(never)]
    pub(crate) unsafe fn teardown_and_release(chunk: NonNull<Self>) {
        // SAFETY: caller owns the unique remaining reference.
        let chunk_ref = &*chunk.as_ptr();
        // Chunks can outlive their provider, so release through `Weak`.
        if let Some(provider) = chunk_ref.provider.upgrade() {
            provider.release(chunk);
        } else {
            Self::destroy(chunk);
        }
    }

    /// Records wasted tail on retire and updates the provider counter; the
    /// provider subtracts it when the chunk is later cached or destroyed.
    ///
    /// # Safety
    ///
    /// `chunk` must reference a live chunk the caller holds a reference to.
    #[cfg(feature = "stats")]
    pub(in crate::internal) unsafe fn record_retire(chunk: NonNull<Self>, wasted: u32) {
        let chunk_ref = &*chunk.as_ptr();
        chunk_ref.set_wasted_at_retire(wasted);
        // If the provider is gone, no stats counter remains to update.
        if let Some(provider) = chunk_ref.provider.upgrade() {
            provider.record_wasted_tail(u64::from(wasted));
        }
    }
}

/// Largest payload byte count a chunk can offer to a bump allocator after
/// accounting for the header.
#[inline]
#[must_use]
pub(crate) const fn max_bump_extent<A: Allocator + Clone>() -> usize {
    super::constants::MAX_CHUNK_BYTES - Chunk::<A>::header_size()
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    use super::*;

    /// `header_size` is `offset_of!(<last field>) + size_of::<<last field>>()`.
    /// Every fixed header field is pointer-sized and pointer-aligned, so the
    /// header packs without padding. It contains the allocator `Arc`, provider
    /// `Weak`, capacity, `ref_count`, and `next`. Under the `stats` feature an
    /// `AtomicU32` (`wasted_at_retire`) is appended. Computing the expectation
    /// from `size_of` keeps this assertion valid on non-64-bit targets rather
    /// than baking in 8-byte field widths.
    #[test]
    fn header_size_for_global_matches_layout() {
        let expected = mem::size_of::<Arc<Global>>()
            + mem::size_of::<Weak<ChunkProvider<Global>>>()
            + mem::size_of::<usize>()
            + mem::size_of::<AtomicUsize>()
            + mem::size_of::<AtomicPtr<u8>>();

        #[cfg(not(feature = "stats"))]
        assert_eq!(Chunk::<Global>::header_size(), expected);
        #[cfg(feature = "stats")]
        assert_eq!(Chunk::<Global>::header_size(), expected + mem::size_of::<AtomicU32>());
    }

    /// `struct_align` is the maximum of the allocator handle alignment,
    /// `align_of::<usize>()`, and `CHUNK_ALIGN`.
    #[test]
    fn struct_align_is_max_of_components() {
        let got = Chunk::<Global>::struct_align();
        // Chunks must be CHUNK_ALIGN-aligned so smart-pointer
        // chunk-header recovery via `byte_sub` lands on the header.
        assert!(got >= super::super::constants::CHUNK_ALIGN);
        assert!(got >= mem::align_of::<Arc<Global>>());
        assert!(got >= mem::align_of::<usize>());
        // Equality at the typical case: pointer and usize alignment are
        // both 8 on 64-bit, so CHUNK_ALIGN dominates.
        assert_eq!(got, super::super::constants::CHUNK_ALIGN);
    }

    /// `chunk_layout` rounds size to `value_align` and base alignment to
    /// `struct_align`, without inflating every class to `CHUNK_ALIGN`.
    #[test]
    fn chunk_layout_does_not_inflate_size_to_base_align() {
        use super::super::chunk_alloc::chunk_layout;
        use super::super::constants::{CHUNK_ALIGN, NUM_CHUNK_CLASSES, SizeClass};

        let header = Chunk::<Global>::header_size();
        let value_align = Chunk::<Global>::value_align();
        let base_align = Chunk::<Global>::struct_align();
        assert_eq!(base_align, CHUNK_ALIGN, "chunks need CHUNK_ALIGN base alignment");
        assert!(value_align <= base_align);

        for i in 0..NUM_CHUNK_CLASSES {
            let class = SizeClass::new(i);
            let total = class.bytes();
            let payload = total - header;
            let layout = chunk_layout(header, payload, value_align, base_align).expect("layout fits");
            // Size classes are powers-of-two multiples of 512, hence
            // already `value_align`-aligned, so the rounded size is exactly
            // the class bytes — crucially NOT inflated to `CHUNK_ALIGN`.
            assert_eq!(
                layout.size(),
                total,
                "class {i} size must equal class bytes, not be padded to base align"
            );
            assert_eq!(
                layout.align(),
                CHUNK_ALIGN,
                "base must stay CHUNK_ALIGN-aligned for header recovery"
            );
            if total < CHUNK_ALIGN {
                assert!(
                    layout.size() < CHUNK_ALIGN,
                    "class {i} ({total} B) must not be inflated to {CHUNK_ALIGN} B"
                );
            }
        }
    }

    /// Pins `value_align()` (a hand-computed layout constant) against the
    /// real alignment of a constructed chunk, so a future field with a
    /// larger alignment can't silently make the size-rounding too small
    /// (which would be UB). `align_of_val` is valid on the DST reference.
    #[test]
    fn value_align_matches_real_alignment() {
        // SAFETY: single-threaded test; refcount forced to 0 before destroy.
        unsafe {
            let chunk = Chunk::<Global>::allocate(Arc::new(Global), Weak::new(), 64).expect("allocate chunk");
            let real = mem::align_of_val(chunk.as_ref());
            assert_eq!(
                Chunk::<Global>::value_align(),
                real,
                "value_align must equal align_of_val of the real chunk DST"
            );
            assert!(Chunk::<Global>::struct_align() >= real);
            chunk.as_ref().set_ref_count_for_test(0);
            Chunk::destroy(chunk);
        }
    }

    /// `max_bump_extent` is the maximum chunk size minus its header.
    #[test]
    fn max_bump_extent_is_max_minus_header() {
        let header = Chunk::<Global>::header_size();
        let extent = max_bump_extent::<Global>();
        assert_eq!(extent, super::super::constants::MAX_CHUNK_BYTES - header);
        assert!(extent < super::super::constants::MAX_CHUNK_BYTES);
    }

    // The test configuration turns the overflow abort into a panic.
    #[test]
    #[should_panic(expected = "refcount overflow")]
    fn inc_ref_overflow_triggers_abort_guard() {
        // SAFETY: the test owns the chunk and restores its count before
        // destruction.
        unsafe {
            let chunk = Chunk::<Global>::allocate(Arc::new(Global), Weak::new(), 64).expect("allocate chunk");
            chunk.as_ref().set_ref_count_for_test(usize::MAX);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                chunk.as_ref().inc_ref();
            }));
            chunk.as_ref().set_ref_count_for_test(0);
            Chunk::destroy(chunk);
            std::panic::resume_unwind(result.expect_err("inc_ref must panic"));
        }
    }

    // Zero credit/refund operations preserve the count; nonzero operations
    // round-trip to the initial count.
    #[test]
    fn pre_credit_and_refund_zero_are_noops() {
        // SAFETY: single-threaded test owning the only references; we
        // restore the refcount to 0 before destroying the chunk.
        unsafe {
            let chunk = Chunk::<Global>::allocate(Arc::new(Global), Weak::new(), 64).expect("allocate chunk");
            let header = chunk.as_ref();
            let before = header.ref_count.load(Ordering::Relaxed);

            header.pre_credit_refs(0);
            header.refund_refs(0);
            assert_eq!(
                header.ref_count.load(Ordering::Relaxed),
                before,
                "zero credit/refund must not move the count"
            );

            header.pre_credit_refs(5);
            assert_eq!(header.ref_count.load(Ordering::Relaxed), before + 5);
            header.refund_refs(5);
            assert_eq!(
                header.ref_count.load(Ordering::Relaxed),
                before,
                "credit/refund round-trip must restore the count"
            );

            header.set_ref_count_for_test(0);
            Chunk::destroy(chunk);
        }
    }

    // The test configuration turns pre-credit overflow into a panic.
    #[test]
    #[should_panic(expected = "refcount overflow")]
    fn pre_credit_refs_overflow_triggers_abort_guard() {
        // SAFETY: the test owns the chunk and restores its count before
        // destruction.
        unsafe {
            let chunk = Chunk::<Global>::allocate(Arc::new(Global), Weak::new(), 64).expect("allocate chunk");
            chunk.as_ref().set_ref_count_for_test(usize::MAX);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                chunk.as_ref().pre_credit_refs(1);
            }));
            chunk.as_ref().set_ref_count_for_test(0);
            Chunk::destroy(chunk);
            std::panic::resume_unwind(result.expect_err("pre_credit_refs must panic"));
        }
    }
}
