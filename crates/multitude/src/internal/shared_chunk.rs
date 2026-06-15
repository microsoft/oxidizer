// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multi-threaded reference-counted arena chunk.

// See note in `local_chunk.rs`: methods touching raw memory are `unsafe fn`
// with module-level safety contracts; we don't repeat the inner unsafe
// wrappers that edition 2024 requires by default.
#![allow(unsafe_op_in_unsafe_fn, reason = "see module doc: inner unsafe blocks in unsafe fn add noise here")]
#![allow(clippy::unnecessary_safety_comment, reason = "safety rationale documented at function level")]

use alloc::sync::Weak;
use core::cell::UnsafeCell;
use core::mem;
use core::ptr::{self, NonNull};
#[cfg(feature = "stats")]
use core::sync::atomic::AtomicU32;
use core::sync::atomic::{AtomicPtr, AtomicU16, AtomicUsize, Ordering, fence};

use allocator_api2::alloc::{AllocError, Allocator};

use super::chunk::Chunk;
use super::chunk_provider::ChunkProvider;
use super::constants::{CHUNK_ALIGN, refcount_overflow_abort};
use super::drop_entry::replay_drops;

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
    /// Intrusive cache-freelist link, used while the chunk sits on
    /// the provider's shared cache (refcount = 0). CAS-pushed and
    /// CAS-popped from any thread, so the storage is atomic. `null`
    /// when not on the list. Placed after `ref_count` (both 8-aligned)
    /// so the trailing `drop_entry_count` (`u16`) packs against
    /// `data` without end-of-struct padding.
    ///
    /// Unlike `LocalChunk::next`, this slot is *only* used for the
    /// cache freelist: shared chunks don't have a retired-list phase
    /// since handouts outlive the arena and chunks transition
    /// directly from refcount = 1 → 0 → cached (or destroyed).
    next: AtomicPtr<u8>,
    drop_entry_count: AtomicU16,
    /// Free bytes between the bump cursor and the drop-entry top at the
    /// time this chunk was retired from a `ChunkMutator`. Set in the
    /// mutator's `Drop` and read by [`ChunkProvider::release_shared`]
    /// to decrement the wasted-tail counter. Stays at 0 for chunks that
    /// never went through a mutator (e.g. preallocated cache fills).
    ///
    /// Read in `release_shared` after the chunk's atomic refcount has
    /// dropped to zero (with an acquire fence); the mutator's `Drop`
    /// performs the `set` before its own `dec_ref`, so the store is
    /// visible.
    #[cfg(feature = "stats")]
    wasted_at_retire: AtomicU32,
    /// Bump-payload tail. See `LocalChunk` for the
    /// [`UnsafeCell<u8>]` provenance rationale. The payload start is
    /// **not** required to be `DropEntry`-aligned:
    /// [`replay_drops`](super::drop_entry::replay_drops) aligns drop-
    /// entry positions via the absolute payload-end address.
    data: [UnsafeCell<u8>],
}

impl<A: Allocator + Clone> SharedChunk<A> {
    /// Borrow the non-owning back-pointer to the chunk's provider. The
    /// provider may have been dropped (a shared chunk can outlive its
    /// arena), so callers must `upgrade()` to use it.
    #[inline]
    pub(crate) fn provider(&self) -> &Weak<ChunkProvider<A>> {
        &self.provider
    }

    /// Reads the free byte count stashed by the owning `ChunkMutator`'s
    /// `Drop` (the gap between bump cursor and drop-entry top at retire).
    /// `0` for chunks that never went through a mutator.
    #[cfg(feature = "stats")]
    #[inline]
    pub(crate) fn wasted_at_retire(&self) -> u32 {
        // Acquire pairs with the `Release` store in `set_wasted_at_retire`;
        // shared chunks may be inspected on a different thread than the
        // one that performed the retire (the last `Arc::drop`).
        self.wasted_at_retire.load(Ordering::Acquire)
    }

