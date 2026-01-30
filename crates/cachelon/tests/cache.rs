// Copyright (c) Microsoft Corporation.

#![cfg(feature = "test-util")]

//! Integration tests for Cache API.

use cachelon::{Cache, CacheEntry, Error};
use tick::Clock;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[test]
fn builder_creates_cache() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, String>(clock).memory().build();

    assert!(!cache.name().is_empty());
}

#[test]
fn name_returns_non_empty_string() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, i32>(clock).memory().build();

    let name = cache.name();
    assert!(!name.is_empty());
}

#[test]
fn clock_returns_reference() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, i32>(clock).memory().build();

    let clock_ref = cache.clock();
    // Verify we can use the clock
    let _ = clock_ref.instant();
}

#[test]
fn inner_and_into_inner() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, i32>(clock).memory().build();

    let inner = cache.inner();
    assert!(!inner.name().is_empty());

    let owned_inner = cache.into_inner();
    assert!(!owned_inner.name().is_empty());
}

#[test]
fn get_insert_operations() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "test_key".to_string();

        assert!(cache.get(&key).await.is_none());

        cache.insert(&key, CacheEntry::new(42)).await;

        let entry = cache.get(&key).await;
        assert!(entry.is_some());
        assert_eq!(*entry.unwrap().value(), 42);
    });
}

#[test]
fn try_get_try_insert() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        let result = cache.try_get(&key).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());

        let result = cache.try_insert(&key, CacheEntry::new(100)).await;
        result.unwrap();

        let result = cache.try_get(&key).await;
        assert!(result.is_ok());
        assert_eq!(*result.unwrap().unwrap().value(), 100);
    });
}

#[test]
fn invalidate_removes_entry() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        cache.insert(&key, CacheEntry::new(42)).await;
        assert!(cache.get(&key).await.is_some());

        cache.invalidate(&key).await;
        assert!(cache.get(&key).await.is_none());
    });
}

#[test]
fn try_invalidate_returns_ok() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await;

        let result = cache.try_invalidate(&key).await;
        result.unwrap();
        assert!(cache.get(&key).await.is_none());
    });
}

#[test]
fn contains_checks_existence() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        assert!(!cache.contains(&key).await);

        cache.insert(&key, CacheEntry::new(42)).await;

        assert!(cache.contains(&key).await);
    });
}

#[test]
fn try_contains_returns_result() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        let result = cache.try_contains(&key).await;
        assert!(result.is_ok());
        assert!(!result.unwrap());

        cache.insert(&key, CacheEntry::new(42)).await;

        let result = cache.try_contains(&key).await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    });
}

#[test]
fn clear_removes_all_entries() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        cache.insert(&"k1".to_string(), CacheEntry::new(1)).await;
        cache.insert(&"k2".to_string(), CacheEntry::new(2)).await;

        // Verify entries exist before clearing
        assert!(cache.get(&"k1".to_string()).await.is_some());
        assert!(cache.get(&"k2".to_string()).await.is_some());

        cache.clear().await;

        assert!(cache.get(&"k1".to_string()).await.is_none());
        assert!(cache.get(&"k2".to_string()).await.is_none());
    });
}

#[test]
fn try_clear_returns_ok() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        cache.insert(&"k1".to_string(), CacheEntry::new(1)).await;

        let result = cache.try_clear().await;
        result.unwrap();

        assert!(cache.get(&"k1".to_string()).await.is_none());
    });
}

#[test]
fn len_and_is_empty() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        // Empty cache returns Some values
        assert_eq!(cache.len(), Some(0));
        assert_eq!(cache.is_empty(), Some(true));

        cache.insert(&"key".to_string(), CacheEntry::new(42)).await;

        // After insert, len() and is_empty() return Some values
        // Note: exact count may be eventually consistent with moka cache
        assert!(cache.len().is_some());
        assert!(cache.is_empty().is_some());
    });
}

#[test]
fn get_or_insert_returns_cached() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        let entry = cache.get_or_insert(&key, || async { 42 }).await;
        assert_eq!(*entry.value(), 42);

        let entry = cache.get_or_insert(&key, || async { 100 }).await;
        assert_eq!(*entry.value(), 42);
    });
}

#[test]
fn try_get_or_insert_success() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        let result: std::result::Result<CacheEntry<i32>, Error> = cache.try_get_or_insert(&key, || async { Ok(42) }).await;

        assert!(result.is_ok());
        assert_eq!(*result.unwrap().value(), 42);

        // Verify caching: second call should return cached value, not 100
        let result: std::result::Result<CacheEntry<i32>, Error> = cache.try_get_or_insert(&key, || async { Ok(100) }).await;

        assert!(result.is_ok());
        assert_eq!(*result.unwrap().value(), 42);
    });
}

#[test]
#[cfg(feature = "tokio")]
fn stampede_protection_returns_cached() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().stampede_protection().build();

        let key = "key".to_string();

        let result = cache.get(&key).await;
        assert!(result.is_none());

        cache.insert(&key, CacheEntry::new(42)).await;
        let result = cache.get(&key).await;
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
    });
}

// =============================================================================
// Thread Safety Tests (per O-ABSTRACTIONS-SEND-SYNC guideline)
// =============================================================================

/// Verifies that Cache with `InMemoryCache` storage is Send.
#[test]
fn cachelon_with_memory_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Cache<String, i32, cachelon_memory::InMemoryCache<String, i32>>>();
}

/// Verifies that Cache with `InMemoryCache` storage is Sync.
#[test]
fn cachelon_with_memory_is_sync() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<Cache<String, i32, cachelon_memory::InMemoryCache<String, i32>>>();
}

/// Verifies that `CacheEntry` is Send.
#[test]
fn cachelon_entry_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<CacheEntry<i32>>();
    assert_send::<CacheEntry<String>>();
}

/// Verifies that `CacheEntry` is Sync.
#[test]
fn cachelon_entry_is_sync() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<CacheEntry<i32>>();
    assert_sync::<CacheEntry<String>>();
}

/// Verifies that Error is Send.
#[test]
fn error_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Error>();
}

/// Verifies that Error is Sync.
#[test]
fn error_is_sync() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<Error>();
}
