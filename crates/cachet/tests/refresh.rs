// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Refresh through public APIs.

#![cfg(test)]

testing_aids::init_tracing!();

mod support;

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
        let _runtime_guard = runtime.enter();
        futures::executor::block_on(async {
            let clock = Clock::new_frozen_at(now);
            let primary = MockCache::new();
            let fallback = MockCache::<String, i32>::new();
            let mut entry = CacheEntry::new(42);
            entry.ensure_cached_at(cached_at);
            primary.insert("key".to_string(), entry).await.unwrap();
            let cache = Cache::builder(clock.clone())
                .storage(primary)
                .fallback(Cache::builder(clock).storage(fallback))
                .time_to_refresh(refresh)
                .build();
            assert_eq!(*cache.get("key").await.unwrap().unwrap().value(), 42);
            runtime.metrics().num_alive_tasks() != 0
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

mod refresh_paths {
    use std::sync::Mutex;
    use std::time::Duration;

    use cachet::{Cache, CacheEntry, CacheTier, Error, InsertOutcome, InsertPolicy, SizeError, TimeToRefresh};
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
            entry.ensure_cached_at(self.clock.system_time() - Duration::from_mins(1));
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

    use crate::support::Completion;

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
