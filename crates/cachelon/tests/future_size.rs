// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "memory")]

//! Tests to verify that cache future sizes remain bounded with nested fallback tiers.

use cachelon::{Cache, CacheEntry};
use tick::Clock;

#[test]
fn single_level_future_size_bounded() {
    let clock = Clock::new_frozen();
    let key = "key".to_string();

    let cache = Cache::builder::<String, i32>(clock).memory().build();

    let get_size = size_of_val(&cache.get(&key));
    let insert_size = size_of_val(&cache.insert(&key, CacheEntry::new(42)));
    let invalidate_size = size_of_val(&cache.invalidate(&key));

    // Verify that the future sizes are within reasonable bounds
    assert!(get_size < 1000, "Get future size is too large: {get_size}");
    assert!(insert_size < 1500, "Insert future size is too large: {insert_size}");
    assert!(invalidate_size < 1500, "Invalidate future size is too large: {invalidate_size}");
}

#[test]
fn future_size_bounded_with_nesting() {
    let clock = Clock::new_frozen();
    let key = "key".to_string();

    let l3_cache = Cache::builder::<String, i32>(clock.clone()).memory();
    let l2_cache = Cache::builder::<String, i32>(clock.clone()).memory().fallback(l3_cache);
    let cache = Cache::builder::<String, i32>(clock).memory().fallback(l2_cache).build();

    let get_size = size_of_val(&cache.get(&key));
    let insert_size = size_of_val(&cache.insert(&key, CacheEntry::new(42)));
    let invalidate_size = size_of_val(&cache.invalidate(&key));

    // Current sizes: get=904, insert=192, invalidate=224
    // get is larger because primary lookup is not boxed (to avoid allocation on hits)
    assert!(get_size < 1000, "Get future size is too large: {get_size}");
    assert!(insert_size < 500, "Insert future size is too large: {insert_size}");
    assert!(invalidate_size < 500, "Invalidate future size is too large: {invalidate_size}");
}

#[test]
fn loading_methods_future_size() {
    let clock = Clock::new_frozen();
    let key = "key".to_string();

    let cache = Cache::builder::<String, i32>(clock).memory().stampede_protection().build();

    let get_or_insert_size = size_of_val(&cache.get_or_insert(&key, || async { 42 }));
    let try_get_or_insert_size = size_of_val(&cache.try_get_or_insert(&key, || async { Ok::<_, std::io::Error>(42) }));
    let optionally_get_or_insert_size = size_of_val(&cache.optionally_get_or_insert(&key, || async { Some(42) }));

    // Verify that the future sizes are within reasonable bounds
    // Sizes vary across architectures (e.g. aarch64 vs x86_64), so the limits include some headroom.
    assert!(
        get_or_insert_size < 1600,
        "get_or_insert future size is too large: {get_or_insert_size}"
    );
    assert!(
        try_get_or_insert_size < 1600,
        "try_get_or_insert future size is too large: {try_get_or_insert_size}"
    );
    assert!(
        optionally_get_or_insert_size < 1600,
        "optionally_get_or_insert future size is too large: {optionally_get_or_insert_size}"
    );
}
