// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Crate-private chunk-slot helpers for [`Arena`].
//!
//! This module holds the generic current-chunk state plus panic guards
//! for oversized one-shot allocations.

use core::cell::Cell;
use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use crate::internal::constants::LARGE;
use crate::internal::drop_list::DropEntry as InnerDropEntry;
use crate::internal::local_chunk::{LocalChunk, max_bump_extent as local_max_bump_extent};
use crate::internal::shared_chunk::{SharedChunk, max_bump_extent as shared_max_bump_extent};

/// Dispatches [`CurrentChunk`] over the two chunk flavors.
pub(super) trait ChunkKind {
    /// Pointer to the start of the chunk payload.
    ///
    /// # Safety
    ///
    /// `chunk` must be live (refcount-positive) for the duration of
    /// the call.
    unsafe fn data_ptr_of(chunk: NonNull<Self>) -> NonNull<u8>;

    /// Maximum bump-extent for normal-sized chunks of this flavor.
    fn max_bump_extent() -> usize;

    /// Reads the chunk's `capacity` field.
    ///
    /// # Safety
    ///
    /// `chunk` must be live (refcount-positive) for the duration of
    /// the call.
    unsafe fn capacity_of(chunk: NonNull<Self>) -> usize;
}

impl<A: Allocator + Clone> ChunkKind for LocalChunk<A> {
    #[inline(always)]
    unsafe fn data_ptr_of(chunk: NonNull<Self>) -> NonNull<u8> {
        // SAFETY: caller guarantees `chunk` is live.
        unsafe { Self::data_ptr(chunk) }
    }
    #[inline(always)]
    fn max_bump_extent() -> usize {
        local_max_bump_extent::<A>()
    }
    #[inline(always)]
    unsafe fn capacity_of(chunk: NonNull<Self>) -> usize {
        // SAFETY: caller guarantees `chunk` is live.
        unsafe { chunk.as_ref() }.capacity
    }
}

impl<A: Allocator + Clone> ChunkKind for SharedChunk<A> {
    #[inline(always)]
    unsafe fn data_ptr_of(chunk: NonNull<Self>) -> NonNull<u8> {
        // SAFETY: caller guarantees `chunk` is live.
        unsafe { Self::data_ptr(chunk) }
    }
    #[inline(always)]
    fn max_bump_extent() -> usize {
        shared_max_bump_extent::<A>()
    }
    #[inline(always)]
    unsafe fn capacity_of(chunk: NonNull<Self>) -> usize {
        // SAFETY: caller guarantees `chunk` is live.
        unsafe { chunk.as_ref() }.capacity
    }
}

/// Generic "current chunk" slot mirroring the bump-pointer state of an
/// installed chunk. Both the local (`Rc`/`Box`/`SimpleRef`) and shared
/// (`Arc`) pipelines use this type — see [`Arena::current_local`] /
/// [`Arena::current_shared`].
pub(super) struct CurrentChunk<C: ChunkKind + ?Sized> {
    /// `None` while the slot is in stub state (post-`new` / post-`reset`).
    /// In stub state the bump check fails naturally on the first
    /// allocation and falls into the cold refill path.
    pub(super) chunk: Cell<Option<NonNull<C>>>,
    /// Bump cursor (next free payload byte). `NonNull::dangling()` in
    /// stub state; never dereferenced because the bump check
    /// `end <= drop_back` fails when `drop_back == data_ptr == dangling`
    /// (both at address 1) for any nonzero `size`. ZST paths use
    /// `size.max(1)` to force the check to fail in stub state.
    pub(super) data_ptr: Cell<NonNull<u8>>,
    /// Limit pointer: start of the trailing drop-entry back-stack,
    /// equivalently one past the last free byte. Equals `data_ptr`
    /// (= `dangling`) in stub state; moves down as drop entries are
    /// installed.
    pub(super) drop_back: Cell<NonNull<u8>>,
    /// Per-tenure handle counter (Rc/Box for Local, Arc for Shared).
    /// Read at swap-out by `reconcile_swap_out` to subtract the unused
    /// portion of the `LARGE` refcount inflation in one operation.
    pub(super) smart_pointers_issued: Cell<usize>,
}

