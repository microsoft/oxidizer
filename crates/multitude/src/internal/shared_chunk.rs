// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multi-threaded reference-counted arena chunk.

// Raw-memory methods are `unsafe fn` with item-level safety contracts; inner
// unsafe blocks would not add a boundary here.
#![allow(unsafe_op_in_unsafe_fn, reason = "see module doc: inner unsafe blocks in unsafe fn add noise here")]
#![allow(clippy::unnecessary_safety_comment, reason = "safety rationale documented at function level")]

use alloc::sync::Weak;
use core::cell::UnsafeCell;
use core::mem;
use core::ptr::{self, NonNull};
#[cfg(feature = "stats")]
use core::sync::atomic::AtomicU32;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering, fence};

use allocator_api2::alloc::{AllocError, Allocator};

use super::chunk::Chunk;
use super::chunk_provider::ChunkProvider;
use super::constants::{CHUNK_ALIGN, refcount_overflow_abort};

/// A bump-allocation chunk whose allocations can outlive the arena.
///
/// Reference counts and cache-list links are atomic so that handles released
/// from any thread can safely race with the arena's own teardown path. The
/// header is followed in memory by `capacity` bytes of payload; see
/// [`payload_ptr`](Self::payload_ptr).
#[repr(C)]
pub(crate) struct SharedChunk<A: Allocator + Clone> {
    allocator: A,
    provider: Weak<ChunkProvider<A>>,
    capacity: usize,
    ref_count: AtomicUsize,
    /// Intrusive shared-cache freelist link. Atomic because releases can push
    /// from any thread; null when not cached.
    ///
    /// Shared chunks use this only for the cache freelist, not a retired list.
    next: AtomicPtr<u8>,
    /// Wasted tail recorded when a `ChunkMutator` retires this chunk; released
    /// by [`ChunkProvider::release_shared`].
    ///
    /// Release/acquire ordering makes the recorded value visible after
    /// refcount reaches zero.
    #[cfg(feature = "stats")]
    wasted_at_retire: AtomicU32,
    /// Bump-payload tail. See `LocalChunk` for the
    /// [`UnsafeCell<u8>]` provenance rationale. Shared chunks register no
    /// drop entries (values are owned by `Box`/`Arc` and dropped eagerly),
    /// so the whole payload is available to the bump cursor.
    data: [UnsafeCell<u8>],
}

impl<A: Allocator + Clone> SharedChunk<A> {
    /// Borrow the weak provider back-pointer; callers must `upgrade()`.
    #[inline]
    pub(crate) fn provider(&self) -> &Weak<ChunkProvider<A>> {
        &self.provider
    }

    /// Reads the wasted-tail count stashed at retire time.
    #[cfg(feature = "stats")]
    #[inline]
    pub(crate) fn wasted_at_retire(&self) -> u32 {
        // Acquire pairs with `set_wasted_at_retire`'s Release store; release
        // may run on a different thread than retire.
        self.wasted_at_retire.load(Ordering::Acquire)
    }

    /// Stashes wasted-tail bytes for release-time stats subtraction.
    ///
    /// `Release` pairs with release-time acquire after refcount reaches zero.
    #[cfg(feature = "stats")]
    #[inline]
    pub(crate) fn set_wasted_at_retire(&self, n: u32) {
        self.wasted_at_retire.store(n, Ordering::Release);
    }

    #[inline]
    // Mutation testing is suppressed: see `LocalChunk::header_size`. This
    // is a pure const layout computation pinned exactly by the
    // `header_size_for_global_matches_layout` test under both feature configs, and
    // the `#[cfg(not(feature = "stats"))]` branch is dead code under the
    // all-features mutants run.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) const fn header_size() -> usize {
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
    #[cfg_attr(test, mutants::skip)] // both branches saturate at CHUNK_ALIGN
    pub(crate) const fn struct_align() -> usize {
        let base = Self::value_align();
        if base >= CHUNK_ALIGN { base } else { CHUNK_ALIGN }
    }

    /// The chunk type's own alignment, used to round allocation size. This is
    /// separate from [`Self::struct_align`], the base-address alignment.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // pure layout constant pinned by a dedicated test
    pub(crate) const fn value_align() -> usize {
        let a = mem::align_of::<A>();
        let b = mem::align_of::<usize>();
        if a >= b { a } else { b }
    }

