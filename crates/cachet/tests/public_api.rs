// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache contracts through public builders, operations, and event handlers.

testing_aids::init_tracing!();
mod cache {

    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[cfg(feature = "seismograph")]
    use cachet::Error;
    use cachet::telemetry::handler::RequestId;
    use cachet::{Cache, CacheEntry, CacheEventHandler, CacheOperationEvent, CacheTier, CacheTierEvent};
    use cachet_tier::MockCache;
    use tick::Clock;

    #[test]
    fn gamma_round2_computed_entry_and_stored_candidate_share_the_same_original_timestamp() {
        let control = tick::ClockControl::new().auto_advance(Duration::from_millis(1));
        let storage = MockCache::<u32, u32>::new();
        let cache = Cache::builder(control.to_clock()).storage(storage.clone()).build();
        block_on(async {
            let returned = cache.get_or_insert_with(&7, || async { CacheEntry::new(37) }).await.unwrap();
            let stored = storage.get(&7).await.unwrap().unwrap();
            assert!(returned.cached_at().is_some());
            assert_eq!(stored.cached_at(), returned.cached_at());
            assert_eq!(*stored.value(), 37);
        });
    }

    #[test]
    #[serial_test::serial]
    #[cfg(feature = "seismograph")]
    fn gamma_round2_compute_paths_record_exact_success_failure_and_none_events() {
        use seismograph::recorder::event::EventKind;

        seismograph::recorder(seismograph::recorder::Configuration {
            cache: seismograph::recorder::Policy::all(false),
            event_capacity_per_thread: seismograph::recorder::EventBufferCapacity::new(1024),
            ..Default::default()
        });
        let count = |kind| {
            let thread = seismograph::recorder::thread_id();
            seismograph::snapshot::decode(
                seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default())
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap()
            .events
            .events
            .iter()
            .filter(|event| event.thread_id == thread && event.kind == kind)
            .count()
        };
        let succeeded = count(EventKind::CacheComputeSucceeded);
        let failed = count(EventKind::CacheComputeFailed);
        let none = count(EventKind::CacheComputeReturnedNone);
        let cache = Cache::builder::<u32, u32>(Clock::new_frozen()).storage(MockCache::new()).build();
        block_on(async {
            assert_eq!(*cache.get_or_insert(&1, || async { 7 }).await.unwrap().value(), 7);
            assert_eq!(count(EventKind::CacheComputeSucceeded), succeeded + 1);
            assert_eq!(
                *cache
                    .get_or_insert_with(&2, || async { CacheEntry::new(11) })
                    .await
                    .unwrap()
                    .value(),
                11
            );
            assert_eq!(count(EventKind::CacheComputeSucceeded), succeeded + 2);
            cache.try_get_or_insert(&3, || async { Ok::<_, Error>(13) }).await.unwrap();
            assert_eq!(count(EventKind::CacheComputeSucceeded), succeeded + 3);
            cache
                .try_get_or_insert_with(&4, || async { Ok::<_, Error>(CacheEntry::new(17)) })
                .await
                .unwrap();
            assert_eq!(count(EventKind::CacheComputeSucceeded), succeeded + 4);
            cache.optionally_get_or_insert(&5, || async { Some(19) }).await.unwrap();
            assert_eq!(count(EventKind::CacheComputeSucceeded), succeeded + 5);
            cache
                .try_get_or_insert(&6, || async { Err::<u32, _>(Error::from_message("round2 failure")) })
                .await
                .unwrap_err();
            assert_eq!(count(EventKind::CacheComputeFailed), failed + 1);
            cache
                .try_get_or_insert_with(&7, || async { Err::<CacheEntry<u32>, _>(Error::from_message("round2 failure")) })
                .await
                .unwrap_err();
            assert_eq!(count(EventKind::CacheComputeFailed), failed + 2);
            assert!(cache.optionally_get_or_insert(&8, || async { None }).await.unwrap().is_none());
            assert_eq!(count(EventKind::CacheComputeReturnedNone), none + 1);
        });
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures::executor::block_on(f)
    }

    fn build_cache() -> Cache<String, i32> {
        let clock = Clock::new_frozen();
        Cache::builder::<String, i32>(clock).storage(MockCache::new()).build()
    }

