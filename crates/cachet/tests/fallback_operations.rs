// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fallback operations through public APIs.

#![cfg(test)]

testing_aids::init_tracing!();

mod support;

mod fallback {
    use std::time::Duration;

    use cachet::{Cache, CacheEntry, CacheTier, InsertPolicy};
    use cachet_tier::MockCache;
    use tick::Clock;

    fn make_fallback_cache() -> Cache<String, i32> {
        let clock = Clock::new_frozen();
        Cache::builder(clock.clone())
            .name("fallback")
            .storage(MockCache::new())
            .fallback(Cache::builder(clock).storage(MockCache::new()))
            .build()
    }

    fn policy_cache(policy: InsertPolicy<i32>) -> Cache<String, i32> {
        Cache::builder(Clock::new_frozen())
            .storage(MockCache::new())
            .insert_policy(policy)
            .build()
    }

    /// Tests that promotion from fallback to primary works correctly.
    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_cachet_promotes_from_fallback_to_primary() {
        let clock = Clock::new_frozen();

        let primary_storage = MockCache::<String, i32>::new();
        let primary_check = primary_storage.clone();
        let fallback_storage = MockCache::<String, i32>::new();

        fallback_storage.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .insert_policy(InsertPolicy::always())
            .fallback(fallback)
            .build();

        // Primary should be empty initially
        let primary_result = primary_check.get(&"key".to_string()).await.unwrap();
        assert!(primary_result.is_none());

        // Get should find in fallback and promote to primary
        let result = cache.get(&"key".to_string()).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);

        // Now primary should have the value (promoted from fallback)
        let primary_result = primary_check.get(&"key".to_string()).await.unwrap();
        assert!(primary_result.is_some());
    }

    /// Tests that Never promotion policy prevents promotion to primary.
    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_cachet_never_policy_does_not_promote() {
        let clock = Clock::new_frozen();

        let primary_storage = MockCache::<String, i32>::new();
        let primary_check = primary_storage.clone();
        let fallback_storage = MockCache::<String, i32>::new();

        fallback_storage.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .insert_policy(InsertPolicy::never())
            .fallback(fallback)
            .build();

        // Get should find in fallback but NOT promote
        let result = cache.get(&"key".to_string()).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);

        // Primary should still be empty (no promotion)
        let primary_result = primary_check.get(&"key".to_string()).await.unwrap();
        assert!(primary_result.is_none());
    }

    /// Tests that conditional promotion policy only promotes matching entries.
    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_cachet_when_policy_conditional_promotion() {
        fn is_positive(entry: &CacheEntry<i32>) -> bool {
            *entry.value() > 0
        }

        let clock = Clock::new_frozen();

        let primary_storage = MockCache::<String, i32>::new();
        let primary_check = primary_storage.clone();
        let fallback_storage = MockCache::<String, i32>::new();

        fallback_storage.insert("positive".to_string(), CacheEntry::new(42)).await.unwrap();
        fallback_storage.insert("negative".to_string(), CacheEntry::new(-10)).await.unwrap();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .insert_policy(InsertPolicy::when(is_positive))
            .fallback(fallback)
            .build();

        // Get positive value - should be promoted
        let result = cache.get(&"positive".to_string()).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);

        // Get negative value - should NOT be promoted
        let result = cache.get(&"negative".to_string()).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), -10);

        // Check primary has positive but not negative
        let positive = primary_check.get(&"positive".to_string()).await.unwrap();
        assert!(positive.is_some());
        let negative = primary_check.get(&"negative".to_string()).await.unwrap();
        assert!(negative.is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn policy_type_debug_formatting() {
        let always = InsertPolicy::<i32>::always();
        let never = InsertPolicy::<i32>::never();
        let when = InsertPolicy::<i32>::when(|_| true);

        let always_str = format!("{always:?}");
        let never_str = format!("{never:?}");
        let when_str = format!("{when:?}");

        assert!(always_str.contains("Always"), "got: {always_str}");
        assert!(never_str.contains("Never"), "got: {never_str}");
        assert!(when_str.contains("WhenBoxed"), "got: {when_str}");
    }

    #[test]
    fn insert_policy_always() {
        let cache = policy_cache(InsertPolicy::always());
        let entry = CacheEntry::new(42);
        assert!(matches!(
            futures::executor::block_on(cache.insert("key".into(), entry)).unwrap(),
            cachet::InsertOutcome::Accepted
        ));
    }

    #[test]
    fn insert_policy_never() {
        let cache = policy_cache(InsertPolicy::never());
        let entry = CacheEntry::new(42);
        assert!(!matches!(
            futures::executor::block_on(cache.insert("key".into(), entry)).unwrap(),
            cachet::InsertOutcome::Accepted
        ));
    }

    #[test]
    fn insert_policy_when() {
        let cache = policy_cache(InsertPolicy::when(|e: &CacheEntry<i32>| *e.value() > 10));
        assert!(matches!(
            futures::executor::block_on(cache.insert("key".into(), CacheEntry::new(42))).unwrap(),
            cachet::InsertOutcome::Accepted
        ));
        assert!(!matches!(
            futures::executor::block_on(cache.insert("key".into(), CacheEntry::new(5))).unwrap(),
            cachet::InsertOutcome::Accepted
        ));
    }

    #[test]
    fn fallback_cache_new_constructs() {
        let cache = make_fallback_cache();
        assert_eq!(cache.name(), "fallback");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_get_miss_both() {
        let cache = make_fallback_cache();
        let result = cache.get(&"key".to_string()).await.unwrap();
        assert!(result.is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_insert_writes_both() {
        let cache = make_fallback_cache();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        // Both tiers should have the value
        let entry = cache.get(&"key".to_string()).await.unwrap().unwrap();
        assert_eq!(*entry.value(), 42);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_invalidate() {
        let cache = make_fallback_cache();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        cache.invalidate(&"key".to_string()).await.unwrap();
        assert!(cache.get(&"key".to_string()).await.unwrap().is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_clear() {
        let cache = make_fallback_cache();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        cache.clear().await.unwrap();
        assert!(cache.get(&"key".to_string()).await.unwrap().is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_len() {
        let cache = make_fallback_cache();
        assert_eq!(cache.len().await.unwrap(), 0);
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        assert_eq!(cache.len().await.unwrap(), 1);
    }

    /// Exercises the background-refresh-on-get path: when a primary hit has a
    /// stale `cached_at`, `FallbackCache::get` should trigger `do_refresh`.
    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_get_triggers_background_refresh() {
        let clock = Clock::new_frozen();
        let primary_mock = MockCache::<String, i32>::new();

        // Insert an entry with an old cached_at so should_refresh returns true
        let old_time = clock.system_time() - Duration::from_mins(2);
        let entry = CacheEntry::expires_at(42, Duration::from_mins(5), old_time);
        primary_mock.insert("key".to_string(), entry).await.unwrap();

        let fallback_mock = MockCache::<String, i32>::new();
        let refresh = cachet::TimeToRefresh::new(Duration::from_secs(30), anyspawn::Spawner::new_tokio(None));
        let (sender, completed) = futures::channel::oneshot::channel();

        let fc = Cache::builder(clock.clone())
            .storage(primary_mock)
            .event_handler(crate::support::Completion(std::sync::Mutex::new(Some(sender))))
            .fallback(Cache::builder(clock).storage(fallback_mock))
            .time_to_refresh(refresh)
            .build();

        // Primary hit with stale cached_at should trigger background refresh
        let result = fc.get(&"key".to_string()).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);

        completed.await.unwrap();
    }
}
