// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for Cache API.

#![cfg(feature = "memory")]

use cachet::{Cache, CacheEntry, Error};
use cachet_tier::MockCache;
use tick::Clock;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[cfg_attr(miri, ignore)]
#[test]
fn builder_creates_cache() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, String>(clock).memory().build();

    assert!(!cache.name().is_empty());
}

#[cfg_attr(miri, ignore)]
#[test]
fn name_returns_non_empty_string() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, i32>(clock).memory().build();

    let name = cache.name();
    assert!(!name.is_empty());
}

#[cfg_attr(miri, ignore)]
#[test]
fn clock_returns_reference() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, i32>(clock).memory().build();

    let clock_ref = cache.clock();
    // Verify we can use the clock
    let _ = clock_ref.instant();
}

#[cfg_attr(miri, ignore)]
#[test]
fn get_insert_operations() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "test_key".to_string();

        assert!(cache.get(&key).await.unwrap().is_none());

        cache.insert(key.clone(), CacheEntry::new(42)).await.unwrap();

        let entry = cache.get(&key).await.unwrap().expect("entry should exist");
        assert_eq!(*entry.value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn invalidate_removes_entry() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        cache.insert(key.clone(), CacheEntry::new(42)).await.unwrap();
        assert!(cache.get(&key).await.unwrap().is_some());

        cache.invalidate(&key).await.unwrap();
        assert!(cache.get(&key).await.unwrap().is_none());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn contains_checks_existence() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        assert!(!cache.contains(&key).await.unwrap());

        cache.insert(key.clone(), CacheEntry::new(42)).await.unwrap();

        assert!(cache.contains(&key).await.unwrap());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn clear_removes_all_entries() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        cache.insert("k1".to_string(), CacheEntry::new(1)).await.unwrap();
        cache.insert("k2".to_string(), CacheEntry::new(2)).await.unwrap();

        // Verify entries exist before clearing
        assert!(cache.get(&"k1".to_string()).await.unwrap().is_some());
        assert!(cache.get(&"k2".to_string()).await.unwrap().is_some());

        cache.clear().await.unwrap();

        assert!(cache.get(&"k1".to_string()).await.unwrap().is_none());
        assert!(cache.get(&"k2".to_string()).await.unwrap().is_none());
    });
}

