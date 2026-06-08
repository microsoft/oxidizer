// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Per-flavor chunk lifecycle and access operations.
//!
//! [`ChunkOps`] is the trait that [`ChunkMutator`](super::ChunkMutator) uses
//! to manipulate either a [`LocalChunk`](super::LocalChunk) or a
//! [`SharedChunk`](super::SharedChunk) without caring which flavor it has.
//! It also drives the "refcount hit zero" teardown path, which depends on
//! the flavor: local chunks return to the provider's single-threaded cache,
//! shared chunks return to the provider's lock-free cache.

// All trait methods are `unsafe fn` with documented safety contracts at the
// function level; the inner unsafe wrappers required by edition 2024 add
// noise without any additional safety boundary, so we suppress the lint.
#![allow(unsafe_op_in_unsafe_fn, reason = "see module doc: inner unsafe blocks in unsafe fn add noise here")]
#![allow(clippy::unnecessary_safety_comment, reason = "safety rationale documented at function level")]

use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use super::chunk::Chunk;
use super::local_chunk::LocalChunk;
use super::shared_chunk::SharedChunk;

/// Operations every chunk flavor must support.
///
/// Implemented for [`LocalChunk<A>`] and [`SharedChunk<A>`]. The associated
/// `Allocator` type lets generic callers recover the provider type for
/// release-routing.
pub(crate) trait ChunkOps: Chunk {
    /// Allocator type used to back this chunk flavor's underlying storage.
    type Allocator: Allocator + Clone;

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
}

#[allow(
    clippy::use_self,
    reason = "must call inherent methods, not the trait Self methods, to avoid infinite recursion"
)]
impl<A: Allocator + Clone> ChunkOps for LocalChunk<A> {
    type Allocator = A;

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
        // Route the just-released chunk back to the provider. The provider
        // is guaranteed to outlive every local-chunk teardown: the arena's
        // `provider: Arc<ChunkProvider>` field is declared after the
        // chunk-holding fields, so on `Arena::drop` the local mutators
        // tear down first (running this code) while the provider Arc is
        // still alive; chunks parked in the provider's own cache are torn
        // down directly via `LocalChunk::destroy` in `drain_all` and do
        // not reach this code path. See the type-level doc on
        // `LocalChunk`.
        let provider = chunk_ref.provider();
        debug_assert!(!provider.is_null(), "local-chunk provider back-pointer is null in teardown");
        (*provider).release_local(chunk);
    }
}

#[allow(
    clippy::use_self,
    reason = "must call inherent methods, not the trait Self methods, to avoid infinite recursion"
)]
impl<A: Allocator + Clone> ChunkOps for SharedChunk<A> {
    type Allocator = A;

    #[inline]
    unsafe fn payload_ptr(chunk: NonNull<Self>) -> NonNull<u8> {
        // SAFETY: delegated to the inherent `SharedChunk::payload_ptr`.
        SharedChunk::payload_ptr(chunk)
    }

    #[cold]
    #[inline(never)]
    unsafe fn teardown_and_release(chunk: NonNull<Self>) {
        // SAFETY: see local variant. Replay drops + clear count before the
        // chunk is recycled to the shared cache (where its payload's first
        // bytes are reused as a Treiber-stack next-link).
        let chunk_ref = &*chunk.as_ptr();
        let drop_count = chunk_ref.drop_entry_count();
        if drop_count != 0 {
            let payload = SharedChunk::payload_ptr(chunk).as_ptr();
            let capacity = chunk_ref.capacity();
            super::drop_entry::replay_drops(payload, capacity, drop_count);
            chunk_ref.set_drop_entry_count(0);
        }
        // Shared chunks CAN outlive their provider (an Arc<T> backed by
        // a shared chunk can be held past Arena::drop), so we still need
        // the Weak::upgrade dance here.
        if let Some(provider) = chunk_ref.provider().upgrade() {
            provider.release_shared(chunk);
        } else {
            SharedChunk::destroy(chunk);
        }
    }
}

// Note: the prior `orphan_local_chunk_is_destroyed_on_mutator_drop` test
// (which exercised the now-removed `destroy_orphan_local` defensive arm)
// is gone — that branch was eliminated when `LocalChunk` switched from a
// `Weak<ChunkProvider>` to a non-owning raw back-pointer. See the
// type-level doc on `LocalChunk` for the soundness argument and
// `teardown_and_release` above for the simplified routing.
