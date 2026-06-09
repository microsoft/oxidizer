// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multi-threaded reference-counted arena chunk.

// See note in `local_chunk.rs`: methods touching raw memory are `unsafe fn`
// with module-level safety contracts; we don't repeat the inner unsafe
// wrappers that edition 2024 requires by default.
#![allow(unsafe_op_in_unsafe_fn, reason = "see module doc: inner unsafe blocks in unsafe fn add noise here")]
#![allow(clippy::unnecessary_safety_comment, reason = "safety rationale documented at function level")]

use alloc::sync::Weak;
use core::alloc::Layout;
use core::cell::UnsafeCell;
use core::mem;
use core::ptr::{self, NonNull};
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
    drop_entry_count: AtomicU16,
    /// Explicit padding so the header size stays a multiple of 8, keeping
    /// the payload start 8-aligned. The payload start must be 8-aligned both
    /// for the `AtomicPtr<u8>` cache link stored there while the chunk is free
    /// (see [`cache_link`](Self::cache_link)) and for the `DropEntry`s the bump
    /// allocator packs against the payload tail (which are positioned relative
    /// to the payload start; see [`replay_drops`](super::drop_entry::replay_drops)).
    /// Without it, shrinking `drop_entry_count` from `usize` to `u16` would land
    /// the payload at a non-8-aligned offset, which is UB. This is temporary:
    /// once those payload-relative accesses are made tolerant of an unaligned
    /// payload base, this padding can be removed and the header shrunk.
    _padding: [u8; 6],
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

    #[inline]
    pub(crate) const fn header_size() -> usize {
        mem::offset_of!(Self, _padding) + mem::size_of::<[u8; 6]>()
    }

    #[inline]
    #[cfg_attr(test, mutants::skip)] // both branches saturate at CHUNK_ALIGN
    pub(crate) const fn struct_align() -> usize {
        let a = mem::align_of::<A>();
        let b = mem::align_of::<usize>();
        let base = if a >= b { a } else { b };
        if base >= CHUNK_ALIGN { base } else { CHUNK_ALIGN }
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
        let (raw_u8_ptr, _layout) =
            crate::internal::chunk_alloc::alloc_chunk_raw(&allocator, Self::header_size(), Self::struct_align(), payload_size)?;
        let fat: *mut Self = ptr::slice_from_raw_parts_mut(raw_u8_ptr, payload_size) as *mut Self;
        // SAFETY: see `LocalChunk::allocate`.
        unsafe {
            ptr::write(&raw mut (*fat).allocator, allocator);
            ptr::write(&raw mut (*fat).provider, provider);
            ptr::write(&raw mut (*fat).capacity, payload_size);
            ptr::write(&raw mut (*fat).ref_count, AtomicUsize::new(1));
            ptr::write(&raw mut (*fat).drop_entry_count, AtomicU16::new(0));
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
        let align = Self::struct_align();
        let header_ref = &*chunk.as_ptr();
        let capacity = header_ref.capacity;
        let drop_count = header_ref.drop_entry_count.load(Ordering::Acquire) as usize;
        replay_drops(Self::payload_ptr(chunk).as_ptr(), capacity, drop_count);
        let allocator: A = ptr::read(&raw const (*chunk.as_ptr()).allocator);
        ptr::drop_in_place(&raw mut (*chunk.as_ptr()).provider);
        let total = header + capacity;
        let layout = Layout::from_size_align(total, align).expect("matches allocate(); header+capacity stayed within isize::MAX");
        let raw_ptr = chunk.as_ptr().cast::<u8>();
        allocator.deallocate(NonNull::new_unchecked(raw_ptr), layout);
        drop(allocator);
    }

    /// Pointer to the `AtomicPtr<u8>` cache link stored in the first
    /// bytes of the chunk's payload. Cache stores thin pointers since
    /// `*mut Self` is fat for the DST.
    ///
    /// # Safety
    ///
    /// Chunk must be in the cached state.
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "SharedChunk payload is CHUNK_ALIGN-aligned; AtomicPtr fits within that alignment"
    )]
    #[inline]
    pub(crate) unsafe fn cache_link(chunk: NonNull<Self>) -> *const AtomicPtr<u8> {
        Self::payload_ptr(chunk).as_ptr().cast::<AtomicPtr<u8>>()
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
}

impl<A: Allocator + Clone> Chunk for SharedChunk<A> {
    #[inline]
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
}