    fn build_cache_with_stampede() -> Cache<String, i32> {
        let clock = Clock::new_frozen();
        Cache::builder::<String, i32>(clock)
            .storage(MockCache::new())
            .stampede_protection()
            .build()
    }

    #[test]
    fn cache_builder_creates_cache() {
        let clock = Clock::new_frozen();
        let _ = Cache::builder::<String, i32>(clock);
    }

    #[test]
    fn cache_new_and_accessors() {
        let cache = build_cache();
        assert!(!cache.name().is_empty());
        let _ = cache.clock();
    }

    #[test]
    fn cache_get_miss() {
        block_on(async {
            let cache = build_cache();
            let result = cache.get("missing").await.unwrap();
            assert!(result.is_none());
        });
    }

    #[test]
    fn cache_event_handler_receives_fallback_tier_events() {
        type EventRecord = Vec<(RequestId, String, String, bool)>;

        #[derive(Clone)]
        struct RecordingHandler {
            tier_events: Arc<Mutex<EventRecord>>,
            operation_events: Arc<Mutex<EventRecord>>,
        }

        impl CacheEventHandler for RecordingHandler {
            fn on_tier_event(&self, event: &CacheTierEvent<'_>) {
                self.tier_events.lock().unwrap().push((
                    event.request_id,
                    event.tier_name.to_string(),
                    event.outcome.to_string(),
                    event.fallback,
                ));
            }

            fn on_operation_complete(&self, event: &CacheOperationEvent<'_>) {
                self.operation_events.lock().unwrap().push((
                    event.request_id,
                    event.cache_name.to_string(),
                    event.operation.to_string(),
                    event.coalesced,
                ));
            }
        }

        let tier_events = Arc::new(Mutex::new(Vec::new()));
        let operation_events = Arc::new(Mutex::new(Vec::new()));

        block_on(async {
            let clock = Clock::new_frozen();
            let handler = RecordingHandler {
                tier_events: Arc::clone(&tier_events),
                operation_events: Arc::clone(&operation_events),
            };

            let l2 = Cache::builder::<String, i32>(clock.clone()).storage(MockCache::new()).name("l2");
            let cache = Cache::builder::<String, i32>(clock)
                .storage(MockCache::new())
                .name("l1")
                .event_handler(handler)
                .fallback(l2)
                .build();

            let result = cache.get("missing").await.unwrap();
            assert!(result.is_none());
        });

        let tier_events = tier_events.lock().unwrap().clone();
        let operation_events = operation_events.lock().unwrap().clone();
        let request_id = operation_events[0].0;

        assert_eq!(
            tier_events,
            vec![
                (
                    request_id,
                    "l1".to_string(),
                    cachet::telemetry::attributes::EVENT_MISS.to_string(),
                    false
                ),
                (
                    request_id,
                    "l2".to_string(),
                    cachet::telemetry::attributes::EVENT_MISS.to_string(),
                    true
                ),
            ]
        );
        assert_eq!(
            operation_events,
            vec![(request_id, "l1".to_string(), "cache.get".to_string(), false)]
        );
    }

    #[test]
    fn cache_insert_and_get() {
        block_on(async {
            let cache = build_cache();
            cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
            let entry = cache.get("key").await.unwrap().unwrap();
            assert_eq!(*entry.value(), 42);
        });
    }

    #[test]
    fn cache_invalidate_no_stampede() {
        block_on(async {
            let cache = build_cache();
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            cache.invalidate("key").await.unwrap();
            assert!(cache.get("key").await.unwrap().is_none());
        });
    }

    #[test]
    fn cache_invalidate_with_stampede() {
        block_on(async {
            let cache = build_cache_with_stampede();
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            cache.invalidate("key").await.unwrap();
            assert!(cache.get("key").await.unwrap().is_none());
        });
    }

    #[test]
    fn cache_contains() {
        block_on(async {
            let cache = build_cache();
            assert!(!cache.contains("key").await.unwrap());
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            assert!(cache.contains("key").await.unwrap());
        });
    }

