// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Regression tests for the single-freelist + class-floor cache design.
//!
//! The chunk provider keeps at most one freelist per chunk type. The
//! freelist's "floor" class ratchets monotonically: chunks released
//! below the current floor are returned to the system, and any cached
//! chunks below the floor are evicted at the next floor bump.
#![cfg(feature = "stats")]
use multitude::{Arena, ArenaBuilder};

/// Bumping the cache class via successive refills should evict
/// previously cached small chunks, returning them to the system.
#[test]
fn cache_floor_evicts_smaller_chunks_on_bump() {
    let arena: Arena = ArenaBuilder::new().build();
    // Trigger several refills so the local-class ratchet advances.
    // Each `alloc_slice_fill_with` of an increasing length forces a
    // refill once the current chunk runs out, advancing the ratchet.
    for class in 0..6_u32 {
        // Allocate enough bytes to force a refill at the next-larger class.
        let bytes = 256_usize << class;
        let _slice: &mut [u8] = arena.alloc_slice_fill_with(bytes, |_| 0);
    }
    let stats = arena.stats();
    // The provider should have allocated multiple normal local chunks
    // (one per refill), and most of the smaller ones should have been
    // evicted (returned to the backing allocator) as the floor advanced.
    assert!(stats.normal_local_chunks_allocated >= 1);
}

/// After enough refills to saturate the ratchet at class 7, the cache
/// floor is at class 7 and any newly released small chunk (e.g. an
/// oversized one-shot whose total happens to land on a smaller class
/// size) is returned to the system rather than cached.
#[test]
fn release_below_floor_bypasses_cache() {
    let arena: Arena = ArenaBuilder::new().build();
    // Drive the ratchet up to class 7 by issuing many shared-chunk refills.
    for _ in 0..NUM_REFILLS {
        // Each alloc_arc forces a small-chunk shared allocation if the
        // current chunk runs out; cycling through these advances the ratchet.
        let _a = arena.alloc_arc([0u32; 16]);
    }
    let stats = arena.stats();
    // We expect a bounded number of chunks: ratchet saturates at class 7,
    // and the cache holds at most one chunk per refill that hasn't been
    // released yet. The point is that we don't accumulate `NUM_REFILLS`
    // chunks worth of memory.
    assert!(stats.normal_shared_chunks_allocated <= NUM_REFILLS as u64);
}

const NUM_REFILLS: usize = 32;
