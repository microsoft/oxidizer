// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Insert rejection through public APIs.

#![cfg(test)]

testing_aids::init_tracing!();

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
