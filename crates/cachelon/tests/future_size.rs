// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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
    assert!(get_size < 6000, "Get future size is too large: {get_size}");
    assert!(insert_size < 6000, "Insert future size is too large: {insert_size}");
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
    assert!(get_size < 6000, "Get future size is too large: {get_size}");
    assert!(insert_size < 6000, "Insert future size is too large: {insert_size}");
    assert!(invalidate_size < 6000, "Invalidate future size is too large: {invalidate_size}");
}
