// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Mutation-test kills for `Arena::max_normal_alloc` routing boundaries and
//! `Vec::shrink_to_fit`'s oversized-route bypass. Each test targets a specific
//! `cargo mutants` finding flagged as MISSED.

#![allow(clippy::std_instead_of_core, reason = "tests use std")]
#![allow(clippy::unwrap_used, reason = "test code")]

use multitude::Arena;
use multitude::vec::Vec as ArenaVec;

// --- ChunkProvider::config (kills `config -> Default`) -----------------------
//
// `config().max_normal_alloc` decides whether an allocation routes to
// the normal-cache size classes or to a one-shot oversized chunk. Set a
// non-default `max_normal_alloc` well below the default `16 * 1024` and
// allocate at a size between the two: the original config gates it to
// oversized, the mutant's `Default::default()` keeps it on the normal
// path. `oversized_*_chunks_allocated` stats expose the routing.

#[cfg(feature = "stats")]
#[test]
fn chunk_provider_config_returns_custom_max_normal_alloc_local() {
    // Default max_normal_alloc = 16 KiB. Set 4 KiB and request a 12 KiB
    // local allocation: original → oversized; mutant (default) → normal.
    let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();
    let baseline = arena.stats().oversized_chunks_allocated;
    let _: &mut [u8] = arena.alloc_slice_fill_with::<u8, _>(12 * 1024, |_| 0);
    let after = arena.stats().oversized_chunks_allocated;
    assert!(
        after > baseline,
        "12 KiB local allocation with 4 KiB max_normal_alloc must route to an oversized chunk; stats: {after} vs baseline {baseline}",
    );
}

#[cfg(feature = "stats")]
#[test]
fn chunk_provider_config_returns_custom_max_normal_alloc_shared() {
    let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();
    let baseline = arena.stats().oversized_chunks_allocated;
    // `alloc_str_box` runs through the shared-chunk path
    // (`impl_alloc_str_box_prefixed_shared`).
    let s: std::string::String = (0..12 * 1024).map(|_| 'a').collect();
    let _ = arena.alloc_str_box(&s);
    let after = arena.stats().oversized_chunks_allocated;
    assert!(
        after > baseline,
        "12 KiB shared allocation with 4 KiB max_normal_alloc must route to an oversized chunk; stats: {after} vs baseline {baseline}",
    );
}

// --- ChunkProvider::acquire_local / acquire_shared first-`>` boundary --------
//
// `if min_payload > self.config.max_normal_alloc || ... { allocate_oversized }`.
// `>` → `>=` flips routing at `min_payload == max_normal_alloc`: the
// original routes through the normal cache; the mutant escapes to
// `allocate_oversized_*`. Stats expose the difference via
// `oversized_chunks_allocated` / `oversized_chunks_allocated`.
// Without the `stats` feature we still observe routing through
// `Arena::max_normal_alloc` plus a successful normal-sized allocation
// that the mutant would mis-route. We use the `stats` feature here.

#[cfg(feature = "stats")]
#[test]
fn acquire_local_at_max_normal_alloc_boundary_stays_normal_class() {
    let mna = 4 * 1024;
    let arena: Arena = Arena::builder().max_normal_alloc(mna).build();
    let baseline = arena.stats().oversized_chunks_allocated;
    // `worst_case_slice_payload::<u8>(len) = len * 1 + align_of::<u8>()
    //  = len + 1`; choose `len == mna - 1` so the refill_hint =
    // `min_payload` arrives at `acquire_local` exactly equal to
    // `max_normal_alloc`. Original `>` keeps this on the normal path;
    // mutant `>=` routes to oversized.
    let len = mna - 1;
    let _: &mut [u8] = arena.alloc_slice_fill_with::<u8, _>(len, |_| 0);
    let after = arena.stats().oversized_chunks_allocated;
    assert_eq!(
        after - baseline,
        0,
        "min_payload == max_normal_alloc must stay on the normal cache path",
    );
}

#[cfg(feature = "stats")]
#[test]
fn acquire_shared_at_max_normal_alloc_boundary_stays_normal_class() {
    let mna = 4 * 1024;
    let arena: Arena = Arena::builder().max_normal_alloc(mna).build();
    let baseline = arena.stats().oversized_chunks_allocated;
    // `impl_alloc_str_box_prefixed_shared` calls
    // `refill_shared(words * size_of::<usize>())`. With
    // `words = 1 + len.div_ceil(8).max(1)`, picking `len` so that
    // `words * 8 == mna` lands at the boundary. For `mna = 4096`,
    // `words = 512`, so we need `1 + ceil(len/8) == 512` ⇒
    // `ceil(len/8) == 511` ⇒ `len in 4081..=4088` covers it; choose
    // 4088 for a clean multiple-of-8 boundary.
    let len: usize = 4088;
    debug_assert_eq!(1 + len.div_ceil(core::mem::size_of::<usize>()).max(1), 512);
    let s: std::string::String = (0..len).map(|_| 'a').collect();
    let _ = arena.alloc_str_box(&s);
    let after = arena.stats().oversized_chunks_allocated;
    assert_eq!(
        after - baseline,
        0,
        "min_payload == max_normal_alloc must stay on the normal cache path (shared)",
    );
}

// --- Vec::shrink_to_fit oversized-route bypass (`> with >=` at line 116) -----
//
// `if total_bytes > self.arena.max_normal_alloc() { return; }` — the
// mutant `>=` returns early when `total_bytes == max_normal_alloc`,
// skipping the bump-cursor reclaim. The visible `capacity()` getter
// therefore stays at `cap` instead of dropping to `len`.
//
// We use `cap == mna - 1` so that the Vec's `refill_hint` (which adds
// `align_of::<T>()` for cursor-alignment slack) stays `<= mna` and the
// Vec is allocated from the current normal chunk rather than a
// dedicated one-shot oversized chunk. `total_bytes == cap == mna - 1`
// still distinguishes the original `>` from the mutated `>=`: original
// proceeds with `try_reclaim_tail`, mutated would early-return at the
// (slightly different) `mna - 1 >= mna` boundary.

#[test]
fn vec_shrink_to_fit_at_max_normal_alloc_boundary_reclaims() {
    let mna = 4 * 1024;
    let arena: Arena = Arena::builder().max_normal_alloc(mna).build();
    // u8 keeps `total_bytes == cap`. Pick the largest cap whose refill hint
    // still fits in a normal chunk (`refill_hint <= mna`), so the Vec lives in
    // `current` and `try_reclaim_tail` has a chance to fire. The freezable
    // buffer reserves the `Arc<[u8]>` freeze prefix, so the hint is
    // `cap + 16` (≈12B strong+len prefix + 4B alignment slack).
    let cap = mna - 16;
    let mut v: ArenaVec<'_, u8> = arena.alloc_vec_with_capacity(cap);
    v.extend_from_slice([7_u8; 16]);
    assert_eq!(v.capacity(), cap);
    v.shrink_to_fit();
    assert_eq!(
        v.capacity(),
        v.len(),
        "shrink_to_fit on a Vec backed by the current normal chunk must reclaim the unused tail",
    );
}
