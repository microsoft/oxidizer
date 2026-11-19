// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tier operations through public APIs.

#![cfg(test)]

mod tier {

    use std::time::{Duration, SystemTime};

    use cachet_memory::{InMemoryCache, InMemoryCacheBuilder};
    use cachet_tier::{CacheEntry, CacheTier};

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_max_capacity_sets_limit() {
        let expected_max_capacity = 100;
        let builder = InMemoryCacheBuilder::<String, i32>::new().max_capacity(expected_max_capacity);

        assert!(format!("{builder:?}").contains("max_capacity: Some(100)"));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_initial_capacity_sets_initial_capacity() {
        let expected_initial_capacity = 50;
        let builder = InMemoryCacheBuilder::<String, i32>::new().initial_capacity(expected_initial_capacity);

        assert!(format!("{builder:?}").contains("initial_capacity: Some(50)"));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_time_to_live_sets_ttl() {
        let expected_ttl = Duration::from_mins(5);
        let builder = InMemoryCacheBuilder::<String, i32>::new().time_to_live(expected_ttl);

        assert!(format!("{builder:?}").contains("time_to_live: Some(300s)"));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_time_to_idle_sets_tti() {
        let expected_tti = Duration::from_mins(1);
        let builder = InMemoryCacheBuilder::<String, i32>::new().time_to_idle(expected_tti);

        assert!(format!("{builder:?}").contains("time_to_idle: Some(60s)"));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_name_sets_cache_name() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().name("test-cache");

        assert!(format!("{builder:?}").contains("name: Some(\"test-cache\")"));
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[tokio::test]
    async fn insert_and_get_returns_value() {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

        let value = cache.get(&"key".to_string()).await.unwrap();
        assert_eq!(*value.unwrap().value(), 42);
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[tokio::test]
    async fn get_returns_none_after_per_entry_ttl() {
        let cache = InMemoryCache::<String, i32>::new();
        cache
            .insert("key".to_string(), CacheEntry::expires_at(42, Duration::ZERO, SystemTime::now()))
            .await
            .unwrap();

        let value = cache.get(&"key".to_string()).await.unwrap();
        assert!(value.is_none());
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[tokio::test]
    async fn update_with_per_entry_ttl_expires_entry() {
        let cache = InMemoryCache::<String, i32>::new();
        // Insert entry without per-entry TTL (will not expire on its own)
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

        // Re-insert same key with zero TTL (should expire immediately via expire_after_update)
        cache
            .insert("key".to_string(), CacheEntry::expires_at(99, Duration::ZERO, SystemTime::now()))
            .await
            .unwrap();

        let value = cache.get(&"key".to_string()).await.unwrap();
        assert!(value.is_none(), "Entry should expire after update with zero TTL");
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[tokio::test]
    async fn get_returns_none_after_cache_ttl() {
        let cache = InMemoryCache::<String, i32>::builder()
            .time_to_live(Duration::ZERO)
            .build()
            .unwrap();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

        let value = cache.get(&"key".to_string()).await.unwrap();
        assert!(value.is_none());
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[tokio::test]
    async fn get_returns_none_after_cache_tti() {
        let cache = InMemoryCache::<String, i32>::builder()
            .time_to_idle(Duration::ZERO)
            .build()
            .unwrap();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

        let value = cache.get(&"key".to_string()).await.unwrap();
        assert!(value.is_none());
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[tokio::test]
    async fn invalidate_removes_entry() {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

        cache.invalidate(&"key".to_string()).await.unwrap();

        let value = cache.get(&"key".to_string()).await.unwrap();
        assert!(value.is_none());
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[tokio::test]
    async fn clear_removes_all_entries() {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert("key1".to_string(), CacheEntry::new(42)).await.unwrap();
        cache.insert("key2".to_string(), CacheEntry::new(43)).await.unwrap();

        cache.clear().await.unwrap();

        let value1 = cache.get(&"key1".to_string()).await.unwrap();
        let value2 = cache.get(&"key2".to_string()).await.unwrap();
        assert!(value1.is_none());
        assert!(value2.is_none());
    }
}