#[test]
fn len_returns_correct_count() {
    block_on(async {
        // Use MockCache (HashMap-backed) for immediate consistency of len()
        let clock = Clock::new_frozen();
        let cache = Cache::builder(clock).storage(MockCache::<String, i32>::new()).build();

        assert_eq!(cache.len(), Some(0));

        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

        assert_eq!(cache.len(), Some(1));
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn get_or_insert_returns_cached() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        let entry = cache.get_or_insert(&key, || async { 42 }).await.unwrap();
        assert_eq!(*entry.value(), 42);

        let entry = cache.get_or_insert(&key, || async { 100 }).await.unwrap();
        assert_eq!(*entry.value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn try_get_or_insert_success() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        let entry = cache.try_get_or_insert(&key, || async { Ok::<_, Error>(42) }).await.unwrap();
        assert_eq!(*entry.value(), 42);

        // Verify caching: second call should return cached value, not 100
        let entry = cache.try_get_or_insert(&key, || async { Ok::<_, Error>(100) }).await.unwrap();
        assert_eq!(*entry.value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
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

#[cfg_attr(miri, ignore)]
#[test]
fn stampede_protection_returns_cached() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().stampede_protection().build();

        let key = "key".to_string();

        let result = cache.get(&key).await.unwrap();
        assert!(result.is_none());

        cache.insert(key.clone(), CacheEntry::new(42)).await.unwrap();
        let entry = cache.get(&key).await.unwrap().expect("entry should exist");
        assert_eq!(*entry.value(), 42);
    });
}

#[test]
fn is_empty_returns_correct_value() {
    block_on(async {
        // Use MockCache for immediate consistency
        let clock = Clock::new_frozen();
        let cache = Cache::builder(clock).storage(MockCache::<String, i32>::new()).build();

        assert_eq!(cache.is_empty(), Some(true));

        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

        assert_eq!(cache.is_empty(), Some(false));
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn stampede_protection_invalidate() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().stampede_protection().build();

        let key = "key".to_string();
        cache.insert(key.clone(), CacheEntry::new(42)).await.unwrap();
        assert!(cache.get(&key).await.unwrap().is_some());

        cache.invalidate(&key).await.unwrap();
        assert!(cache.get(&key).await.unwrap().is_none());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn stampede_protection_get_or_insert() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().stampede_protection().build();

        let key = "key".to_string();

        let entry = cache.get_or_insert(&key, || async { 42 }).await.unwrap();
        assert_eq!(*entry.value(), 42);

        // Second call returns cached value
        let entry = cache.get_or_insert(&key, || async { 100 }).await.unwrap();
        assert_eq!(*entry.value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn stampede_protection_try_get_or_insert_success() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().stampede_protection().build();

        let key = "key".to_string();

        let entry = cache.try_get_or_insert(&key, || async { Ok::<_, Error>(42) }).await.unwrap();
        assert_eq!(*entry.value(), 42);

        // Cached on second call
        let entry = cache.try_get_or_insert(&key, || async { Ok::<_, Error>(100) }).await.unwrap();
        assert_eq!(*entry.value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn stampede_protection_try_get_or_insert_error() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().stampede_protection().build();

        let key = "key".to_string();

        let result: Result<CacheEntry<i32>, Error> = cache
            .try_get_or_insert(&key, || async { Err(Error::from_message("test error")) })
            .await;

        result.expect_err("factory error should propagate through stampede protection");
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn stampede_protection_optionally_get_or_insert_some() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().stampede_protection().build();

        let key = "key".to_string();

        let entry = cache.optionally_get_or_insert(&key, || async { Some(42) }).await.unwrap();
        assert_eq!(*entry.unwrap().value(), 42);

        // Cached on second call
        let entry = cache.optionally_get_or_insert(&key, || async { Some(100) }).await.unwrap();
        assert_eq!(*entry.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn stampede_protection_optionally_get_or_insert_none() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().stampede_protection().build();

        let key = "key".to_string();

        // None result is not cached
        let result = cache.optionally_get_or_insert(&key, || async { None::<i32> }).await.unwrap();
        assert!(result.is_none());

        // Not cached, so second call still invokes factory
        let result = cache.optionally_get_or_insert(&key, || async { Some(42) }).await.unwrap();
        assert_eq!(*result.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn optionally_get_or_insert_none_not_cached() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();

        // None result is not cached
        let result = cache.optionally_get_or_insert(&key, || async { None::<i32> }).await.unwrap();
        assert!(result.is_none());

        // Not cached, so second call still invokes factory
        let result = cache.optionally_get_or_insert(&key, || async { Some(42) }).await.unwrap();
        assert_eq!(*result.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn optionally_get_or_insert_hit_returns_cached() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();
        cache.insert(key.clone(), CacheEntry::new(99)).await.unwrap();

        // Should return cached value without calling factory
        let result = cache.optionally_get_or_insert(&key, || async { Some(42) }).await.unwrap();
        assert_eq!(*result.unwrap().value(), 99);
    });
}

// =============================================================================
// Thread Safety Tests (per O-ABSTRACTIONS-SEND-SYNC guideline)
// =============================================================================

/// Verifies that Cache with `InMemoryCache` storage is Send.
#[cfg_attr(miri, ignore)]
#[test]
fn cache_with_memory_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Cache<String, i32, cachet_memory::InMemoryCache<String, i32>>>();
}

/// Verifies that Cache with `InMemoryCache` storage is Sync.
#[cfg_attr(miri, ignore)]
#[test]
fn cache_with_memory_is_sync() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<Cache<String, i32, cachet_memory::InMemoryCache<String, i32>>>();
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
    use cachet_tier::{CacheOp, MockCache};

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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use cachet::CacheTier;
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

        async fn insert(&self, _key: String, _entry: CacheEntry<i32>) -> Result<(), Error> {
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

#[cfg_attr(miri, ignore)]
#[test]
fn stampede_protection_invalidate_removes_entry() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().stampede_protection().build();

        let key = "key".to_string();
        cache.insert(key.clone(), CacheEntry::new(42)).await.unwrap();
        assert!(cache.get(&key).await.unwrap().is_some());

        cache.invalidate(&key).await.unwrap();
        assert!(cache.get(&key).await.unwrap().is_none());
    });
}

#[test]
fn try_get_or_insert_with_storage_error_propagates() {
    block_on(async {
        use cachet_tier::{CacheOp, MockCache};

        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        // Fail on get so do_try_get_or_insert's inner get fails
        mock.fail_when(|op| matches!(op, CacheOp::Get(_)));
        let cache = Cache::builder(clock).storage(mock).build();

        let result: Result<CacheEntry<i32>, Error> = cache.try_get_or_insert("key", || async { Ok::<_, std::io::Error>(42) }).await;
        result.unwrap_err();
    });
}

#[test]
fn optionally_get_or_insert_with_storage_error_propagates() {
    block_on(async {
        use cachet_tier::{CacheOp, MockCache};

        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        mock.fail_when(|op| matches!(op, CacheOp::Get(_)));
        let cache = Cache::builder(clock).storage(mock).build();

        let result: Result<Option<CacheEntry<i32>>, Error> = cache.optionally_get_or_insert("key", || async { Some(42) }).await;
        result.unwrap_err();
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn cache_debug_output() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, i32>(clock).memory().build();
    let debug_str = format!("{cache:?}");
    assert!(debug_str.contains("Cache"), "got: {debug_str}");
}

#[cfg_attr(miri, ignore)]
#[test]
fn cache_debug_with_stampede_protection() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, i32>(clock).memory().stampede_protection().build();
    let debug_str = format!("{cache:?}");
    assert!(debug_str.contains("Mergers"), "got: {debug_str}");
}

// =============================================================================
// Borrow semantics tests
// =============================================================================

#[cfg_attr(miri, ignore)]
#[test]
fn borrow_get_insert_with_str_key() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        // Use &str keys with Cache<String, i32>
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        let entry = cache.get("key").await.unwrap().expect("entry should exist");
        assert_eq!(*entry.value(), 42);

        assert!(cache.get("missing").await.unwrap().is_none());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn borrow_invalidate_with_str_key() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        assert!(cache.contains("key").await.unwrap());

        cache.invalidate("key").await.unwrap();
        assert!(!cache.contains("key").await.unwrap());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn borrow_get_or_insert_with_str_key() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let entry = cache.get_or_insert("key", || async { 42 }).await.unwrap();
        assert_eq!(*entry.value(), 42);

        // Second call returns cached value
        let entry = cache.get_or_insert("key", || async { 100 }).await.unwrap();
        assert_eq!(*entry.value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn borrow_try_get_or_insert_with_str_key() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let entry = cache.try_get_or_insert("key", || async { Ok::<_, Error>(42) }).await.unwrap();
        assert_eq!(*entry.value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn borrow_optionally_get_or_insert_with_str_key() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let result = cache.optionally_get_or_insert("missing", || async { None::<i32> }).await.unwrap();
        assert!(result.is_none());

        let result = cache.optionally_get_or_insert("key", || async { Some(42) }).await.unwrap();
        assert_eq!(*result.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn borrow_stampede_protection_with_str_key() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().stampede_protection().build();

        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        let entry = cache.get("key").await.unwrap().expect("entry should exist");
        assert_eq!(*entry.value(), 42);

        cache.invalidate("key").await.unwrap();
        assert!(cache.get("key").await.unwrap().is_none());

        let entry = cache.get_or_insert("new", || async { 77 }).await.unwrap();
        assert_eq!(*entry.value(), 77);
    });
}

// =============================================================================
// Service feature tests
// =============================================================================

#[cfg(feature = "service")]
mod service_tests {
    use cachet::{CacheOperation, CacheResponse, GetRequest, InsertRequest, InvalidateRequest};
    use layered::Service;

    use super::*;

    /// Simple in-memory service implementing Service<CacheOperation>
    #[derive(Clone)]
    struct InMemoryService {
        data: std::sync::Arc<parking_lot::Mutex<std::collections::HashMap<String, CacheEntry<i32>>>>,
    }

    impl InMemoryService {
        fn new() -> Self {
            Self {
                data: std::sync::Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            }
        }
    }

    impl Service<CacheOperation<String, i32>> for InMemoryService {
        type Out = Result<CacheResponse<i32>, Error>;

        async fn execute(&self, input: CacheOperation<String, i32>) -> Self::Out {
            match input {
                CacheOperation::Get(req) => Ok(CacheResponse::Get(self.data.lock().get(&req.key).cloned())),
                CacheOperation::Insert(req) => {
                    self.data.lock().insert(req.key, req.entry);
                    Ok(CacheResponse::Insert)
                }
                CacheOperation::Invalidate(req) => {
                    self.data.lock().remove(&req.key);
                    Ok(CacheResponse::Invalidate)
                }
                CacheOperation::Clear => {
                    self.data.lock().clear();
                    Ok(CacheResponse::Clear)
                }
            }
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn cache_builder_service_creates_cache() {
        block_on(async {
            let clock = Clock::new_frozen();
            let cache = Cache::builder::<String, i32>(clock).service(InMemoryService::new()).build();
            assert!(!cache.name().is_empty());

            // Verify the cache works end-to-end through the service layer
            cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
            let entry = cache.get(&"key".to_string()).await.unwrap().expect("entry should exist");
            assert_eq!(*entry.value(), 42);
        });
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn cache_service_get() {
        block_on(async {
            let clock = Clock::new_frozen();
            let cache = Cache::builder::<String, i32>(clock).memory().build();
            cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

            let response = cache
                .execute(CacheOperation::Get(GetRequest::new("key".to_string())))
                .await
                .unwrap();
            match response {
                CacheResponse::Get(Some(entry)) => assert_eq!(*entry.value(), 42),
                other => panic!("expected Get(Some), got {other:?}"),
            }
        });
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn cache_service_get_miss() {
        block_on(async {
            let clock = Clock::new_frozen();
            let cache = Cache::builder::<String, i32>(clock).memory().build();

            let response = cache
                .execute(CacheOperation::Get(GetRequest::new("missing".to_string())))
                .await
                .unwrap();
            match response {
                CacheResponse::Get(None) => {}
                other => panic!("expected Get(None), got {other:?}"),
            }
        });
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn cache_service_insert() {
        block_on(async {
            let clock = Clock::new_frozen();
            let cache = Cache::builder::<String, i32>(clock).memory().build();

            let response = cache
                .execute(CacheOperation::Insert(InsertRequest::new("key".to_string(), CacheEntry::new(42))))
                .await
                .unwrap();
            assert!(matches!(response, CacheResponse::Insert));

            // Verify the value was inserted
            let entry = cache.get(&"key".to_string()).await.unwrap().unwrap();
            assert_eq!(*entry.value(), 42);
        });
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn cache_service_invalidate() {
        block_on(async {
            let clock = Clock::new_frozen();
            let cache = Cache::builder::<String, i32>(clock).memory().build();
            cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

            let response = cache
                .execute(CacheOperation::Invalidate(InvalidateRequest::new("key".to_string())))
                .await
                .unwrap();
            assert!(matches!(response, CacheResponse::Invalidate));

            assert!(cache.get(&"key".to_string()).await.unwrap().is_none());
        });
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn cache_service_clear() {
        block_on(async {
            let clock = Clock::new_frozen();
            let cache = Cache::builder::<String, i32>(clock).memory().build();
            cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

            let response = cache.execute(CacheOperation::Clear).await.unwrap();
            assert!(matches!(response, CacheResponse::Clear));

            assert!(cache.get(&"key".to_string()).await.unwrap().is_none());
        });
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn cache_builder_enable_metrics() {
        block_on(async {
            let tester = testing_aids::MetricTester::new();
            let clock = Clock::new_frozen();
            let cache = Cache::builder::<String, i32>(clock)
                .memory()
                .enable_metrics(tester.meter_provider())
                .build();

            let key = "key".to_string();
            cache.insert(key.clone(), CacheEntry::new(42)).await.unwrap();
            let entry = cache.get(&key).await.unwrap().expect("entry should exist");
            assert_eq!(*entry.value(), 42);
        });
    }
}
