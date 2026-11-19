// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Memory cache configuration and operations through public APIs.

mod builder {
    use std::time::Duration;

    use cachet_memory::InMemoryCacheBuilder;
    use cachet_memory::policy::EvictionPolicy;

    #[test]
    fn max_capacity_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().max_capacity(100);
        assert!(format!("{builder:?}").contains("max_capacity: Some(100)"));
    }

    #[test]
    fn initial_capacity_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().initial_capacity(50);
        assert!(format!("{builder:?}").contains("initial_capacity: Some(50)"));
    }

    #[test]
    fn time_to_live_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().time_to_live(Duration::from_mins(5));
        assert!(format!("{builder:?}").contains("time_to_live: Some(300s)"));
    }

    #[test]
    fn time_to_idle_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().time_to_idle(Duration::from_mins(1));
        assert!(format!("{builder:?}").contains("time_to_idle: Some(60s)"));
    }

    #[test]
    fn name_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().name("test");
        assert!(format!("{builder:?}").contains("name: Some(\"test\")"));
    }

    #[test]
    fn eviction_telemetry_defaults_false() {
        let builder = InMemoryCacheBuilder::<String, i32>::new();
        assert!(!builder.eviction_telemetry_enabled());
    }

    #[test]
    fn with_eviction_telemetry_sets_flag() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().with_eviction_telemetry();
        assert!(builder.eviction_telemetry_enabled());
    }

    #[test]
    fn with_hasher_preserves_eviction_telemetry_flag() {
        let builder = InMemoryCacheBuilder::<String, i32>::new()
            .with_eviction_telemetry()
            .with_hasher(std::collections::hash_map::RandomState::new());
        assert!(builder.eviction_telemetry_enabled());
    }

    #[test]
    fn debug_impl_renders_all_fields() {
        let builder = InMemoryCacheBuilder::<String, i32>::new()
            .max_capacity(100)
            .initial_capacity(10)
            .time_to_live(Duration::from_mins(1))
            .time_to_idle(Duration::from_secs(30))
            .name("my_cache")
            .with_eviction_telemetry()
            .with_removal_observer(|_| {})
            .on_eviction(|_, _, _| {});
        let rendered = format!("{builder:?}");
        assert!(rendered.contains("InMemoryCacheBuilder"));
        assert!(rendered.contains("max_capacity: Some(100)"));
        assert!(rendered.contains("initial_capacity: Some(10)"));
        assert!(rendered.contains("time_to_live: Some(60s)"));
        assert!(rendered.contains("time_to_idle: Some(30s)"));
        assert!(rendered.contains("name: Some(\"my_cache\")"));
        assert!(rendered.contains("eviction_telemetry: true"));
        assert!(rendered.contains("removal_observers: 1"));
        assert!(rendered.contains("eviction_listener: Some(\"<set>\")"));
    }

    #[test]
    fn build_max_capacity_lt_initial_capacity_returns_validation_error() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .max_capacity(100)
            .initial_capacity(101)
            .build();
        ohno::assert_error_message!(
            result.unwrap_err(),
            "invalid cache configuration: initial_capacity (101) exceeds max_capacity (100)"
        );
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[test]
    fn build_max_capacity_eq_initial_capacity_succeeds() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .max_capacity(100)
            .initial_capacity(100)
            .build();
        result.unwrap();
    }

    #[test]
    fn build_ttl_less_than_tti_returns_validation_error() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .time_to_live(Duration::from_mins(1))
            .time_to_idle(Duration::from_mins(2))
            .build();
        ohno::assert_error_message!(
            result.unwrap_err(),
            "invalid cache configuration: time to idle (120s) exceeds time to live (60s)."
        );
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[test]
    fn build_ttl_eq_tti_succeeds() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .time_to_live(Duration::from_mins(1))
            .time_to_idle(Duration::from_mins(1))
            .build();
        result.unwrap();
    }

    #[test]
    fn build_eviction_policy_stores_value() {
        let policy = EvictionPolicy::lru();
        let builder = InMemoryCacheBuilder::<String, i32>::new().eviction_policy(policy.clone());
        assert!(format!("{builder:?}").contains(&format!("eviction_policy: {policy:?}")));
    }
}

mod policy {
    use cachet_memory::policy::EvictionPolicy;

    #[test]
    fn tiny_lfu_policy_has_correct_inner() {
        let policy = EvictionPolicy::tiny_lfu();
        assert_eq!(format!("{policy:?}"), "EvictionPolicy::TinyLfu");
    }

    #[test]
    fn lru_policy_has_correct_inner() {
        let policy = EvictionPolicy::lru();
        assert_eq!(format!("{policy:?}"), "EvictionPolicy::Lru");
    }

    #[test]
    fn default_policy_is_tiny_lfu() {
        let policy = EvictionPolicy::default();
        assert_eq!(policy, EvictionPolicy::tiny_lfu());
    }
}

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