    /// Stashes the chunk's wasted-tail bytes at retire time, to be
    /// subtracted from the provider's wasted-tail counter when the chunk
    /// is eventually released to the cache or destroyed.
    ///
    /// `Release` so cross-thread `release_shared` callers observe the
    /// stored value after their acquire fence on refcount = 0.
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
        // otherwise it's `drop_entry_count`. The `[UnsafeCell<u8>]` tail
        // has align 1 and sits flush against whichever it is.
        #[cfg(feature = "stats")]
        {
            mem::offset_of!(Self, wasted_at_retire) + mem::size_of::<AtomicU32>()
        }
        #[cfg(not(feature = "stats"))]
        {
            mem::offset_of!(Self, drop_entry_count) + mem::size_of::<AtomicU16>()
        }
    }

    #[inline]
    #[cfg_attr(test, mutants::skip)] // both branches saturate at CHUNK_ALIGN
    pub(crate) const fn struct_align() -> usize {
        let base = Self::value_align();
        if base >= CHUNK_ALIGN { base } else { CHUNK_ALIGN }
    }

    /// The chunk type's own alignment (`align_of::<Self>()`, ignoring the
    /// align-1 `[UnsafeCell<u8>]` tail): the max of `align_of::<A>()` and
    /// `align_of::<usize>()` (every other header field — the atomics and
    /// the `Weak` pointer — has alignment `<= align_of::<usize>()`).
    ///
    /// Used to round the allocation *size* (vs. [`Self::struct_align`],
    /// the larger *base*-address alignment). Pinned against the real
    /// `align_of_val` by `value_align_matches_real_alignment`.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // pure layout constant pinned by a dedicated test
    pub(crate) const fn value_align() -> usize {
        let a = mem::align_of::<A>();
        let b = mem::align_of::<usize>();
        if a >= b { a } else { b }
    }

    /// Recovers the chunk header (as a thin `*mut u8` carrying the
    /// chunk allocation's provenance) from a pointer into the chunk's
    /// payload by walking backwards through the chunk's `CHUNK_ALIGN`
    /// tile.
    ///
    /// Uses [`NonNull::byte_sub`] (provenance-preserving) rather than
    /// reconstituting the header pointer from an integer.
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
            ptr::write(&raw mut (*fat).drop_entry_count, AtomicU16::new(0));
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
        let drop_count = header_ref.drop_entry_count.load(Ordering::Acquire) as usize;
        replay_drops(Self::payload_ptr(chunk).as_ptr(), capacity, drop_count);
        let allocator: A = ptr::read(&raw const (*chunk.as_ptr()).allocator);
        ptr::drop_in_place(&raw mut (*chunk.as_ptr()).provider);
        let layout = crate::internal::chunk_alloc::chunk_layout(header, capacity, Self::value_align(), Self::struct_align())
            .expect("matches allocate(); header+capacity stayed within isize::MAX");
        let raw_ptr = chunk.as_ptr().cast::<u8>();
        allocator.deallocate(NonNull::new_unchecked(raw_ptr), layout);
        drop(allocator);
    }

    /// Pointer to the chunk's intrusive cache-freelist link
    /// (`AtomicPtr<u8>` storing a thin header pointer; cache stores
    /// thin pointers since `*mut Self` is fat for the DST). The field
    /// lives in the chunk header.
    ///
    /// # Safety
    ///
    /// Chunk must be allocated (header live); ordering of accesses
    /// through the returned pointer is the caller's responsibility.
    #[inline]
    pub(crate) unsafe fn cache_link(chunk: NonNull<Self>) -> *const AtomicPtr<u8> {
        &raw const (*chunk.as_ptr()).next
    }

    /// Re-initializes a chunk popped from the cache: refcount → 1,
    /// drop-entry count → 0. The caller becomes the +1 holder.
    ///
    /// # Safety
    ///
    /// `chunk` must be a freshly popped, refcount-zero, uniquely-owned
    /// chunk; the cache link is invalidated by this call.
    #[inline]
    pub(crate) unsafe fn reinit_for_acquire(chunk: NonNull<Self>) {
        // SAFETY: caller owns the unique reference; atomics are safe to
        // store unconditionally.
        let r = &*chunk.as_ptr();
        r.ref_count.store(1, Ordering::Relaxed);
        r.drop_entry_count.store(0, Ordering::Relaxed);
    }

    /// Loads the drop-entry count with `Acquire` ordering.
    ///
    /// The [`Chunk::drop_entry_count`](super::chunk::Chunk::drop_entry_count)
    /// accessor uses `Relaxed`, which suffices for the owner thread. This
    /// `Acquire` variant is for cross-thread readers (the deferred-init
    /// commit in [`Arc`](crate::Arc)): it pairs with the owner thread's
    /// `Release` publish in
    /// [`set_drop_entry_count`](super::chunk::Chunk::set_drop_entry_count)
    /// (via `ChunkMutator::publish_drop_count`) so the placeholder slot's
    /// bytes are visible before the count is read.
    #[inline]
    pub(crate) fn drop_entry_count_acquire(&self) -> usize {
        self.drop_entry_count.load(Ordering::Acquire) as usize
    }

    /// Overwrites the refcount. Test-only seam so unit tests can drive
    /// refcount-dependent paths (e.g. the overflow guard) without poking
    /// the field directly.
    #[cfg(test)]
    pub(crate) fn set_ref_count_for_test(&self, count: usize) {
        self.ref_count.store(count, Ordering::Relaxed);
    }

    /// Decrements `chunk`'s refcount on behalf of the caller, and if
    /// that drops the count to zero, routes the chunk back through
    /// [`teardown_and_release`](super::chunk_ops::ChunkOps::teardown_and_release).
    ///
    /// Used by smart-pointer drop paths ([`Box`](crate::Box),
    /// [`Arc`](crate::Arc)) and by [`ChunkMutator`](super::ChunkMutator)
    /// itself to share the "release one ref I am holding" sequence.
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

    /// Atomically reserves `n` additional strong references on this
    /// chunk in a single `fetch_add`, in addition to whatever the
    /// caller already holds. Aborts the process on overflow.
    ///
    /// Used by the arena's per-chunk surplus pre-credit: at chunk
    /// install time the arena reserves a large surplus of refs so
    /// per-allocation handouts can be tracked in a non-atomic local
    /// counter; the unused portion is returned to the chunk via
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

    /// Atomically returns `n` previously pre-credited but unused
    /// refs to the chunk's counter via `fetch_sub` with `Release`
    /// ordering. `Release` matches the existing per-ref `dec_ref`
    /// ordering so any writes the arena thread performed into the
    /// chunk are visible to other-thread holders that may observe
    /// the lower count.
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

    #[inline]
    fn drop_entry_count(&self) -> usize {
        self.drop_entry_count.load(Ordering::Relaxed) as usize
    }

    #[inline]
    fn set_drop_entry_count(&self, count: usize) {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "a 64KiB chunk holds at most 4096 drop entries (« u16::MAX); round-trip asserted below"
        )]
        let narrowed = count as u16;
        debug_assert_eq!(usize::from(narrowed), count, "drop-entry count exceeds u16 range");
        self.drop_entry_count.store(narrowed, Ordering::Release);
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
    /// 8 (`ref_count`) + 8 (`next`) + 2 (`drop_entry_count`) = 34 bytes.
    /// Under the `stats` feature an additional `wasted_at_retire:
    /// AtomicU32` is appended after 2 pad bytes (offset 36) for 40 bytes
    /// total. `next` is placed between `ref_count` and
    /// `drop_entry_count` so the trailing `u16` packs against `data`
    /// without padding when stats are off.
    #[test]
    fn header_size_for_global_matches_layout() {
        #[cfg(not(feature = "stats"))]
        assert_eq!(SharedChunk::<Global>::header_size(), 34);
        #[cfg(feature = "stats")]
        assert_eq!(SharedChunk::<Global>::header_size(), 40);
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

    /// `chunk_layout` must round the allocation *size* up to
    /// `value_align` (8) and set the *base* alignment to `struct_align`
    /// (`CHUNK_ALIGN`), but must NOT round the size up to `CHUNK_ALIGN`.
    /// Each cacheable size class must therefore produce an allocation
    /// whose size equals the class bytes, not 64 KiB.
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