    #[test]
    fn cache_clear() {
        block_on(async {
            let cache = build_cache();
            cache.insert("a".to_string(), CacheEntry::new(1)).await.unwrap();
            cache.clear().await.unwrap();
            assert!(cache.get("a").await.unwrap().is_none());
        });
    }

    #[test]
    fn cache_len_and_is_empty() {
        block_on(async {
            let cache = build_cache();
            assert_eq!(cache.len().await.unwrap(), 0);
            assert!(cache.is_empty().await.unwrap());
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            assert_eq!(cache.len().await.unwrap(), 1);
            assert!(!cache.is_empty().await.unwrap());
        });
    }

    #[test]
    fn cache_get_with_stampede() {
        block_on(async {
            let cache = build_cache_with_stampede();
            cache.insert("key".to_string(), CacheEntry::new(99)).await.unwrap();
            let entry = cache.get("key").await.unwrap().unwrap();
            assert_eq!(*entry.value(), 99);
        });
    }

    #[test]
    fn cache_get_miss_with_stampede() {
        block_on(async {
            let cache = build_cache_with_stampede();
            assert!(cache.get("missing").await.unwrap().is_none());
        });
    }

    #[test]
    fn cache_get_or_insert_miss() {
        block_on(async {
            let cache = build_cache();
            let entry = cache.get_or_insert("key", || async { 42 }).await.unwrap();
            assert_eq!(*entry.value(), 42);
        });
    }

    #[test]
    fn cache_get_or_insert_hit() {
        block_on(async {
            let cache = build_cache();
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            let entry = cache.get_or_insert("key", || async { 99 }).await.unwrap();
            assert_eq!(*entry.value(), 1);
        });
    }

    #[test]
    fn cache_get_or_insert_with_stampede() {
        block_on(async {
            let cache = build_cache_with_stampede();
            let entry = cache.get_or_insert("key", || async { 42 }).await.unwrap();
            assert_eq!(*entry.value(), 42);
        });
    }

    #[test]
    fn cache_try_get_or_insert_ok() {
        block_on(async {
            let cache = build_cache();
            let entry = cache
                .try_get_or_insert("key", || async { Ok::<_, std::io::Error>(42) })
                .await
                .unwrap();
            assert_eq!(*entry.value(), 42);
        });
    }

    #[test]
    fn cache_try_get_or_insert_hit() {
        block_on(async {
            let cache = build_cache();
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            let entry = cache
                .try_get_or_insert("key", || async { Ok::<_, std::io::Error>(99) })
                .await
                .unwrap();
            assert_eq!(*entry.value(), 1);
        });
    }

    #[test]
    fn cache_try_get_or_insert_err() {
        block_on(async {
            let cache = build_cache();
            let result = cache
                .try_get_or_insert("key", || async { Err::<i32, _>(std::io::Error::other("fail")) })
                .await;
            result.unwrap_err();
        });
    }

    #[test]
    fn cache_try_get_or_insert_with_stampede() {
        block_on(async {
            let cache = build_cache_with_stampede();
            let entry = cache
                .try_get_or_insert("key", || async { Ok::<_, std::io::Error>(42) })
                .await
                .unwrap();
            assert_eq!(*entry.value(), 42);
        });
    }

    #[test]
    fn cache_optionally_get_or_insert_some() {
        block_on(async {
            let cache = build_cache();
            let entry = cache.optionally_get_or_insert("key", || async { Some(42) }).await.unwrap();
            assert_eq!(*entry.unwrap().value(), 42);
        });
    }

    #[test]
    fn cache_optionally_get_or_insert_none() {
        block_on(async {
            let cache = build_cache();
            let result = cache.optionally_get_or_insert("key", || async { None }).await.unwrap();
            assert!(result.is_none());
        });
    }

    #[test]
    fn cache_optionally_get_or_insert_hit() {
        block_on(async {
            let cache = build_cache();
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            let entry = cache.optionally_get_or_insert("key", || async { Some(99) }).await.unwrap();
            assert_eq!(*entry.unwrap().value(), 1);
        });
    }

    #[test]
    fn cache_optionally_get_or_insert_with_stampede() {
        block_on(async {
            let cache = build_cache_with_stampede();
            let entry = cache.optionally_get_or_insert("key", || async { Some(42) }).await.unwrap();
            assert_eq!(*entry.unwrap().value(), 42);
        });
    }
}

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
            .event_handler(super::refresh_paths::Completion(std::sync::Mutex::new(Some(sender))))
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

