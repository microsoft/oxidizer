// Copyright (c) Microsoft Corporation.

//! MockCache Testing Example
//!
//! Demonstrates how to use `MockCache` to test cache-dependent code:
//! - Using MockCache as a storage backend
//! - Recording and verifying cache operations
//! - Injecting failures to test error handling
//! - Pre-populating test data
//! - Sharing state between cache and test assertions

use cachelon::{Cache, CacheEntry};
use cachelon_tier::testing::{CacheOp, MockCache};
use tick::Clock;

#[tokio::main]
async fn main() {
    basic_mock_usage().await;
    operation_recording().await;
    failure_injection().await;
    prepopulated_data().await;
    shared_state_testing().await;
}

/// Basic example: Use MockCache as the storage backend for a Cache.
async fn basic_mock_usage() {
    let clock = Clock::new_tokio();

    // Create a MockCache and use it as storage
    let mock = MockCache::<String, i32>::new();
    let cache = Cache::builder(clock).storage(mock.clone()).build();

    // Use the cache normally
    cache.insert(&"user:1".to_string(), CacheEntry::new(42)).await;

    let _value = cache.get(&"user:1".to_string()).await;

    // The mock tracks all operations and entries
    let _operation_count = mock.operations().len();
    let _entry_count = mock.entry_count();
}

/// Verify that your code performs the expected cache operations.
async fn operation_recording() {
    let clock = Clock::new_tokio();
    let mock = MockCache::<String, String>::new();
    let cache = Cache::builder(clock).storage(mock.clone()).build();

    // Simulate some application logic that uses the cache
    let key = "session:abc123".to_string();

    // Check if session exists (miss)
    let _ = cache.get(&key).await;

    // Create new session
    cache.insert(&key, CacheEntry::new("user_data".to_string())).await;

    // Read session back
    let _ = cache.get(&key).await;

    // Verify the exact sequence of operations
    let ops = mock.operations();

    // Assert specific operations occurred in order
    assert!(matches!(&ops[0], CacheOp::Get(k) if k == "session:abc123"));
    assert!(matches!(&ops[1], CacheOp::Insert { key, .. } if key == "session:abc123"));
    assert!(matches!(&ops[2], CacheOp::Get(k) if k == "session:abc123"));
}

/// Inject failures to test error handling paths.
async fn failure_injection() {
    let clock = Clock::new_tokio();
    let mock = MockCache::<String, i32>::new();
    let cache = Cache::builder(clock).storage(mock.clone()).build();

    // Test 1: Fail all get operations
    mock.fail_when(|op| matches!(op, CacheOp::Get(_)));

    let result = cache.try_get(&"key".to_string()).await;
    assert!(result.is_err(), "Expected get to fail");

    // Note: Infallible methods still work (they just return None/succeed silently)
    let infallible_result = cache.get(&"key".to_string()).await;
    assert!(infallible_result.is_none());

    // Test 2: Clear failures and try again
    mock.clear_failures();
    let result = cache.try_get(&"key".to_string()).await;
    assert!(result.is_ok(), "Expected get to succeed after clearing failures");

    // Test 3: Fail only specific keys
    mock.fail_when(|op| matches!(op, CacheOp::Get(k) if k == "forbidden"));

    let allowed = cache.try_get(&"allowed".to_string()).await;
    let forbidden = cache.try_get(&"forbidden".to_string()).await;

    assert!(allowed.is_ok());
    assert!(forbidden.is_err());

    // Test 4: Fail based on value being inserted
    mock.clear_failures();
    mock.fail_when(|op| matches!(op, CacheOp::Insert { entry, .. } if *entry.value() < 0));

    let positive = cache.try_insert(&"pos".to_string(), CacheEntry::new(100)).await;
    let negative = cache.try_insert(&"neg".to_string(), CacheEntry::new(-1)).await;

    assert!(positive.is_ok());
    assert!(negative.is_err());
}

/// Pre-populate the mock with test data.
async fn prepopulated_data() {
    use std::collections::HashMap;

    let clock = Clock::new_tokio();

    // Create mock with existing data
    let mut initial_data = HashMap::new();
    initial_data.insert("config:timeout".to_string(), CacheEntry::new(30));
    initial_data.insert("config:retries".to_string(), CacheEntry::new(3));
    initial_data.insert("config:batch_size".to_string(), CacheEntry::new(100));

    let mock = MockCache::with_data(initial_data);
    let cache = Cache::builder(clock).storage(mock.clone()).build();

    // Data is immediately available
    let _timeout = cache.get(&"config:timeout".to_string()).await;
    let _retries = cache.get(&"config:retries".to_string()).await;

    let _initial_entry_count = mock.entry_count();
}

/// Demonstrate that cloned MockCache instances share state.
async fn shared_state_testing() {
    let clock = Clock::new_tokio();
    let mock = MockCache::<String, String>::new();

    // Clone the mock - both references share the same underlying data
    let mock_for_cache = mock.clone();
    let mock_for_assertions = mock.clone();

    let cache = Cache::builder(clock).storage(mock_for_cache).build();

    // Operations through the cache...
    cache.insert(&"key".to_string(), CacheEntry::new("value".to_string())).await;

    // ...are visible through our assertion handle
    assert!(mock_for_assertions.contains_key(&"key".to_string()));

    // Operations are recorded in shared state
    let _operation_count = mock_for_assertions.operations().len();

    // Failures set through one handle affect the other
    mock_for_assertions.fail_when(|_| true);
    let result = cache.try_get(&"key".to_string()).await;
    assert!(result.is_err());

    // Clear operations for clean assertions
    mock.clear_operations();
    assert_eq!(mock_for_assertions.operations().len(), 0);
}
