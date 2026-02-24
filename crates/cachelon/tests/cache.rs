// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "memory")]

//! Integration tests for Cache API.

use cachelon::{Cache, CacheEntry, Error};
use tick::Clock;

type TestResult = Result<(), Error>;

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
fn into_dynamic_preserves_functionality() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        // Insert before converting
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await?;

        let dynamic = cache.into_dynamic();

        // Verify existing data is accessible
        let entry = dynamic.get(&"key".to_string()).await?.expect("entry should exist");
        assert_eq!(*entry.value(), 42);

        // Verify we can still insert and retrieve
        dynamic.insert(&"new".to_string(), CacheEntry::new(100)).await?;
        assert_eq!(*dynamic.get(&"new".to_string()).await?.unwrap().value(), 100);

        // Verify get_or_insert works (requires Cache wrapper functionality)
        let entry = dynamic.get_or_insert(&"computed".to_string(), || async { 200 }).await?;
        assert_eq!(*entry.value(), 200);

        Ok(())
    })
}

#[test]
fn get_insert_operations() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "test_key".to_string();

        assert!(cache.get(&key).await?.is_none());

        cache.insert(&key, CacheEntry::new(42)).await?;

        let entry = cache.get(&key).await?.expect("entry should exist");
        assert_eq!(*entry.value(), 42);
        Ok(())
    })
}

#[test]
fn invalidate_removes_entry() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        cache.insert(&key, CacheEntry::new(42)).await?;
        assert!(cache.get(&key).await?.is_some());

        cache.invalidate(&key).await?;
        assert!(cache.get(&key).await?.is_none());
        Ok(())
    })
}

#[test]
fn contains_checks_existence() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        assert!(!cache.contains(&key).await?);

        cache.insert(&key, CacheEntry::new(42)).await?;

        assert!(cache.contains(&key).await?);
        Ok(())
    })
}

#[test]
fn clear_removes_all_entries() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        cache.insert(&"k1".to_string(), CacheEntry::new(1)).await?;
        cache.insert(&"k2".to_string(), CacheEntry::new(2)).await?;

        // Verify entries exist before clearing
        assert!(cache.get(&"k1".to_string()).await?.is_some());
        assert!(cache.get(&"k2".to_string()).await?.is_some());

        cache.clear().await?;

        assert!(cache.get(&"k1".to_string()).await?.is_none());
        assert!(cache.get(&"k2".to_string()).await?.is_none());
        Ok(())
    })
}

#[test]
fn len_returns_some() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        // Empty cache returns Some(0)
        assert_eq!(cache.len(), Some(0));

        cache.insert(&"key".to_string(), CacheEntry::new(42)).await?;

        // After insert, len() returns Some value
        // Note: exact count may be eventually consistent with moka cache
        assert!(cache.len().is_some());
        Ok(())
    })
}

#[test]
fn get_or_insert_returns_cached() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        let entry = cache.get_or_insert(&key, || async { 42 }).await?;
        assert_eq!(*entry.value(), 42);

        let entry = cache.get_or_insert(&key, || async { 100 }).await?;
        assert_eq!(*entry.value(), 42);
        Ok(())
    })
}

#[test]
fn try_get_or_insert_success() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        let entry = cache.try_get_or_insert(&key, || async { Ok::<_, Error>(42) }).await?;
        assert_eq!(*entry.value(), 42);

        // Verify caching: second call should return cached value, not 100
        let entry = cache.try_get_or_insert(&key, || async { Ok::<_, Error>(100) }).await?;
        assert_eq!(*entry.value(), 42);
        Ok(())
    })
}

#[test]
fn try_get_or_insert_error() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        let result: Result<CacheEntry<i32>, Error> = cache
            .try_get_or_insert(&key, || async { Err(Error::from_message("test error")) })
            .await;

        result.expect_err("factory error should propagate");
    });
}

#[test]
fn stampede_protection_returns_cached() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().stampede_protection().build();

        let key = "key".to_string();

        let result = cache.get(&key).await?;
        assert!(result.is_none());

        cache.insert(&key, CacheEntry::new(42)).await?;
        let entry = cache.get(&key).await?.expect("entry should exist");
        assert_eq!(*entry.value(), 42);
        Ok(())
    })
}

// =============================================================================
// Thread Safety Tests (per O-ABSTRACTIONS-SEND-SYNC guideline)
// =============================================================================

/// Verifies that Cache with `InMemoryCache` storage is Send.
#[test]
fn cache_with_memory_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Cache<String, i32, cachelon_memory::InMemoryCache<String, i32>>>();
}

/// Verifies that Cache with `InMemoryCache` storage is Sync.
#[test]
fn cache_with_memory_is_sync() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<Cache<String, i32, cachelon_memory::InMemoryCache<String, i32>>>();
}

/// Verifies that `CacheEntry` is Send.
#[test]
fn cache_entry_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<CacheEntry<i32>>();
    assert_send::<CacheEntry<String>>();
}

/// Verifies that `CacheEntry` is Sync.
#[test]
fn cache_entry_is_sync() {
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

/// Verifies that with stampede protection, storage errors are propagated (not hidden).
#[test]
fn stampede_protection_propagates_storage_errors() {
    use cachelon_tier::testing::{CacheOp, MockCache};

    block_on(async {
        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        mock.fail_when(|op| matches!(op, CacheOp::Get(_)));

        let cache = Cache::builder(clock).storage(mock).stampede_protection().build();

        let result: Result<Option<CacheEntry<i32>>, Error> = cache.get(&"key".to_string()).await;
        assert!(result.is_err(), "storage error should propagate through stampede protection");
    });
}

/// Verifies that with stampede protection, panics are converted to errors (not hidden as misses).
#[test]
fn stampede_protection_converts_panic_to_error() {
    use cachelon::CacheTier;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use uniflight::LeaderPanicked;

    /// A cache tier that panics on the first get.
    #[derive(Clone)]
    struct PanickingCache {
        panicked: Arc<AtomicBool>,
    }

    impl CacheTier<String, i32> for PanickingCache {
        async fn get(&self, _key: &String) -> Result<Option<CacheEntry<i32>>, Error> {
            assert!(self.panicked.swap(true, Ordering::SeqCst), "simulated panic in cache tier");
            Ok(None)
        }

        async fn insert(&self, _key: &String, _entry: CacheEntry<i32>) -> Result<(), Error> {
            Ok(())
        }

        async fn invalidate(&self, _key: &String) -> Result<(), Error> {
            Ok(())
        }

        async fn clear(&self) -> Result<(), Error> {
            Ok(())
        }
    }

    block_on(async {
        let clock = Clock::new_frozen();
        let storage = PanickingCache {
            panicked: Arc::new(AtomicBool::new(false)),
        };
        let cache = Cache::builder(clock).storage(storage).stampede_protection().build();

        let result = cache.get(&"key".to_string()).await;

        // Should be an error, not Ok(None)
        let err = result.expect_err("panic should be converted to error, not hidden as cache miss");

        // The error should wrap a LeaderPanicked error
        assert!(err.is_source::<LeaderPanicked>(), "error should wrap LeaderPanicked, got: {err}");

        // The panic message should be extractable
        let panicked = err.source_as::<LeaderPanicked>().expect("should extract LeaderPanicked");
        assert!(
            panicked.message().contains("simulated panic"),
            "panic message should be preserved: {}",
            panicked.message()
        );
    });
}