impl<C: ChunkKind + ?Sized> Default for CurrentChunk<C> {
    fn default() -> Self {
        Self {
            chunk: Cell::new(None),
            data_ptr: Cell::new(NonNull::dangling()),
            drop_back: Cell::new(NonNull::dangling()),
            smart_pointers_issued: Cell::new(0),
        }
    }
}

impl<C: ChunkKind + ?Sized> CurrentChunk<C> {
    /// Number of drop entries currently recorded in the back-stack of
    /// the chunk that this slot mirrors. Derived from the chunk's own
    /// payload extent and the slot's `drop_back` limit pointer, so the
    /// slot does not store it explicitly.
    ///
    /// # Safety
    ///
    /// `chunk` must be live (refcount-positive) for the duration of
    /// the call.
    #[inline]
    pub(super) unsafe fn drop_count(&self, chunk: NonNull<C>) -> u16 {
        // SAFETY: caller guarantees `chunk` is live.
        let cap = unsafe { C::capacity_of(chunk) };
        // SAFETY: caller guarantees `chunk` is live.
        let payload_base = unsafe { C::data_ptr_of(chunk) }.as_ptr() as usize;
        // Mirror the bump-extent cap applied at chunk install time so
        // this counter stays in sync with where the slot's `drop_back`
        // was actually initialized: the top of the back-stack is
        // `data + bump_extent`, not `data + capacity`. `refill_*`
        // bounds `cap <= MAX_CHUNK_BYTES` for any chunk that ever
        // becomes a current slot, so the cap is always applied.
        let bump_extent = cap.min(C::max_bump_extent());
        let payload_end = payload_base + bump_extent;
        let drop_back_addr = self.drop_back.get().as_ptr() as usize;
        let bytes = payload_end - drop_back_addr;
        // Capacity is bounded so the drop-entry count never reaches `u16::MAX`.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "current-slot chunks are bounded by MAX_CHUNK_BYTES = 64 KiB, yielding count <= 8192"
        )]
        let count = (bytes / core::mem::size_of::<InnerDropEntry>()) as u16;
        count
    }

    /// Bump `smart_pointers_issued`, aborting on overflow.
    ///
    /// We cap at `LARGE - 1` so swap-out reconciliation cannot underflow
    /// and still has one unit of headroom for local lazy pinning.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Overflow boundary unreachable: requires 2^62 outstanding refs.
    pub(super) fn bump_smart_pointers_issued(&self) {
        let n = self.smart_pointers_issued.get();
        // SAFETY: `LARGE - 1` needs about 2^62 live handles, so this is unreachable.
        unsafe { core::hint::assert_unchecked(n < LARGE - 1) };
        check_smart_pointers_issued_overflow(n);
        self.smart_pointers_issued.set(n + 1);
    }
}

pub(super) type CurrentLocalChunk<A> = CurrentChunk<LocalChunk<A>>;
pub(super) type CurrentSharedChunk<A> = CurrentChunk<SharedChunk<A>>;

/// Panic guard for local oversized allocations before final reconciliation.
pub(super) struct OversizedLocalGuard<A: Allocator + Clone> {
    pub(super) chunk: NonNull<LocalChunk<A>>,
}

impl<A: Allocator + Clone> Drop for OversizedLocalGuard<A> {
    fn drop(&mut self) {
        // SAFETY: chunk held LARGE; rcs_issued = 0, pinned = false → drops to zero, freed.
        unsafe { LocalChunk::reconcile_swap_out(self.chunk, 0, false) };
    }
}

/// Shared-flavor mirror of [`OversizedLocalGuard`].
pub(super) struct OversizedSharedGuard<A: Allocator + Clone> {
    pub(super) chunk: NonNull<SharedChunk<A>>,
}

impl<A: Allocator + Clone> Drop for OversizedSharedGuard<A> {
    fn drop(&mut self) {
        // SAFETY: chunk held LARGE; arcs_issued = 0 → drops to zero, freed.
        unsafe { SharedChunk::reconcile_swap_out(self.chunk, 0) };
    }
}

#[inline(always)]
#[expect(
    clippy::inline_always,
    reason = "must inline at every alloc site to avoid a per-call function-call overhead; see PERF.md"
)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)] // Refcount overflow requires physically unreachable outstanding refs.
pub(super) fn check_smart_pointers_issued_overflow(n: usize) {
    if n >= LARGE - 1 {
        crate::internal::constants::refcount_overflow_abort();
    }
}
