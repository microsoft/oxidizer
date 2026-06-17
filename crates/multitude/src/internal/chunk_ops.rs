// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Per-flavor chunk lifecycle and access operations.
//!
//! [`ChunkOps`] lets [`ChunkMutator`](super::chunk_mutator::ChunkMutator) handle
//! [`LocalChunk`](super::local_chunk::LocalChunk) and [`SharedChunk`](super::shared_chunk::SharedChunk)
//! through one lifecycle interface.

// All trait methods are `unsafe fn` with documented safety contracts at the
// function level; the inner unsafe wrappers required by edition 2024 add
// noise without any additional safety boundary, so we suppress the lint.
#![allow(unsafe_op_in_unsafe_fn, reason = "see module doc: inner unsafe blocks in unsafe fn add noise here")]
#![allow(clippy::unnecessary_safety_comment, reason = "safety rationale documented at function level")]

use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use super::chunk::Chunk;
use super::chunk_alloc::chunk_alloc_size;
use super::local_chunk::LocalChunk;
use super::shared_chunk::SharedChunk;

/// Operations every chunk flavor must support.
///
/// Implemented for [`LocalChunk<A>`] and [`SharedChunk<A>`].
pub(crate) trait ChunkOps: Chunk {
    /// Allocator type used to back this chunk flavor's underlying storage.
    type Allocator: Allocator + Clone;

    /// Whether this chunk flavor stores per-allocation drop entries packed at
    /// its payload tail.
    ///
    /// `true` only for [`LocalChunk`]: plain arena references (`&mut T` /
    /// `&mut [T]`) have no destructor of their own, so the chunk runs them at
    /// teardown. `false` for [`SharedChunk`], whose values are owned by `Box`
    /// or `Arc` and dropped eagerly on their last reference. The
    /// [`ChunkMutator`](super::chunk_mutator::ChunkMutator) keys all its
    /// drop-entry bookkeeping off this const so the shared monomorphization
    /// compiles the dead paths away.
    const REGISTERS_DROPS: bool;

    /// Header size in bytes for this chunk flavor.
    fn header_size() -> usize;

    /// Publishes the mutator's locally-tracked drop-entry count into the chunk
    /// header so teardown can replay them. A no-op for flavors that never
    /// register drop entries ([`Self::REGISTERS_DROPS`] is `false`).
    ///
    /// # Safety
    ///
    /// `chunk` must reference a live chunk the caller holds a reference to.
    unsafe fn publish_drop_entry_count(chunk: NonNull<Self>, count: usize);

    /// Payload alignment for this chunk flavor.
    fn value_align() -> usize;

    /// Rounded backing-allocation size (`Layout::size()`) of a chunk whose
    /// payload holds `payload` bytes. The single source of truth for chunk
    /// byte accounting: every reserve/release/cache path routes through here
    /// so the rounded footprint stays balanced.
    #[inline]
    fn footprint(payload: usize) -> Result<usize, AllocError> {
        chunk_alloc_size(Self::header_size(), payload, Self::value_align())
    }

    /// Returns a pointer to the first byte of the chunk's payload.
    ///
    /// # Safety
    ///
    /// `chunk` must reference a live (non-deallocated) chunk.
    unsafe fn payload_ptr(chunk: NonNull<Self>) -> NonNull<u8>;

    /// Routes `chunk` (refcount zero, drops already replayed) back to the
    /// provider's cache, or deallocates it outright if the provider is dead.
    ///
    /// # Safety
    ///
    /// Caller must hold the unique remaining reference to `chunk`.
    unsafe fn teardown_and_release(chunk: NonNull<Self>);

    /// Records wasted tail on retire; the provider subtracts it when the
    /// chunk is later cached or destroyed.
    ///
    /// # Safety
    ///
    /// `chunk` must reference a live chunk. Caller (the mutator) holds at
    /// least one reference.
    #[cfg(feature = "stats")]
    unsafe fn record_retire(chunk: NonNull<Self>, wasted: u32);
}

#[allow(
    clippy::use_self,
    reason = "must call inherent methods, not the trait Self methods, to avoid infinite recursion"
)]
impl<A: Allocator + Clone> ChunkOps for LocalChunk<A> {
    type Allocator = A;

    const REGISTERS_DROPS: bool = true;

    #[inline]
    fn header_size() -> usize {
        LocalChunk::<A>::header_size()
    }

    #[inline]
    unsafe fn publish_drop_entry_count(chunk: NonNull<Self>, count: usize) {
        // SAFETY: caller holds a live reference to `chunk`.
        chunk.as_ref().set_drop_entry_count(count);
    }

