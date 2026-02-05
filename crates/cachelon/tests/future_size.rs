// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "memory")]

//! Tests to verify that cache future sizes remain bounded with nested fallback tiers.

use cachelon::{Cache, CacheEntry, LoadingCache};
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
    assert!(get_size < 6000, "Get future size is too large: {get_size}");
    assert!(insert_size < 3000, "Insert future size is too large: {insert_size}");
    assert!(invalidate_size < 6000, "Invalidate future size is too large: {invalidate_size}");
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

    // Verify that the future sizes are within reasonable bounds
    assert!(get_size < 1000, "Get future size is too large: {get_size}");
    assert!(insert_size < 1000, "Insert future size is too large: {insert_size}");
    assert!(invalidate_size < 1000, "Invalidate future size is too large: {invalidate_size}");
}

#[test]
fn loading_cache_future_size_bounded() {
    let clock = Clock::new_frozen();
    let key = "key".to_string();

    let cache = Cache::builder::<String, i32>(clock).memory().build();
    let loader = LoadingCache::new(cache);

    let get_or_insert_size = size_of_val(&loader.get_or_insert(&key, || async { 42 }));
    let try_get_or_insert_size =
        size_of_val(&loader.try_get_or_insert(&key, || async { Ok::<_, std::io::Error>(42) }));
    let optionally_get_or_insert_size =
        size_of_val(&loader.optionally_get_or_insert(&key, || async { Some(42) }));

    // Verify that the future sizes are within reasonable bounds
    assert!(get_or_insert_size < 1000, "get_or_insert future size is too large: {get_or_insert_size}");
    assert!(try_get_or_insert_size < 1000, "try_get_or_insert future size is too large: {try_get_or_insert_size}");
    assert!(optionally_get_or_insert_size < 1000, "optionally_get_or_insert future size is too large: {optionally_get_or_insert_size}");
}
