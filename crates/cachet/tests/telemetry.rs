// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Telemetry through public APIs.

#![cfg(test)]

testing_aids::init_tracing!();

mod telemetry_attributes {
    use cachet::telemetry::attributes::*;

    #[test]
    fn field_constants_match_tracing_field_names() {
        // These constants must match the field names used in tracing macros in telemetry/cache.rs.
        assert_eq!(FIELD_NAME, "cache.name");
        assert_eq!(FIELD_EVENT, "cache.event");
        assert_eq!(FIELD_DURATION_NS, "cache.duration_ns");
        assert_eq!(FIELD_OPERATION, "cache.operation");
        assert_eq!(FIELD_COALESCED, "cache.coalesced");
    }

    #[test]
    fn event_constants_are_unique() {
        let events = [
            EVENT_HIT,
            EVENT_MISS,
            EVENT_EXPIRED,
            EVENT_GET_ERROR,
            EVENT_INSERTED,
            EVENT_INSERT_REJECTED,
            EVENT_INSERT_ERROR,
            EVENT_INVALIDATED,
            EVENT_INVALIDATE_ERROR,
            EVENT_CLEARED,
            EVENT_CLEAR_ERROR,
            EVENT_REFRESH_HIT,
            EVENT_REFRESH_MISS,
            EVENT_REFRESH_ERROR,
            EVENT_EVICTION,
        ];

        for (i, a) in events.iter().enumerate() {
            for b in &events[i + 1..] {
                assert_ne!(a, b, "duplicate event constant: {a}");
            }
        }
    }
}

mod events {
    use std::sync::{Arc, Mutex};
    #[cfg(feature = "logs")]
    use std::time::Duration;

    use cachet::{Cache, CacheEntry, CacheEventHandler, CacheOperationEvent, CacheTierEvent, TransformCodec, TransformEncoder};
    use cachet_tier::MockCache;
    use tick::Clock;

    #[derive(Clone, Default)]
    struct Roles(Arc<Mutex<Vec<(String, bool)>>>);
    impl CacheEventHandler for Roles {
        fn on_tier_event(&self, event: &CacheTierEvent<'_>) {
            if event.tier_name != "outer" {
                self.0.lock().unwrap().push((event.tier_name.to_owned(), event.fallback));
            }
        }
        fn on_operation_complete(&self, _: &CacheOperationEvent<'_>) {}
    }

    #[test]
    fn gamma_round2_nested_fallback_builders_preserve_the_inherited_primary_role() {
        futures::executor::block_on(async {
            for fallback in [false, true] {
                let clock = Clock::new_frozen();
                let roles = Roles::default();
                let builder = Cache::builder::<i32, i32>(clock.clone())
                    .name("primary")
                    .event_handler(roles.clone())
                    .storage(MockCache::new())
                    .fallback(Cache::builder(clock.clone()).name("fallback").storage(MockCache::new()));
                let cache = if fallback {
                    Cache::builder::<i32, i32>(clock)
                        .name("outer")
                        .event_handler(roles.clone())
                        .storage(MockCache::new())
                        .fallback(builder)
                        .build()
                } else {
                    builder.build()
                };
                assert!(cache.get(&7).await.unwrap().is_none());
                assert_eq!(*roles.0.lock().unwrap(), [("primary".into(), fallback), ("fallback".into(), true)]);
            }
        });
    }

    #[tokio::test]
    async fn gamma_round2_transform_building_preserves_telemetry_and_each_tiers_role() {
        for root in [false, true] {
            let clock = Clock::new_frozen();
            let roles = Roles::default();
            let builder = Cache::builder::<i32, i32>(clock.clone())
                .name("pre")
                .event_handler(roles.clone())
                .storage(MockCache::new())
                .transform(
                    TransformEncoder::new(cachet::infallible(|key: &i32| *key)),
                    TransformCodec::new(
                        cachet::infallible(|value: &i32| *value),
                        cachet::infallible_owned(|value: i32| value),
                    ),
                )
                .fallback(Cache::builder(clock.clone()).name("post").storage(MockCache::new()));
            let cache = if root {
                builder.build()
            } else {
                Cache::builder::<i32, i32>(clock)
                    .name("outer")
                    .event_handler(roles.clone())
                    .storage(MockCache::new())
                    .fallback(builder)
                    .build()
            };
            assert!(cache.get(&7).await.unwrap().is_none());
            assert_eq!(*roles.0.lock().unwrap(), [("pre".to_owned(), !root), ("post".to_owned(), true)]);
        }
    }

    #[cfg(feature = "logs")]
    #[test]
    fn logs_emit_at_correct_severity_levels() {
        use cachet::CacheTier;
        use testing_aids::tracing_logs::Capture;
        futures::executor::block_on(async {
            let storage = MockCache::<String, i32>::new();
            let cache = Cache::builder(Clock::new_frozen())
                .storage(storage.clone())
                .name("cache")
                .enable_logs()
                .ttl(Duration::ZERO)
                .build();
            storage.fail_when(|_| true);
            {
                let capture = Capture::new();
                let _guard = tracing::subscriber::set_default(capture.subscriber());
                cache.get("key").await.unwrap_err();
                capture.assert_contains("ERROR");
            }
            storage.clear_failures();
            storage.insert("key".into(), CacheEntry::new(42)).await.unwrap();
            {
                let capture = Capture::new();
                let _guard = tracing::subscriber::set_default(capture.subscriber());
                assert!(cache.get("key").await.unwrap().is_none());
                capture.assert_contains("INFO");
            }
            cache.insert("key".into(), CacheEntry::new(42)).await.unwrap();
            let capture = Capture::new();
            let _guard = tracing::subscriber::set_default(capture.subscriber());
            assert!(cache.get("key").await.unwrap().is_some());
            let output = capture.output();
            let hit = output.lines().find(|line| line.contains("cache.hit")).unwrap();
            assert_eq!(hit.split_whitespace().next(), Some("DEBUG"), "{output}");
        });
    }

    #[test]
    fn telemetry_disabled_emits_nothing() {
        let storage = MockCache::<String, i32>::new();
        let cache = Cache::builder(Clock::new_frozen()).storage(storage).build();
        futures::executor::block_on(cache.insert("key".into(), CacheEntry::new(42))).unwrap();
        let capture = testing_aids::tracing_logs::Capture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());
        assert!(futures::executor::block_on(cache.get("key")).unwrap().is_some());
        assert!(capture.output().is_empty());
    }

    #[test]
    fn next_request_id_returns_unique_incrementing_values() {
        struct Ids(Arc<Mutex<Vec<u64>>>);
        impl CacheEventHandler for Ids {
            fn on_tier_event(&self, _: &CacheTierEvent<'_>) {}
            fn on_operation_complete(&self, event: &CacheOperationEvent<'_>) {
                self.0.lock().unwrap().push(event.request_id);
            }
        }
        let ids = Arc::new(Mutex::new(Vec::new()));
        let cache = Cache::builder::<i32, i32>(Clock::new_frozen())
            .storage(MockCache::new())
            .event_handler(Ids(Arc::clone(&ids)))
            .build();
        for _ in 0..3 {
            assert!(futures::executor::block_on(cache.get(&7)).unwrap().is_none());
        }
        let ids = ids.lock().unwrap();
        assert!(ids[1] > ids[0]);
        assert!(ids[2] > ids[1]);
    }
}
