// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache operations through public APIs.

#![cfg(test)]

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