    /// Recovers a thin chunk-header pointer from an in-payload pointer.
    ///
    /// Uses [`NonNull::byte_sub`] to preserve provenance.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // mask mutations break refcount → OOM in mutant harness
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
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "chunk header is over-aligned; capacity offset is a multiple of usize alignment"
    )]
    pub(crate) unsafe fn header_to_fat(header: *mut u8) -> *mut Self {
        let cap_field_offset = mem::offset_of!(Self, capacity);
        let cap = ptr::read(header.add(cap_field_offset).cast::<usize>());
        ptr::slice_from_raw_parts_mut(header, cap) as *mut Self
    }

    // Mutation testing is suppressed: `> → >=` only differs at the
    // unreachable exact-`isize::MAX` boundary.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn allocate(allocator: A, provider: Weak<ChunkProvider<A>>, payload_size: usize) -> Result<NonNull<Self>, AllocError> {
        let (raw_u8_ptr, _layout) = crate::internal::chunk_alloc::alloc_chunk_raw(
            &allocator,
            Self::header_size(),
            payload_size,
            Self::value_align(),
            Self::struct_align(),
        )?;
        let fat: *mut Self = ptr::slice_from_raw_parts_mut(raw_u8_ptr, payload_size) as *mut Self;
        // SAFETY: see `LocalChunk::allocate`.
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

    #[inline]
    pub(crate) unsafe fn payload_ptr(chunk: NonNull<Self>) -> NonNull<u8> {
        // SAFETY: see `LocalChunk::payload_ptr`.
        let data_slice_ptr: *mut [UnsafeCell<u8>] = &raw mut (*chunk.as_ptr()).data;
        NonNull::new_unchecked(data_slice_ptr.cast::<u8>())
    }

    /// # Safety
    ///
    /// Caller must hold the unique remaining reference (refcount observed
    /// zero with an acquire fence covering all prior releases).
    pub(crate) unsafe fn destroy(chunk: NonNull<Self>) {
        let header = Self::header_size();
        let header_ref = &*chunk.as_ptr();
        let capacity = header_ref.capacity;
        // Shared chunks register no drop entries; per-`Arc` values drop on
        // their last strong reference.
        let allocator: A = ptr::read(&raw const (*chunk.as_ptr()).allocator);
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
    pub(crate) unsafe fn cache_link(chunk: NonNull<Self>) -> *const AtomicPtr<u8> {
        &raw const (*chunk.as_ptr()).next
    }

    /// Re-initializes a chunk popped from the cache: refcount → 1. The caller
    /// becomes the +1 holder.
    ///
    /// # Safety
    ///
    /// `chunk` must be a freshly popped, refcount-zero, uniquely-owned
    /// chunk; the cache link is invalidated by this call.
    #[inline]
    pub(crate) unsafe fn reinit_for_acquire(chunk: NonNull<Self>) {
        // SAFETY: caller owns the unique reference; the refcount store is
        // safe to issue unconditionally.
        let r = &*chunk.as_ptr();
        r.ref_count.store(1, Ordering::Relaxed);
    }

    /// Overwrites the refcount. Test-only seam so unit tests can drive
    /// refcount-dependent paths (e.g. the overflow guard) without poking
    /// the field directly.
    #[cfg(test)]
    pub(crate) fn set_ref_count_for_test(&self, count: usize) {
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
    pub(crate) unsafe fn release_one_ref(chunk: NonNull<Self>) {
        // SAFETY: caller holds a +1 we are releasing. `dec_ref`
        // observes the previous refcount; if it returns true the
        // caller holds the unique remaining reference and we route
        // through `teardown_and_release`.
        use super::chunk_ops::ChunkOps;
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
}

impl<A: Allocator + Clone> Chunk for SharedChunk<A> {
    #[inline]
    // Mutation testing is suppressed: see `LocalChunk::capacity` — a
    // 0/1 capacity drives the allocator's refill loop into an unbounded
    // spin, hanging the suite instead of failing it.
    #[cfg_attr(test, mutants::skip)]
    fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    fn inc_ref(&self) {
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

    #[inline]
    fn dec_ref(&self) -> bool {
        let prev = self.ref_count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            fence(Ordering::Acquire);
            true
        } else {
            false
        }
    }
}

/// Largest payload byte count a shared chunk can offer to a bump allocator
/// after accounting for the header.
#[inline]
#[must_use]
pub(crate) const fn max_bump_extent<A: Allocator + Clone>() -> usize {
    super::constants::MAX_CHUNK_BYTES - SharedChunk::<A>::header_size()
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    use super::*;

    /// `header_size` is `offset_of!(<last field>) + size_of::<<last field>>()`.
    /// For `SharedChunk<Global>`, the header layout is fixed:
    /// 0 (allocator ZST) + 8 (provider `Weak`) + 8 (capacity) +
    /// 8 (`ref_count`) + 8 (`next`) = 32 bytes. Under the `stats` feature an
    /// additional `wasted_at_retire: AtomicU32` is appended (offset 32) for
    /// 36 bytes total. Shared chunks carry no `drop_entry_count` — they never
    /// register drop entries.
    #[test]
    fn header_size_for_global_matches_layout() {
        #[cfg(not(feature = "stats"))]
        assert_eq!(SharedChunk::<Global>::header_size(), 32);
        #[cfg(feature = "stats")]
        assert_eq!(SharedChunk::<Global>::header_size(), 36);
    }

    /// `struct_align` returns the max of `align_of::<A>()`,
    /// `align_of::<usize>()`, and `CHUNK_ALIGN`. Pin the exact value so
    /// the `>= → <` mutation flips it.
    #[test]
    fn struct_align_is_max_of_components() {
        let got = SharedChunk::<Global>::struct_align();
        // Shared chunks must be CHUNK_ALIGN-aligned so smart-pointer
        // chunk-header recovery via `byte_sub` lands on the header.
        assert!(got >= super::super::constants::CHUNK_ALIGN);
        assert!(got >= mem::align_of::<Global>());
        assert!(got >= mem::align_of::<usize>());
        // Equality at the typical case: Global is ZST so its align is
        // 1, usize align is 8 (on 64-bit), CHUNK_ALIGN dominates.
        assert_eq!(got, super::super::constants::CHUNK_ALIGN);
    }

    /// `chunk_layout` rounds size to `value_align` and base alignment to
    /// `struct_align`, without inflating every class to `CHUNK_ALIGN`.
    #[test]
    fn chunk_layout_does_not_inflate_size_to_base_align() {
        use super::super::chunk_alloc::chunk_layout;
        use super::super::constants::{CHUNK_ALIGN, NUM_CHUNK_CLASSES, SizeClass};

        let header = SharedChunk::<Global>::header_size();
        let value_align = SharedChunk::<Global>::value_align();
        let base_align = SharedChunk::<Global>::struct_align();
        assert_eq!(base_align, CHUNK_ALIGN, "shared chunks need CHUNK_ALIGN base alignment");
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
            let chunk = SharedChunk::<Global>::allocate(Global, Weak::new(), 64).expect("allocate chunk");
            let real = mem::align_of_val(chunk.as_ref());
            assert_eq!(
                SharedChunk::<Global>::value_align(),
                real,
                "value_align must equal align_of_val of the real chunk DST"
            );
            assert!(SharedChunk::<Global>::struct_align() >= real);
            chunk.as_ref().set_ref_count_for_test(0);
            SharedChunk::destroy(chunk);
        }
    }

    /// `max_bump_extent` subtracts the header from `MAX_CHUNK_BYTES`;
    /// pin the relation so `- → +` mutation is caught.
    #[test]
    fn max_bump_extent_is_max_minus_header() {
        let header = SharedChunk::<Global>::header_size();
        let extent = max_bump_extent::<Global>();
        assert_eq!(extent, super::super::constants::MAX_CHUNK_BYTES - header);
        assert!(extent < super::super::constants::MAX_CHUNK_BYTES);
    }

    // Covers `inc_ref`'s refcount-overflow guard call site: forcing the
    // refcount to its saturation point and incrementing once routes through
    // `refcount_overflow_abort`, which panics (instead of aborting) under
    // `cfg(test)` so the otherwise-unreachable guard can be exercised.
    #[test]
    #[should_panic(expected = "refcount overflow")]
    fn inc_ref_overflow_triggers_abort_guard() {
        // SAFETY: single-threaded test. Allocate a real chunk, force its
        // refcount to the saturation point, then `inc_ref` to drive the
        // overflow guard. We catch the panic, restore the refcount so the
        // chunk can be safely destroyed (avoiding a Miri leak), then resume
        // unwinding so `should_panic` observes the original panic.
        unsafe {
            let chunk = SharedChunk::<Global>::allocate(Global, Weak::new(), 64).expect("allocate chunk");
            chunk.as_ref().set_ref_count_for_test(usize::MAX);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                chunk.as_ref().inc_ref();
            }));
            chunk.as_ref().set_ref_count_for_test(0);
            SharedChunk::destroy(chunk);
            std::panic::resume_unwind(result.expect_err("inc_ref must panic"));
        }
    }

    // Covers the `n == 0` early-return guards in `pre_credit_refs` /
    // `refund_refs` (no-op, refcount untouched) plus a non-zero
    // credit/refund round-trip that returns the count to its start.
    #[test]
    fn pre_credit_and_refund_zero_are_noops() {
        // SAFETY: single-threaded test owning the only references; we
        // restore the refcount to 0 before destroying the chunk.
        unsafe {
            let chunk = SharedChunk::<Global>::allocate(Global, Weak::new(), 64).expect("allocate chunk");
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
            SharedChunk::destroy(chunk);
        }
    }

    // Covers `pre_credit_refs`' overflow guard call site: forcing the
    // refcount to its saturation point and pre-crediting one more routes
    // through `refcount_overflow_abort`, which panics under `cfg(test)`.
    #[test]
    #[should_panic(expected = "refcount overflow")]
    fn pre_credit_refs_overflow_triggers_abort_guard() {
        // SAFETY: single-threaded test. Mirrors
        // `inc_ref_overflow_triggers_abort_guard`: drive the guard, catch
        // the panic, restore the refcount so the chunk destroys cleanly,
        // then resume unwinding so `should_panic` observes it.
        unsafe {
            let chunk = SharedChunk::<Global>::allocate(Global, Weak::new(), 64).expect("allocate chunk");
            chunk.as_ref().set_ref_count_for_test(usize::MAX);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                chunk.as_ref().pre_credit_refs(1);
            }));
            chunk.as_ref().set_ref_count_for_test(0);
            SharedChunk::destroy(chunk);
            std::panic::resume_unwind(result.expect_err("pre_credit_refs must panic"));
        }
    }
}
