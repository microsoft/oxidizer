// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests crafted to address missed mutants identified by `cargo mutants`.
#![cfg(feature = "stats")]

use multitude::Arena;

// is_oversized_shared: threshold == max_normal_alloc routes via normal path
#[test]
fn is_oversized_shared_routes_at_threshold_via_normal() {
    const MNA: usize = 4 * 1024;
    let arena = Arena::builder().max_normal_alloc(MNA).build();
    let before_normal = arena.stats().normal_shared_chunks_allocated;
    let before_oversized = arena.stats().oversized_shared_chunks_allocated;
    // wcp = MNA (size MNA-1 + align 1).
    let _arc = arena.alloc_arc([0_u8; MNA - 1]);
    let after_normal = arena.stats().normal_shared_chunks_allocated;
    let after_oversized = arena.stats().oversized_shared_chunks_allocated;
    assert!(after_normal > before_normal);
    assert_eq!(
        after_oversized, before_oversized,
        "threshold must NOT route oversized (kills `>=` mutant)"
    );
}

#[test]
fn is_oversized_shared_routes_above_threshold_via_oversized() {
    const MNA: usize = 4 * 1024;
    let arena = Arena::builder().max_normal_alloc(MNA).build();
    let before_oversized = arena.stats().oversized_shared_chunks_allocated;
    let _arc = arena.alloc_arc([0_u8; MNA]); // wcp = MNA + 1
    let after_oversized = arena.stats().oversized_shared_chunks_allocated;
    assert!(
        after_oversized > before_oversized,
        "above-threshold must route oversized (kills `==` mutant)"
    );
}

#[test]
fn is_oversized_local_routes_at_threshold_via_normal() {
    const MNA: usize = 4 * 1024;
    let arena = Arena::builder().max_normal_alloc(MNA).build();
    let before_normal = arena.stats().normal_local_chunks_allocated;
    let before_oversized = arena.stats().oversized_local_chunks_allocated;
    let s = "x".repeat(MNA);
    let _r: &mut str = arena.alloc_str(&s);
    let after_normal = arena.stats().normal_local_chunks_allocated;
    let after_oversized = arena.stats().oversized_local_chunks_allocated;
    assert!(after_normal > before_normal);
    assert_eq!(after_oversized, before_oversized, "threshold must NOT route oversized");
}

#[test]
fn is_oversized_local_routes_above_threshold_via_oversized() {
    const MNA: usize = 4 * 1024;
    let arena = Arena::builder().max_normal_alloc(MNA).build();
    let before_oversized = arena.stats().oversized_local_chunks_allocated;
    let s = "x".repeat(MNA + 1);
    let _r: &mut str = arena.alloc_str(&s);
    let after_oversized = arena.stats().oversized_local_chunks_allocated;
    assert!(after_oversized > before_oversized);
}

// Vec::shrink_to_fit boundary: total < mna must reclaim (catches `==`/`>=`
// mutants that would early-return at total == mna and below).
#[test]
fn shrink_to_fit_reclaims_strictly_below_max_normal_alloc() {
    let mna = 4 * 1024;
    let arena: Arena = Arena::builder().max_normal_alloc(mna).build();
    // cap = mna - 1 ensures refill_hint = cap + 1 = mna <= mna, so the Vec
    // is allocated in the normal current_local chunk (not oversized) and
    // its end IS at the bump cursor. `total_bytes = cap = mna - 1`,
    // strictly below the threshold.
    let cap = mna - 1;
    let mut v: multitude::vec::Vec<'_, u8> = arena.alloc_vec_with_capacity(cap);
    v.extend_from_slice([7_u8; 16]);
    assert_eq!(v.capacity(), cap);
    v.shrink_to_fit();
    assert_eq!(v.capacity(), v.len(), "Vec strictly below max_normal_alloc must reclaim tail");
}

// Post-reset cache reuse for the floor-bump `==`/`<` mutants on
// chunk_provider:219. With the mutant in effect, the floor never
// advances (or only at no-op intervals), so post-reset the cache
// holds chunks of mixed (smaller) classes. A subsequent alloc at the
// saturated class then pops a smaller chunk, fails to fit, refills
// → allocates a fresh chunk. Original code: post-reset alloc at the
// saturated class pops a class-7 chunk and reuses it (no fresh alloc).
#[test]
fn local_cache_floor_advances_so_post_reset_alloc_reuses_chunk() {
    let mut arena = Arena::new();
    // Drive next_local_class up to its saturated value by issuing enough
    // local refills. Each `alloc_str` of a string larger than the current
    // chunk forces a refill and advances the ratchet.
    let stride = 1024_usize;
    for _ in 0..8 {
        let s = "y".repeat(stride);
        let _r = arena.alloc_str(&s);
    }
    let before_reset = arena.stats().normal_local_chunks_allocated;
    arena.reset();
    // After reset, retired_local clears → chunks go to cache. Floor
    // should equal the saturated class so only saturated-class chunks
    // are retained (smaller ones returned to system).
    //
    // Allocate a single small value: the refill triggers acquire_local
    // at the saturated ratchet class. With the original code, the
    // cache pops a saturated-class chunk → no fresh allocation.
    // With `<` / `==` mutants, the floor never grew → cache holds
    // mixed-class chunks → pop returns one but it might be too small
    // → refill spin → MORE fresh allocations.
    let _ = arena.alloc(0_u8);
    let after_reset = arena.stats().normal_local_chunks_allocated;
    // The fresh-alloc count should NOT explode after the small alloc.
    // Original: at most 1 additional fresh alloc (cache miss for the
    // saturated class). Mutant: many more as the alloc spins through
    // smaller cached chunks that don't fit subsequent refills.
    assert!(
        after_reset - before_reset <= 1,
        "post-reset alloc must reuse cached saturated-class chunk; got {} fresh allocs (kills floor-bump mutants)",
        after_reset - before_reset,
    );
}
