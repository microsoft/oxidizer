// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cold refill paths for `current_local` and `current_shared`.
//! They swap out the active chunk and install a fresh one when the
//! bump fast paths run out of room.

use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use super::Arena;
use crate::internal::constants::DEFAULT_MIN_PAYLOAD;
use crate::internal::local_chunk::{LocalChunk, max_bump_extent as local_max_bump_extent};
use crate::internal::shared_chunk::{SharedChunk, max_bump_extent as shared_max_bump_extent};
use crate::internal::sync::Ordering;

// These helpers reject oversized `min_payload` up front, so the current
// slot is never replaced with an oversized chunk.

impl<A: Allocator + Clone> Arena<A> {
    /// Reconcile and refill the `current_shared` slot.
    ///
    /// On reconciliation: if `current_shared` holds a chunk, do a
    /// single `fetch_sub(LARGE - arcs_issued, Release)` on
    /// its atomic refcount; if the count just hit zero, route the
    /// chunk back to its provider (the chunk has no live `Arc`s).
    /// Otherwise the chunk continues to live independently — its
    /// outstanding `Arc::drop`s will eventually drive it to zero.
    #[cold]
    #[inline(never)]
    #[cfg_attr(test, mutants::skip)] // Defense-in-depth guards and capacity-class routing are all equivalent under reachable inputs.
    pub(super) fn refill_shared(&self, min_payload: usize) -> Result<(), AllocError> {
        // Defense-in-depth: reject oversized requests here too so the mask invariant holds.
        if min_payload > shared_max_bump_extent::<A>() {
            return Err(AllocError);
        }
        if let Some(prev) = self.current_shared.chunk.replace(None) {
            #[cfg(feature = "stats")]
            {
                let data_ptr_addr = self.current_shared.data_ptr.get().as_ptr() as usize;
                let drop_back_addr = self.current_shared.drop_back.get().as_ptr() as usize;
                let wasted = drop_back_addr.saturating_sub(data_ptr_addr);
                crate::arena_stats::StatsStorage::add(&self.provider.stats.wasted_tail_bytes, wasted as u64);
            }

            // Persist the mirrored drop count before swap-out may replay drops.
            // SAFETY: chunk held the LARGE inflation while current.
            unsafe {
                (*prev.as_ptr())
                    .drop_count
                    .store(self.current_shared.drop_count(prev), Ordering::Release);
            }
            let arcs_issued = self.current_shared.smart_pointers_issued.replace(0);
            // Clear the mirror state before reconcile may run user `Drop`.
            // A reentrant `alloc_arc` must see an empty slot, not stale cells.
            self.current_shared.data_ptr.set(NonNull::dangling());
            self.current_shared.drop_back.set(NonNull::dangling());
            // SAFETY: chunk held the LARGE inflation.
            unsafe { SharedChunk::reconcile_swap_out(prev, arcs_issued) };

            // A reentrant `Drop` may already have installed a fresh chunk.
            if self.current_shared.chunk.get().is_some() {
                return Ok(());
            }
        }

        let want = min_payload.max(DEFAULT_MIN_PAYLOAD);
        let chunk = self.provider.acquire_shared(want)?;
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let data = unsafe { SharedChunk::<A>::data_ptr(chunk) };
        // SAFETY: chunk live — acquired with inflated refcount.
        let capacity = unsafe { chunk.as_ref() }.capacity;

        // Cap the bump cursor at the mask-recoverable region.
        let bump_extent = capacity.min(shared_max_bump_extent::<A>());

        // SAFETY: `data + bump_extent` is a one-past-the-end limit
        // pointer (never dereferenced); `bump_extent > 0`.
        let drop_back = unsafe { NonNull::new_unchecked(data.as_ptr().add(bump_extent)) };

        self.current_shared.chunk.set(Some(chunk));
        self.current_shared.data_ptr.set(data);
        self.current_shared.drop_back.set(drop_back);
        self.current_shared.smart_pointers_issued.set(0);

        Ok(())
    }