mod transform_tier {
    fn transform_cache<K, V, KT, VT>(
        inner: cachet_tier::MockCache<KT, VT>,
        encoder: impl cachet::Encoder<K, KT> + 'static,
        codec: impl cachet::Codec<V, VT> + 'static,
    ) -> Cache<K, V>
    where
        K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
        KT: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
        VT: Clone + Send + Sync + 'static,
    {
        let clock = Clock::new_frozen();
        Cache::builder(clock.clone())
            .storage(cachet_tier::MockCache::<K, V>::new())
            .transform(encoder, codec)
            .fallback(Cache::builder(clock).storage(inner))
            .build()
    }

    #[derive(Debug)]
    struct SoftFailureCodec;
    impl Encoder<i32, i32> for SoftFailureCodec {
        fn encode(&self, value: &i32) -> Result<i32, cachet::Error> {
            Ok(*value)
        }
    }
    impl Codec<i32, i32> for SoftFailureCodec {
        fn decode(&self, _: i32) -> Result<DecodeOutcome<i32>, cachet::Error> {
            Ok(DecodeOutcome::SoftFailure("test failure"))
        }
    }

    use cachet::{Cache, CacheEntry, CacheTier, Codec, DecodeOutcome, Encoder, TransformCodec, TransformEncoder, infallible_owned};
    use cachet_tier::MockCache;
    use tick::Clock;

