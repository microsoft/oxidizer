// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Entry expiration through public APIs.

#![cfg(test)]

testing_aids::init_tracing!();

mod wrapper {
    fn make_cache(
        name: &'static str,
        inner: MockCache<String, i32>,
        clock: Clock,
        ttl: Option<Duration>,
        policy: InsertPolicy<i32>,
    ) -> Cache<String, i32> {
        let builder = Cache::builder(clock).storage(inner).name(name).insert_policy(policy);
        if let Some(ttl) = ttl {
            builder.ttl(ttl).build()
        } else {
            builder.build()
        }
    }

    fn read_entry(storage: &MockCache<String, i32>, cache: &Cache<String, i32>, entry: Option<CacheEntry<i32>>) -> Option<CacheEntry<i32>> {
        futures::executor::block_on(async {
            if let Some(entry) = entry {
                storage.insert("key".into(), entry).await.unwrap();
            }
            cache.get("key").await.unwrap()
        })
    }

    use std::time::Duration;

    use cachet::{Cache, CacheEntry, CacheTier, InsertPolicy};
    use cachet_tier::MockCache;
    use tick::Clock;

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn insert_preserves_per_entry_ttl_over_tier_ttl() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let inner_check = inner.clone();
        let tier_ttl = Duration::from_mins(1);
        let entry_ttl = Duration::from_secs(30);
        let wrapper = make_cache("test", inner, clock.clone(), Some(tier_ttl), InsertPolicy::default());

        let entry = CacheEntry::expires_at(42, entry_ttl, clock.system_time());

        wrapper.insert("key".to_string(), entry).await.unwrap();

        let stored = inner_check.get(&"key".to_string()).await.unwrap().unwrap();
        assert_eq!(stored.ttl(), Some(entry_ttl));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn insert_without_tier_ttl_leaves_entry_ttl_unset() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let inner_check = inner.clone();
        let wrapper = make_cache("test", inner, clock, None, InsertPolicy::default());

        let entry = CacheEntry::new(42);
        wrapper.insert("key".to_string(), entry).await.unwrap();

        let stored = inner_check.get(&"key".to_string()).await.unwrap().unwrap();
        assert!(stored.ttl().is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn insert_with_tier_ttl_leaves_entry_ttl_unset() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let inner_check = inner.clone();
        let tier_ttl = Duration::from_mins(1);
        let wrapper = make_cache("test", inner, clock, Some(tier_ttl), InsertPolicy::default());

        let entry = CacheEntry::new(42);
        wrapper.insert("key".to_string(), entry).await.unwrap();

        let stored = inner_check.get(&"key".to_string()).await.unwrap().unwrap();
        assert!(stored.ttl().is_none());
    }

    #[test]
    fn mock_wrapper_new_and_accessors() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let wrapper = make_cache("mock_test", inner, clock, None, InsertPolicy::default());
        assert_eq!(wrapper.name(), "mock_test");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn mock_wrapper_get_insert_invalidate_clear() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let wrapper = make_cache("test", inner, clock, None, InsertPolicy::default());

        // get miss
        assert!(wrapper.get(&"key".to_string()).await.unwrap().is_none());

        // insert + get hit
        wrapper.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        let entry = wrapper.get(&"key".to_string()).await.unwrap().unwrap();
        assert_eq!(*entry.value(), 42);

        // invalidate
        wrapper.invalidate(&"key".to_string()).await.unwrap();
        assert!(wrapper.get(&"key".to_string()).await.unwrap().is_none());