    /// Promote the current chunk into the pin list (if it has handed
    /// out simple references), or reconcile its inflated refcount
    /// (otherwise). Then acquire a fresh chunk and install it as
    /// `current_local`.
    #[cold]
    #[inline(never)]
    #[cfg_attr(test, mutants::skip)] // Defense-in-depth guards and capacity-class routing are all equivalent under reachable inputs.
    pub(super) fn refill_local(&self, min_payload: usize) -> Result<(), AllocError> {
        // Defense-in-depth: reject oversized requests here too; allowing
        // them would break the chunk-header mask trick.
        if min_payload > local_max_bump_extent::<A>() {
            return Err(AllocError);
        }
        if let Some(prev) = self.current_local.chunk.replace(None) {
            #[cfg(feature = "stats")]
            {
                let data_ptr_addr = self.current_local.data_ptr.get().as_ptr() as usize;
                let drop_back_addr = self.current_local.drop_back.get().as_ptr() as usize;
                let wasted = drop_back_addr.saturating_sub(data_ptr_addr);
                crate::arena_stats::StatsStorage::add(&self.provider.stats.wasted_tail_bytes, wasted as u64);
            }

            // Persist the mirrored drop count back into the chunk.
            // `mirror_dc` (back-stack distance) is the source of truth;
            // `chunk_dc` is a redundant counter maintained by
            // `bump_*_drop_count`. They must agree (and the
            // debug_assert catches divergence in debug builds). We
            // store `mirror_dc` unconditionally: if a future regression
            // ever caused `chunk_dc > mirror_dc`, taking `.max()` would
            // make `replay_drops` walk past the last written entry and
            // call a garbage `drop_fn` — much worse than under-replay
            // (which only leaks `T::drop`).
            // SAFETY: chunk held the LARGE inflation while current.
            unsafe {
                let mirror_dc = self.current_local.drop_count(prev);
                let chunk_dc = (*prev.as_ptr()).drop_count.get();
                debug_assert_eq!(mirror_dc, chunk_dc, "drop_count mirror diverged from on-chunk counter");
                let _ = chunk_dc;
                (*prev.as_ptr()).drop_count.set(mirror_dc);
            }

            // Snapshot the per-tenure counters, then clear the mirror
            // state so any reentrant `alloc_*` sees an empty slot.
            let was_pinned = self.current_local_pinned.replace(false);
            let rcs_issued = self.current_local.smart_pointers_issued.replace(0);
            self.current_local.data_ptr.set(NonNull::dangling());
            self.current_local.drop_back.set(NonNull::dangling());

            if was_pinned {
                // Transfer one `+1` into the pin list; reconcile leaves
                // `rcs_issued + 1` behind for that entry.
                let head = self.pinned_local.replace(None);
                // SAFETY: chunk held the LARGE inflation — still alive.
                unsafe { (*prev.as_ptr()).next.set(head) };
                self.pinned_local.set(Some(prev));
            }
            // SAFETY: chunk held the LARGE inflation. Reconcile may
            // route the chunk home if it just hit zero (only possible
            // when `was_pinned == false` and `rcs_issued == 0`).
            unsafe { LocalChunk::reconcile_swap_out(prev, rcs_issued, was_pinned) };

            // A reentrant `Drop` may already have installed a fresh chunk.
            // If so, leave it alone and let the caller retry.
            if self.current_local.chunk.get().is_some() {
                return Ok(());
            }
        }

        let want = min_payload.max(DEFAULT_MIN_PAYLOAD);
        let chunk = self.provider.acquire_local(want)?;
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let data = unsafe { LocalChunk::<A>::data_ptr(chunk) };
        // SAFETY: refcount-positive — acquired with refcount inflated to LARGE.
        let capacity = unsafe { chunk.as_ref() }.capacity;

        // Cap the bump cursor at the mask-recoverable region.
        let bump_extent = capacity.min(local_max_bump_extent::<A>());

        // SAFETY: `data + bump_extent` is a one-past-the-end limit
        // pointer (never dereferenced); `bump_extent > 0`.
        let drop_back = unsafe { NonNull::new_unchecked(data.as_ptr().add(bump_extent)) };

        self.current_local.chunk.set(Some(chunk));
        self.current_local.data_ptr.set(data);
        self.current_local.drop_back.set(drop_back);
        self.current_local_pinned.set(false);
        self.current_local.smart_pointers_issued.set(0);

        Ok(())
    }
}