    #[test]
    fn infallible_encoder_closure_is_covered() {
        let encoder = TransformEncoder::infallible(|k: &i32| k.to_string());
        assert_eq!(encoder.encode(&42).unwrap(), "42");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn insert_delegates_to_inner() {
        let inner = MockCache::<i32, i32>::new();
        let inner_check = inner.clone();
        let adapter = transform_cache(
            inner,
            TransformEncoder::new(|k: &String| k.parse::<i32>()),
            TransformCodec::new(|v: &String| v.parse::<i32>(), infallible_owned(|v: i32| v.to_string())),
        );

        adapter.insert("42".to_string(), CacheEntry::new("100".to_string())).await.unwrap();

        let stored = inner_check.get(&42).await.unwrap().unwrap();
        assert_eq!(*stored.value(), 100);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn get_preserves_ttl_and_cached_at() {
        use std::time::Duration;

        use cachet::infallible;

        let ttl = Duration::from_mins(5);
        let cached_at = Clock::new_frozen().system_time();
        let mut entry = CacheEntry::new(42);
        entry.set_ttl(ttl);
        entry.ensure_cached_at(cached_at);

        let inner = MockCache::with_data(std::iter::once((1, entry)).collect());
        let adapter = transform_cache(
            inner,
            TransformEncoder::new(|k: &i32| Ok::<_, std::convert::Infallible>(*k)),
            TransformCodec::new(infallible(|v: &i32| *v), infallible_owned(|v: i32| v)),
        );

        let result = adapter.get(&1).await.unwrap().unwrap();
        assert_eq!(*result.value(), 42);
        assert_eq!(result.ttl(), Some(ttl));
        assert_eq!(result.cached_at(), Some(cached_at));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn get_returns_none_on_soft_failure() {
        let inner = MockCache::with_data(std::iter::once((1, CacheEntry::new(42))).collect());
        let adapter = transform_cache(
            inner,
            TransformEncoder::new(|k: &i32| Ok::<_, std::convert::Infallible>(*k)),
            SoftFailureCodec,
        );

        let result = adapter.get(&1).await.unwrap();
        assert!(result.is_none(), "soft failure should return None");
    }
}

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
        assert!(!read_entry(&inner_check, &wrapper, Some(entry)).is_none());
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
        assert!(!read_entry(&inner_check, &wrapper, Some(entry)).is_none());
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

#[cfg(feature = "serialize")]
mod serialization {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use bytesbuf::BytesView;
    use bytesbuf::mem::{CallbackMemory, GlobalPool, OpaqueMemory};
    use cachet::{Cache, CacheEntry, CacheOp, CacheTier, Error};
    use cachet_tier::MockCache;
    use serde::{Deserialize, Serialize};
    use thread_aware::unaware;
    use tick::Clock;

    #[derive(Clone, Deserialize)]
    struct FailSerialize;

    impl Serialize for FailSerialize {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("intentional failure"))
        }
    }

    fn counting_memory() -> (OpaqueMemory, Arc<AtomicUsize>) {
        let reservations = Arc::new(AtomicUsize::new(0));
        let provider = CallbackMemory::new(unaware(Arc::clone(&reservations)), |counter, min_bytes| {
            counter.fetch_add(1, Ordering::SeqCst);
            GlobalPool::new().reserve(min_bytes)
        });
        (OpaqueMemory::new(provider), reservations)
    }

    fn encode<T>(value: T, memory: OpaqueMemory) -> Result<BytesView, Error>
    where
        T: Clone + Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    {
        futures::executor::block_on(async {
            let clock = Clock::new_frozen();
            let storage = MockCache::<BytesView, BytesView>::new();
            let cache = Cache::builder::<u32, T>(clock.clone())
                .storage(MockCache::new())
                .serialize(memory)
                .fallback(Cache::builder(clock).storage(storage.clone()))
                .build();
            cache.insert(1, CacheEntry::new(value)).await?;
            let Some(CacheOp::Insert { entry, .. }) = storage.operations().into_iter().find(|op| matches!(op, CacheOp::Insert { .. }))
            else {
                panic!("successful serialized insert must reach storage");
            };
            Ok(entry.into_value())
        })
    }

    fn decode(value: BytesView) -> String {
        futures::executor::block_on(async {
            let clock = Clock::new_frozen();
            let storage = MockCache::<BytesView, BytesView>::new();
            let mut key = vec![1];
            key.extend(postcard::to_allocvec(&1u32).unwrap());
            storage.insert(BytesView::from(key), CacheEntry::new(value)).await.unwrap();
            let cache = Cache::builder::<u32, String>(clock.clone())
                .storage(MockCache::new())
                .serialize(GlobalPool::new())
                .fallback(Cache::builder(clock).storage(storage))
                .build();
            cache.get(&1).await.unwrap().unwrap().into_value()
        })
    }

    #[test]
    fn encode_serialization_failure_returns_err() {
        assert!(encode(FailSerialize, GlobalPool::new().into()).is_err());
    }

    #[test]
    fn encode_decode_roundtrip() {
        let original = "hello, world!".to_string();
        let encoded = encode(original.clone(), GlobalPool::new().into()).unwrap();
        assert_eq!(decode(encoded), original);
    }

    #[test]
    fn encoder_encode_produces_valid_output() {
        let clock = Clock::new_frozen();
        let storage = MockCache::<BytesView, BytesView>::new();
        let cache = Cache::builder::<u32, u32>(clock.clone())
            .storage(MockCache::new())
            .serialize(GlobalPool::new())
            .fallback(Cache::builder(clock).storage(storage.clone()))
            .build();
        assert!(futures::executor::block_on(cache.get(&42)).unwrap().is_none());
        let Some(CacheOp::Get(encoded)) = storage.operations().into_iter().next() else {
            panic!("serialized cache miss must query storage");
        };
        let bytes = encoded.to_vec();
        assert_eq!(bytes[0], 1, "first byte should be format version");
        assert_eq!(postcard::from_bytes::<u32>(&bytes[1..]).unwrap(), 42);
    }

    #[test]
    fn encoder_uses_supplied_memory_provider() {
        let (memory, reservations) = counting_memory();
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<u32, u32>(clock.clone())
            .storage(MockCache::new())
            .serialize(memory)
            .fallback(Cache::builder(clock).storage(MockCache::<BytesView, BytesView>::new()))
            .build();
        assert!(futures::executor::block_on(cache.get(&42)).unwrap().is_none());
        assert!(reservations.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn codec_uses_supplied_memory_provider() {
        let (memory, reservations) = counting_memory();
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<u32, u32>(clock.clone())
            .storage(MockCache::new())
            .serialize(memory)
            .fallback(Cache::builder(clock).storage(MockCache::<BytesView, BytesView>::new()))
            .build();
        assert!(futures::executor::block_on(cache.get(&1)).unwrap().is_none());
        let key_reservations = reservations.load(Ordering::SeqCst);
        futures::executor::block_on(cache.insert(1, CacheEntry::new(42))).unwrap();
        assert!(
            reservations.load(Ordering::SeqCst) > key_reservations.saturating_mul(2),
            "value encoding must reserve memory in addition to the two key encodings"
        );
    }

    #[test]
    fn decode_multi_span_view() {
        let original = "multi-span test".to_string();
        let bytes = encode(original.clone(), GlobalPool::new().into()).unwrap().to_vec();
        let mid = bytes.len() / 2;
        let mut first_half = BytesView::from(bytes[..mid].to_vec());
        first_half.append(BytesView::from(bytes[mid..].to_vec()));
        assert_ne!(first_half.first_slice().len(), first_half.len(), "should be multi-span");
        assert_eq!(decode(first_half), original);
    }
}

mod refresh {
    use std::time::{Duration, SystemTime};

    use anyspawn::Spawner;
    use cachet::{Cache, CacheEntry, CacheTier, TimeToRefresh};
    use cachet_tier::MockCache;
    use tick::Clock;

    fn create_refresh() -> TimeToRefresh<String> {
        TimeToRefresh::new(Duration::from_mins(1), Spawner::new_tokio(None))
    }

    fn starts_refresh(refresh: TimeToRefresh<String>, cached_at: SystemTime, now: SystemTime) -> bool {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(async {
            let clock = Clock::new_frozen_at(now);
            let primary = MockCache::new();
            let fallback = MockCache::<String, i32>::new();
            let mut entry = CacheEntry::new(42);
            entry.ensure_cached_at(cached_at);
            primary.insert("key".to_string(), entry).await.unwrap();
            let cache = Cache::builder(clock.clone())
                .storage(primary)
                .fallback(Cache::builder(clock).storage(fallback.clone()))
                .time_to_refresh(refresh)
                .build();
            assert_eq!(*cache.get("key").await.unwrap().unwrap().value(), 42);
            tokio::task::yield_now().await;
            !fallback.operations().is_empty()
        })
    }
    #[test]
    fn time_to_refresh_new() {
        let refresh = create_refresh();

        assert_eq!(refresh.duration, Duration::from_mins(1));
    }

    #[test]
    fn time_to_refresh_duration_access() {
        let refresh: TimeToRefresh<String> = TimeToRefresh::new(Duration::from_mins(5), Spawner::new_tokio(None));

        assert_eq!(refresh.duration, Duration::from_mins(5));
    }

    #[test]
    fn time_to_refresh_debug() {
        let refresh = create_refresh();
        let debug_str = format!("{refresh:?}");
        assert!(debug_str.contains("TimeToRefresh"), "got: {debug_str}");
        assert!(debug_str.contains("duration"), "got: {debug_str}");
    }

    #[test]
    fn time_to_refresh_should_refresh_false_when_recent() {
        let refresh = create_refresh();
        let clock = Clock::new_frozen();

        // An entry cached at the current time should not need refresh yet
        let now = clock.system_time();
        assert!(!starts_refresh(refresh, now, now));
    }

    #[test]
    fn time_to_refresh_should_refresh_true_when_clock_goes_backward() {
        let refresh: TimeToRefresh<String> = TimeToRefresh::new(Duration::from_mins(5), Spawner::new_tokio(None));
        let clock = Clock::new_frozen();

        // cached_at in the future relative to now causes duration_since to return Err
        let now = clock.system_time();
        let cached_at = now + Duration::from_hours(1);
        assert!(
            starts_refresh(refresh, cached_at, now),
            "should return true when system time goes backward"
        );
    }

    #[test]
    fn time_to_refresh_should_refresh_true_after_duration() {
        let refresh: TimeToRefresh<String> = TimeToRefresh::new(Duration::from_mins(1), Spawner::new_tokio(None));
        let clock = Clock::new_frozen();

        // cached_at is 61 seconds before now, exceeding the 60s refresh duration
        let now = clock.system_time();
        let cached_at = now - Duration::from_secs(61);

        assert!(starts_refresh(refresh, cached_at, now));
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
            capture.assert_contains("DEBUG");
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
            .event_handler(Ids(ids.clone()))
            .build();
        for _ in 0..3 {
            assert!(futures::executor::block_on(cache.get(&7)).unwrap().is_none());
        }
        let ids = ids.lock().unwrap();
        assert!(ids[1] > ids[0]);
        assert!(ids[2] > ids[1]);
    }
}

mod inner_rejection {
    #[derive(Debug)]
    struct RejectingTier {
        storage: MockCache<String, i32>,
        control: Option<ClockControl>,
    }

    impl CacheTier<String, i32> for RejectingTier {
        async fn get(&self, key: &String) -> Result<Option<CacheEntry<i32>>, Error> {
            self.storage.get(key).await
        }
        async fn insert(&self, _: String, _: CacheEntry<i32>) -> Result<InsertOutcome, Error> {
            if let Some(control) = &self.control {
                control.advance(Duration::from_millis(7));
            }
            Ok(InsertOutcome::Rejected)
        }
        async fn invalidate(&self, key: &String) -> Result<(), Error> {
            self.storage.invalidate(key).await
        }
        async fn clear(&self) -> Result<(), Error> {
            self.storage.clear().await
        }
        async fn len(&self) -> Result<u64, SizeError> {
            self.storage.len().await
        }
    }

    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use cachet::{Cache, CacheEntry, CacheEventHandler, CacheOperationEvent, CacheTier, CacheTierEvent, Error, InsertOutcome, SizeError};
    use cachet_tier::MockCache;
    use tick::{Clock, ClockControl};
    #[derive(Clone)]
    struct RejectionDurationHandler {
        durations: Arc<Mutex<Vec<Duration>>>,
    }

    impl CacheEventHandler for RejectionDurationHandler {
        fn on_tier_event(&self, event: &CacheTierEvent<'_>) {
            if event.outcome == cachet::telemetry::attributes::EVENT_INSERT_REJECTED {
                self.durations.lock().unwrap().push(event.duration);
            }
        }

        fn on_operation_complete(&self, _event: &CacheOperationEvent<'_>) {}
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn wrapper_propagates_inner_rejection() {
        let clock = Clock::new_frozen();
        let inner = RejectingTier {
            storage: MockCache::new(),
            control: None,
        };
        let wrapper = Cache::builder(clock).storage(inner).name("test").build();

        let outcome = wrapper.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();

        assert_eq!(outcome, InsertOutcome::Rejected);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn wrapper_records_inner_rejection_duration() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let duration = Duration::from_millis(7);
        let durations = Arc::new(Mutex::new(Vec::new()));
        let inner = RejectingTier {
            storage: MockCache::new(),
            control: Some(control),
        };
        let cache = cachet::Cache::builder::<String, i32>(clock)
            .storage(inner)
            .event_handler(RejectionDurationHandler {
                durations: Arc::clone(&durations),
            })
            .build();

        let outcome = cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();

        assert_eq!(outcome, InsertOutcome::Rejected);
        assert_eq!(*durations.lock().unwrap(), vec![duration]);
        assert!(cache.get("key").await.unwrap().is_none());
        cache.invalidate("key").await.unwrap();
        cache.clear().await.unwrap();
    }
}

mod refresh_paths {
    use std::sync::Mutex;
    use std::time::Duration;

    use cachet::{
        Cache, CacheEntry, CacheEventHandler, CacheOperationEvent, CacheTier, CacheTierEvent, Error, InsertOutcome, InsertPolicy,
        SizeError, TimeToRefresh,
    };
    use cachet_tier::MockCache;
    use futures::channel::oneshot;
    use futures::executor::block_on;
    use tick::Clock;

    #[derive(Debug)]
    struct StalePrimary {
        storage: MockCache<String, i32>,
        clock: Clock,
    }

    impl CacheTier<String, i32> for StalePrimary {
        async fn get(&self, _: &String) -> Result<Option<CacheEntry<i32>>, Error> {
            let mut entry = CacheEntry::new(0);
            entry.ensure_cached_at(self.clock.system_time() - Duration::from_secs(60));
            Ok(Some(entry))
        }
        async fn insert(&self, key: String, entry: CacheEntry<i32>) -> Result<InsertOutcome, Error> {
            self.storage.insert(key, entry).await
        }
        async fn invalidate(&self, key: &String) -> Result<(), Error> {
            self.storage.invalidate(key).await
        }
        async fn clear(&self) -> Result<(), Error> {
            self.storage.clear().await
        }
        async fn len(&self) -> Result<u64, SizeError> {
            self.storage.len().await
        }
    }

    pub(super) struct Completion(pub(super) Mutex<Option<oneshot::Sender<()>>>);
    impl CacheEventHandler for Completion {
        fn on_tier_event(&self, _: &CacheTierEvent<'_>) {}
        fn on_operation_complete(&self, event: &CacheOperationEvent<'_>) {
            if event.operation == "cache.refresh" {
                self.0.lock().unwrap().take().unwrap().send(()).unwrap();
            }
        }
    }

    fn run_refresh(primary: MockCache<String, i32>, fallback: MockCache<String, i32>, policy: InsertPolicy<i32>, reads: usize) {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(async {
            let (sender, completed) = oneshot::channel();
            let clock = Clock::new_frozen();
            let builder = Cache::builder(clock.clone())
                .name("test")
                .storage(StalePrimary {
                    storage: primary,
                    clock: clock.clone(),
                })
                .insert_policy(policy)
                .event_handler(Completion(Mutex::new(Some(sender))));
            #[cfg(feature = "logs")]
            let builder = builder.enable_logs();
            let cache = builder
                .fallback(Cache::builder(clock).storage(fallback))
                .time_to_refresh(TimeToRefresh::new(Duration::ZERO, anyspawn::Spawner::new_tokio(None)))
                .build();
            for _ in 0..reads {
                assert!(cache.get("key").await.unwrap().is_some());
            }
            completed.await.unwrap();
        });
    }

    #[cfg(feature = "logs")]
    #[test]
    fn fallback_miss_logs_refresh_miss_telemetry() {
        let capture = testing_aids::tracing_logs::Capture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());
        run_refresh(MockCache::new(), MockCache::new(), InsertPolicy::always(), 1);
        capture.assert_contains(cachet::telemetry::attributes::FIELD_EVENT);
        capture.assert_contains(cachet::telemetry::attributes::EVENT_REFRESH_MISS);
    }

    #[cfg(feature = "logs")]
    #[test]
    fn fallback_error_logs_refresh_error_telemetry() {
        let capture = testing_aids::tracing_logs::Capture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());
        let fallback = MockCache::new();
        fallback.fail_when(|_| true);
        run_refresh(MockCache::new(), fallback, InsertPolicy::always(), 1);
        capture.assert_contains(cachet::telemetry::attributes::FIELD_EVENT);
        capture.assert_contains(cachet::telemetry::attributes::EVENT_REFRESH_ERROR);
    }

    #[test]
    fn hit_no_promote() {
        let primary = MockCache::new();
        let fallback = MockCache::new();
        block_on(fallback.insert("key".to_string(), CacheEntry::new(42))).unwrap();
        run_refresh(primary.clone(), fallback, InsertPolicy::never(), 1);
        assert!(block_on(primary.get(&"key".to_string())).unwrap().is_none());
    }

    #[test]
    fn hit_with_promote() {
        let primary = MockCache::new();
        let fallback = MockCache::new();
        block_on(fallback.insert("key".to_string(), CacheEntry::new(42))).unwrap();
        run_refresh(primary.clone(), fallback, InsertPolicy::always(), 1);
        let result = block_on(primary.get(&"key".to_string())).unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
    }

    #[test]
    fn promote_error() {
        let primary = MockCache::new();
        primary.fail_when(|_| true);
        let fallback = MockCache::new();
        block_on(fallback.insert("key".to_string(), CacheEntry::new(42))).unwrap();
        run_refresh(primary, fallback, InsertPolicy::always(), 1);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn do_refresh_already_in_flight_returns_early() {
        let fallback = MockCache::new();
        run_refresh(MockCache::new(), fallback.clone(), InsertPolicy::always(), 2);
        assert_eq!(fallback.operations().len(), 1);
    }
}
