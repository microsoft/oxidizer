// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache builders through public APIs.

#![cfg(test)]

testing_aids::init_tracing!();

mod builder_cache {

    use std::time::Duration;

    use cachet::{Cache, InsertPolicy};
    use tick::Clock;

    #[test]
    fn cache_builder_with_ttl() {
        let control = tick::ClockControl::new();
        let clock = control.to_clock();
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .ttl(Duration::from_mins(5))
            .build();

        futures::executor::block_on(async {
            cache.insert("key".to_string(), cachet_tier::CacheEntry::new(42)).await.unwrap();
            assert!(cache.get("key").await.unwrap().is_some(), "entry should exist before TTL");

            control.advance(Duration::from_secs(301));
            assert!(cache.get("key").await.unwrap().is_none(), "entry should expire after TTL");
        });
    }

    #[cfg(feature = "logs")]
    #[test]
    fn builder_enable_logs() {
        use testing_aids::tracing_logs::Capture;

        let capture = Capture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .enable_logs()
            .build();

        futures::executor::block_on(async {
            let _ = cache.get(&"key".to_string()).await;
        });

        // Logs should contain a cache event when logging is enabled
        capture.assert_contains(cachet::telemetry::attributes::EVENT_MISS);
    }

    #[test]
    fn mock_builder_new_and_storage() {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).storage(cachet_tier::MockCache::new()).build();
        assert!(!cache.name().is_empty());
    }

    #[test]
    fn mock_builder_name() {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .name("my_cache")
            .build();
        assert_eq!(cache.name(), "my_cache");
    }

    #[test]
    fn mock_builder_stampede_protection() {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .stampede_protection()
            .build();
        assert!(!cache.name().is_empty());
    }

    #[test]
    fn mock_builder_clock() {
        let clock = Clock::new_frozen();
        let builder = Cache::builder::<String, i32>(clock).storage(cachet_tier::MockCache::new());
        let _ = builder.clock();
    }

    #[test]
    fn mock_builder_fallback_and_build() {
        let clock = Clock::new_frozen();
        let fb = Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new());
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .fallback(fb)
            .build();
        assert!(!cache.name().is_empty());
    }

    #[test]
    fn mock_builder_fallback_insert_policy() {
        let clock = Clock::new_frozen();
        let fb = Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new());
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .insert_policy(InsertPolicy::never())
            .fallback(fb)
            .build();
        assert!(!cache.name().is_empty());
    }

    #[test]
    fn mock_builder_fallback_time_to_refresh() {
        let clock = Clock::new_frozen();
        let fb = Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new());
        let refresh = cachet::TimeToRefresh::new(Duration::from_secs(30), anyspawn::Spawner::new_tokio(None));
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .fallback(fb)
            .time_to_refresh(refresh)
            .build();
        assert!(!cache.name().is_empty());
    }

    #[test]
    fn mock_builder_fallback_stampede_protection() {
        let clock = Clock::new_frozen();
        let fb = Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new());
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .fallback(fb)
            .stampede_protection()
            .build();
        assert!(!cache.name().is_empty());
    }

    #[test]
    fn mock_builder_nested_fallback() {
        let clock = Clock::new_frozen();
        let l3 = Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new());
        let l2 = Cache::builder::<String, i32>(clock.clone())
            .storage(cachet_tier::MockCache::new())
            .fallback(l3);
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .fallback(l2)
            .build();
        assert!(!cache.name().is_empty());
    }

    /// Tests `FallbackBuilder::fallback()` - chaining `.fallback()` on a
    /// `FallbackBuilder` (not just `CacheBuilder`).
    #[test]
    fn fallback_builder_fallback() {
        let clock = Clock::new_frozen();
        let l3 = Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new());
        let l2 = Cache::builder::<String, i32>(clock.clone())
            .storage(cachet_tier::MockCache::new())
            .fallback(l3);
        let l1 = l2.fallback(Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new()));
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .fallback(l1)
            .build();
        assert!(!cache.name().is_empty());
    }
}