        // insert + clear
        wrapper.insert("a".to_string(), CacheEntry::new(1)).await.unwrap();
        wrapper.clear().await.unwrap();
        assert!(wrapper.get(&"a".to_string()).await.unwrap().is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn mock_wrapper_len() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let wrapper = make_cache("test", inner, clock, None, InsertPolicy::default());
        assert_eq!(wrapper.len().await.unwrap(), 0);
        wrapper.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
        assert_eq!(wrapper.len().await.unwrap(), 1);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn mock_wrapper_get_error() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        inner.fail_when(|op| matches!(op, cachet_tier::CacheOp::Get(_)));
        let wrapper = make_cache("test", inner, clock, None, InsertPolicy::default());
        let result = wrapper.get(&"key".to_string()).await;
        result.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn mock_wrapper_insert_error() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        inner.fail_when(|op| matches!(op, cachet_tier::CacheOp::Insert { .. }));
        let wrapper = make_cache("test", inner, clock, None, InsertPolicy::default());
        let result = wrapper.insert("key".to_string(), CacheEntry::new(1)).await;
        result.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn mock_wrapper_invalidate_error() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        inner.fail_when(|op| matches!(op, cachet_tier::CacheOp::Invalidate(_)));
        let wrapper = make_cache("test", inner, clock, None, InsertPolicy::default());
        let result = wrapper.invalidate(&"key".to_string()).await;
        result.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn mock_wrapper_clear_error() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        inner.fail_when(|op| matches!(op, cachet_tier::CacheOp::Clear));
        let wrapper = make_cache("test", inner, clock, None, InsertPolicy::default());
        let result = wrapper.clear().await;
        result.unwrap_err();
    }

    #[test]
    fn wrapper_is_expired_with_no_ttl_returns_false() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let inner_check = inner.clone();
        let wrapper = make_cache("test", inner, clock, None, InsertPolicy::default());

        // Entry without TTL should not be expired
        let entry = CacheEntry::new(42);
        assert!(read_entry(&inner_check, &wrapper, Some(entry)).is_some());
    }

    #[test]
    fn wrapper_is_expired_with_ttl_without_cached_at_returns_true() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let inner_check = inner.clone();
        let wrapper = make_cache("test", inner, clock, Some(Duration::from_mins(1)), InsertPolicy::default());

        // Entry without cached_at should be expired if TTL is configured (treat as expired to be safe)
        let entry = CacheEntry::new(42);
        assert!(read_entry(&inner_check, &wrapper, Some(entry)).is_none());
    }

    #[test]
    fn wrapper_is_expired_when_system_time_goes_backward() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let inner_check = inner.clone();
        let wrapper = make_cache("test", inner, clock.clone(), Some(Duration::from_mins(1)), InsertPolicy::default());

        // Entry with cached_at in the future simulates clock going backward
        let entry = CacheEntry::expires_at(42, Duration::from_mins(1), clock.system_time() + Duration::from_hours(1));
        assert!(read_entry(&inner_check, &wrapper, Some(entry)).is_none());
    }

    #[test]
    fn wrapper_is_not_expired_when_elapsed_equals_ttl() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let inner_check = inner.clone();
        let ttl = Duration::from_mins(1);
        let wrapper = make_cache("test", inner, clock.clone(), Some(ttl), InsertPolicy::default());

        // Entry cached exactly TTL ago → elapsed == ttl → should NOT be expired (uses >)
        let entry = CacheEntry::expires_at(42, ttl, clock.system_time() - ttl);
        assert!(read_entry(&inner_check, &wrapper, Some(entry)).is_some());
    }

    #[test]
    fn mock_wrapper_handle_get_result_none() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let inner_check = inner.clone();
        let wrapper = make_cache("test", inner, clock, None, InsertPolicy::default());
        let result = read_entry(&inner_check, &wrapper, None);
        assert!(result.is_none());
    }

    #[test]
    fn mock_wrapper_handle_get_result_expired() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let inner_check = inner.clone();
        let wrapper = make_cache("test", inner, clock, Some(Duration::from_mins(1)), InsertPolicy::default());
        // Entry without cached_at → considered expired
        let entry = CacheEntry::new(42);
        let result = read_entry(&inner_check, &wrapper, Some(entry));
        assert!(result.is_none());
    }

    #[test]
    fn mock_wrapper_handle_get_result_valid() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let inner_check = inner.clone();
        let wrapper = make_cache("test", inner, clock, None, InsertPolicy::default());
        let entry = CacheEntry::new(42);
        let result = read_entry(&inner_check, &wrapper, Some(entry));
        assert!(result.is_some());
    }
}