    #[inline]
    fn value_align() -> usize {
        LocalChunk::<A>::value_align()
    }

    #[inline]
    unsafe fn payload_ptr(chunk: NonNull<Self>) -> NonNull<u8> {
        // SAFETY: delegated to the inherent `LocalChunk::payload_ptr`.
        LocalChunk::payload_ptr(chunk)
    }

    #[cold]
    #[inline(never)]
    unsafe fn teardown_and_release(chunk: NonNull<Self>) {
        // SAFETY: caller owns the unique remaining reference. We must replay
        // any pending drop entries here (and clear the count) because the
        // cache reuses the first bytes of the payload as a next-link, which
        // would corrupt the first value if its drop ran after recycling.
        let chunk_ref = &*chunk.as_ptr();
        let drop_count = chunk_ref.drop_entry_count();
        if drop_count != 0 {
            let payload = LocalChunk::payload_ptr(chunk).as_ptr();
            let capacity = chunk_ref.capacity();
            super::drop_entry::replay_drops(payload, capacity, drop_count);
            chunk_ref.set_drop_entry_count(0);
        }
        // Local chunks teardown while the arena provider is still alive; cached
        // local chunks are destroyed directly from provider drop.
        let provider = chunk_ref.provider();
        debug_assert!(!provider.is_null(), "local-chunk provider back-pointer is null in teardown");
        (*provider).release_local(chunk);
    }

    #[cfg(feature = "stats")]
    unsafe fn record_retire(chunk: NonNull<Self>, wasted: u32) {
        let chunk_ref = &*chunk.as_ptr();
        chunk_ref.set_wasted_at_retire(wasted);
        let provider = chunk_ref.provider();
        debug_assert!(!provider.is_null(), "local-chunk provider back-pointer is null at retire");
        (*provider).record_wasted_tail(u64::from(wasted));
    }
}

#[allow(
    clippy::use_self,
    reason = "must call inherent methods, not the trait Self methods, to avoid infinite recursion"
)]
impl<A: Allocator + Clone> ChunkOps for SharedChunk<A> {
    type Allocator = A;

    const REGISTERS_DROPS: bool = false;

    #[inline]
    fn header_size() -> usize {
        SharedChunk::<A>::header_size()
    }

    #[inline]
    unsafe fn publish_drop_entry_count(_chunk: NonNull<Self>, _count: usize) {
        // Shared chunks never register drop entries; nothing to publish.
    }

    #[inline]
    fn value_align() -> usize {
        SharedChunk::<A>::value_align()
    }

    #[inline]
    unsafe fn payload_ptr(chunk: NonNull<Self>) -> NonNull<u8> {
        // SAFETY: delegated to the inherent `SharedChunk::payload_ptr`.
        SharedChunk::payload_ptr(chunk)
    }

    #[cold]
    #[inline(never)]
    unsafe fn teardown_and_release(chunk: NonNull<Self>) {
        // SAFETY: caller owns the unique remaining reference. Shared chunks
        // register no drop entries; per-`Arc` values drop on their last strong
        // reference before the chunk reaches the cache.
        let chunk_ref = &*chunk.as_ptr();
        // Shared chunks can outlive their provider, so release through `Weak`.
        if let Some(provider) = chunk_ref.provider().upgrade() {
            provider.release_shared(chunk);
        } else {
            SharedChunk::destroy(chunk);
        }
    }

    #[cfg(feature = "stats")]
    unsafe fn record_retire(chunk: NonNull<Self>, wasted: u32) {
        let chunk_ref = &*chunk.as_ptr();
        chunk_ref.set_wasted_at_retire(wasted);
        // If the provider is gone, no stats counter remains to update.
        if let Some(provider) = chunk_ref.provider().upgrade() {
            provider.record_wasted_tail(u64::from(wasted));
        }
    }
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    use super::*;

    // Kills the `value_align -> 1` mutants on both `ChunkOps` impls: the
    // trait method must report the real payload alignment
    // (`align_of::<usize>()`), which every footprint computation depends on.
    // The inherent `value_align` tests don't cover the trait impls.
    #[test]
    fn chunk_ops_value_align_reports_real_payload_alignment() {
        assert_eq!(
            <LocalChunk<Global> as ChunkOps>::value_align(),
            core::mem::align_of::<usize>(),
            "LocalChunk trait value_align must match the real payload alignment"
        );
        assert_eq!(
            <SharedChunk<Global> as ChunkOps>::value_align(),
            core::mem::align_of::<usize>(),
            "SharedChunk trait value_align must match the real payload alignment"
        );
    }
}
