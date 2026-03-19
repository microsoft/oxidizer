// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Background cache refresh with time-to-refresh (TTR) support.
//!
//! This module provides background refresh capabilities for fallback caches,
//! allowing stale entries to be refreshed from the fallback tier without
//! blocking the primary cache path.

use std::collections::HashSet;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyspawn::Spawner;
use cachet_tier::{CacheEntry, CacheTier};
use parking_lot::Mutex;

use crate::fallback::{FallbackCache, FallbackCacheInner};
use crate::telemetry::ext::ClockExt;
use crate::telemetry::{CacheActivity, CacheOperation};

/// Configuration for background cache refresh.
///
/// When entries in the primary tier exceed the specified duration, they
/// are asynchronously refreshed from the fallback tier in the background.
/// This prevents stale data while avoiding blocking cache reads.
///
/// # Examples
///
/// ```ignore
/// use anyspawn::Spawner;
/// use std::time::Duration;
///
/// let refresh = TimeToRefresh::new(Duration::from_secs(300), Spawner::new_tokio());
/// ```
pub struct TimeToRefresh<K> {
    /// The duration after which a cached entry should be refreshed.
    pub duration: Duration,
    pub(crate) spawner: Spawner,
    in_flight: Mutex<HashSet<K>>,
}

impl<K> Debug for TimeToRefresh<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimeToRefresh")
            .field("duration", &self.duration)
            .finish_non_exhaustive()
    }
}

impl<K> TimeToRefresh<K>
where
    K: Clone + Eq + Hash + Send + 'static,
{
    /// Creates a new `TimeToRefresh` with the given duration and spawner.
    ///
    /// The `duration` specifies how long after insertion an entry becomes stale
    /// and eligible for background refresh. The `spawner` executes refresh tasks
    /// asynchronously without blocking cache reads.
    #[must_use]
    pub fn new(duration: Duration, spawner: Spawner) -> Self {
        Self {
            duration,
            spawner,
            in_flight: Mutex::new(HashSet::new()),
        }
    }

    pub(crate) fn should_refresh(&self, cached_at: SystemTime, now: SystemTime) -> bool {
        match now.duration_since(cached_at) {
            Ok(elapsed) => elapsed >= self.duration,
            Err(_) => true, // If the system time went backwards, consider it stale
        }
    }

    /// Returns true if this key was successfully marked as in-flight (i.e., not already refreshing).
    pub(crate) fn try_start_refresh(&self, key: &K) -> bool {
        self.in_flight.lock().insert(key.clone())
    }

    /// Marks the key as no longer in-flight.
    pub(crate) fn finish_refresh(&self, key: &K) {
        self.in_flight.lock().remove(key);
    }
}

/// A generic drop guard that runs a cleanup closure when dropped.
///
/// This ensures cleanup logic executes even during a panic unwind.
struct DropGuard<F: FnMut()>(F);

impl<F: FnMut()> Drop for DropGuard<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

impl<K, V, P, F> FallbackCache<K, V, P, F>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    P: CacheTier<K, V> + Send + Sync + 'static,
    F: CacheTier<K, V> + Send + Sync + 'static,
{
    /// Triggers a background refresh for the given key.
    ///
    /// If a refresh is already in progress for this key, this method returns
    /// immediately without spawning a duplicate task. Otherwise, it spawns
    /// an async task to fetch the value from the fallback tier and promote
    /// it to the primary tier.
    pub fn do_refresh(&self, key: &K) {
        if let Some(refresh) = &self.inner.refresh {
            // Check if already in-flight on this thread
            if !refresh.try_start_refresh(key) {
                return;
            }

            let inner = Arc::clone(&self.inner);
            let key = key.clone();

            // Fire-and-forget: spawn the refresh task in the background, drop the JoinHandle.
            // The guard ensures finish_refresh runs even if fetch_and_promote panics.
            drop(refresh.spawner.spawn(async move {
                let _guard = DropGuard({
                    let inner = Arc::clone(&inner);
                    let key = key.clone();
                    move || {
                        if let Some(refresh) = &inner.refresh {
                            refresh.finish_refresh(&key);
                        }
                    }
                });
                inner.fetch_and_promote(key).await;
            }));
        }
    }
}

impl<K, V, P, F> FallbackCacheInner<K, V, P, F>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    P: CacheTier<K, V> + Send + Sync + 'static,
    F: CacheTier<K, V> + Send + Sync + 'static,
{
    pub(crate) async fn fetch_and_promote(&self, key: K) {
        let timed = self.clock.timed_async(self.fallback.get(&key)).await;

        match timed.result {
            Ok(Some(value)) => self.handle_fallback_hit(key, value, timed.duration).await,
            Ok(None) | Err(_) => self.handle_fallback_miss(timed.duration),
        }
    }

    async fn handle_fallback_hit(&self, key: K, value: CacheEntry<V>, fetch_duration: Duration) {
        self.telemetry
            .record(self.name, CacheOperation::Get, CacheActivity::RefreshHit, fetch_duration);

        if self.policy.should_promote(&value) {
            self.promote_to_primary(key, value).await;
        }
    }

    async fn promote_to_primary(&self, key: K, value: CacheEntry<V>) {
        let timed = self.clock.timed_async(self.primary.insert(key, value)).await;

        match timed.result {
            Ok(()) => {
                self.telemetry
                    .record(self.name, CacheOperation::Insert, CacheActivity::FallbackPromotion, timed.duration);
            }
            Err(_) => {
                self.telemetry
                    .record(self.name, CacheOperation::Insert, CacheActivity::Error, timed.duration);
            }
        }
    }

    fn handle_fallback_miss(&self, duration: Duration) {
        self.telemetry
            .record(self.name, CacheOperation::Get, CacheActivity::RefreshMiss, duration);
    }
}

#[cfg(test)]
mod tests {
    use tick::Clock;

    use super::*;

    fn create_refresh() -> TimeToRefresh<String> {
        TimeToRefresh::new(Duration::from_secs(60), Spawner::new_tokio())
    }

    #[test]
    fn time_to_refresh_new() {
        let refresh = create_refresh();

        assert_eq!(refresh.duration, Duration::from_secs(60));
    }

    #[test]
    fn time_to_refresh_should_refresh_false_when_recent() {
        let refresh = create_refresh();
        let clock = Clock::new_frozen();

        // An entry cached at the current time should not need refresh yet
        let now = clock.system_time();
        assert!(!refresh.should_refresh(now, now));
    }

    #[test]
    fn time_to_refresh_try_start_refresh() {
        let refresh = create_refresh();

        // First call should succeed (not already in flight)
        let key = "key1".to_string();
        assert!(refresh.try_start_refresh(&key));

        // Second call with same key should fail (already in flight)
        assert!(!refresh.try_start_refresh(&key));

        // Different key should succeed
        let key2 = "key2".to_string();
        assert!(refresh.try_start_refresh(&key2));
    }

    #[test]
    fn time_to_refresh_finish_refresh() {
        let refresh = create_refresh();

        let key = "key1".to_string();

        // Start refresh
        assert!(refresh.try_start_refresh(&key));
        // Can't start again
        assert!(!refresh.try_start_refresh(&key));

        // Finish refresh
        refresh.finish_refresh(&key);

        // Now can start again
        assert!(refresh.try_start_refresh(&key));
    }

    #[test]
    fn time_to_refresh_finish_refresh_nonexistent_key() {
        let refresh = create_refresh();

        // Finishing a non-existent key should not panic
        // Test passes if this doesn't panic
        refresh.finish_refresh(&"nonexistent".to_string());

        // Verify we can still use the refresh object after
        assert!(refresh.try_start_refresh(&"other_key".to_string()));
    }

    #[test]
    fn time_to_refresh_should_refresh_true_when_clock_goes_backward() {
        let refresh: TimeToRefresh<String> = TimeToRefresh::new(Duration::from_secs(300), Spawner::new_tokio());
        let clock = Clock::new_frozen();

        // cached_at in the future relative to now causes duration_since to return Err
        let now = clock.system_time();
        let cached_at = now + Duration::from_secs(3600);
        assert!(
            refresh.should_refresh(cached_at, now),
            "should return true when system time goes backward"
        );
    }

    #[test]
    fn time_to_refresh_should_refresh_true_after_duration() {
        let refresh: TimeToRefresh<String> = TimeToRefresh::new(Duration::from_secs(60), Spawner::new_tokio());
        let clock = Clock::new_frozen();

        // cached_at is 61 seconds before now, exceeding the 60s refresh duration
        let now = clock.system_time();
        let cached_at = now - Duration::from_secs(61);

        assert!(refresh.should_refresh(cached_at, now));
    }

    #[test]
    fn time_to_refresh_duration_access() {
        let refresh: TimeToRefresh<String> = TimeToRefresh::new(Duration::from_secs(300), Spawner::new_tokio());

        assert_eq!(refresh.duration, Duration::from_secs(300));
    }

    #[test]
    fn time_to_refresh_concurrent_keys() {
        let refresh = create_refresh();

        // Multiple keys can be in flight simultaneously
        let key1 = "key1".to_string();
        let key2 = "key2".to_string();
        let key3 = "key3".to_string();

        assert!(refresh.try_start_refresh(&key1));
        assert!(refresh.try_start_refresh(&key2));
        assert!(refresh.try_start_refresh(&key3));

        // All three should be blocked now
        assert!(!refresh.try_start_refresh(&key1));
        assert!(!refresh.try_start_refresh(&key2));
        assert!(!refresh.try_start_refresh(&key3));

        // Finish one
        refresh.finish_refresh(&key2);

        // key2 can start again, others still blocked
        assert!(!refresh.try_start_refresh(&key1));
        assert!(refresh.try_start_refresh(&key2));
        assert!(!refresh.try_start_refresh(&key3));
    }

    #[test]
    fn time_to_refresh_debug() {
        let refresh = create_refresh();
        let debug_str = format!("{refresh:?}");
        assert!(debug_str.contains("TimeToRefresh"), "got: {debug_str}");
        assert!(debug_str.contains("duration"), "got: {debug_str}");
    }
}

#[cfg(test)]
mod fetch_and_promote_tests {
    use cachet_tier::MockCache;
    use testing_aids::MetricTester;
    use tick::Clock;

    use super::*;
    use crate::fallback::FallbackPromotionPolicy;
    use crate::telemetry::{TelemetryConfig, attributes};
    use crate::wrapper::CacheWrapper;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures::executor::block_on(f)
    }

    fn build_fallback_cache<P, F: CacheTier<String, i32> + 'static>(
        primary: P,
        fallback: F,
        policy: FallbackPromotionPolicy<i32>,
    ) -> FallbackCache<String, i32, P, F> {
        let clock = Clock::new_frozen();
        let telemetry = TelemetryConfig::new().build();
        FallbackCache::new("test", primary, fallback, policy, clock, None, telemetry)
    }

    #[cfg_attr(miri, ignore)] // OTel SDK calls SystemTime::now() which miri blocks under isolation
    #[test]
    fn fallback_miss_records_refresh_miss_telemetry() {
        block_on(async {
            let tester = MetricTester::new();
            let clock = Clock::new_frozen();
            let telemetry = TelemetryConfig::new().with_metrics(tester.meter_provider()).build();
            let primary = MockCache::<String, i32>::new();
            let fallback = MockCache::<String, i32>::new();
            let fc = FallbackCache::new("test", primary, fallback, FallbackPromotionPolicy::always(), clock, None, telemetry);

            // Fallback is empty → handle_fallback_miss Ok(None) branch
            fc.inner.fetch_and_promote("missing".to_string()).await;

            tester.assert_attributes_contain(&[opentelemetry::KeyValue::new(attributes::CACHE_ACTIVITY_NAME, "cache.refresh_miss")]);
        });
    }

    #[test]
    fn fallback_error() {
        block_on(async {
            let primary = MockCache::<String, i32>::new();
            let fallback = MockCache::<String, i32>::new();
            fallback.fail_when(|_| true);
            let fc = build_fallback_cache(primary, fallback, FallbackPromotionPolicy::always());

            // Fallback errors → handle_fallback_miss Err branch
            fc.inner.fetch_and_promote("key".to_string()).await;
        });
    }

    #[test]
    fn hit_no_promote() {
        block_on(async {
            let primary = MockCache::<String, i32>::new();
            let fallback = MockCache::<String, i32>::new();
            fallback.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

            let fc = build_fallback_cache(primary.clone(), fallback, FallbackPromotionPolicy::never());

            // Fallback hit with never() policy → handle_fallback_hit without promotion
            fc.inner.fetch_and_promote("key".to_string()).await;

            // Primary should still be empty
            assert!(primary.get(&"key".to_string()).await.unwrap().is_none());
        });
    }

    #[test]
    fn hit_with_promote() {
        block_on(async {
            let primary = MockCache::<String, i32>::new();
            let fallback = MockCache::<String, i32>::new();
            fallback.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

            let fc = build_fallback_cache(primary.clone(), fallback, FallbackPromotionPolicy::always());

            // Fallback hit with always() policy → promote_to_primary success
            fc.inner.fetch_and_promote("key".to_string()).await;

            // Primary should now have the value
            let result = primary.get(&"key".to_string()).await.unwrap();
            assert!(result.is_some());
            assert_eq!(*result.unwrap().value(), 42);
        });
    }

    #[test]
    fn promote_error() {
        block_on(async {
            let primary = MockCache::<String, i32>::new();
            primary.fail_when(|_| true);
            let fallback = MockCache::<String, i32>::new();
            fallback.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

            let fc = build_fallback_cache(primary, fallback, FallbackPromotionPolicy::always());

            // Fallback hit, primary insert fails → promote_to_primary error path
            fc.inner.fetch_and_promote("key".to_string()).await;
        });
    }

    /// Regression test: if `fetch_and_promote` panics, the key must not remain
    /// permanently stuck in the `in_flight` set. Without an RAII guard, the
    /// `finish_refresh` call is skipped on panic, blocking all future refreshes
    /// for that key.
    #[tokio::test]
    async fn panic_in_refresh_does_not_leave_key_stuck_in_flight() {
        let primary = MockCache::<String, i32>::new();
        let fallback = MockCache::<String, i32>::new();
        fallback.fail_when(|_| panic!("simulated panic in fallback get"));

        let clock = Clock::new_frozen();
        let telemetry = TelemetryConfig::new().build();
        let refresh = TimeToRefresh::new(Duration::from_secs(60), Spawner::new_tokio());

        let fc = FallbackCache::new(
            "test",
            primary,
            fallback,
            FallbackPromotionPolicy::always(),
            clock,
            Some(refresh),
            telemetry,
        );

        let key = "panic_key".to_string();

        // Trigger background refresh — spawns a task that will panic
        fc.do_refresh(&key);

        // Give the spawned task time to run and panic
        tokio::time::sleep(Duration::from_millis(100)).await;

        // The key should NOT be stuck in in_flight.
        // try_start_refresh returns true if the key is NOT in the set.
        let can_refresh_again = fc
            .inner
            .refresh
            .as_ref()
            .expect("refresh should be configured")
            .try_start_refresh(&key);

        assert!(
            can_refresh_again,
            "key should not be stuck in in_flight after a panic in fetch_and_promote"
        );
    }

    type MockWrapper = CacheWrapper<String, i32, MockCache<String, i32>>;

    fn make_wrapper(mock: MockCache<String, i32>) -> MockWrapper {
        let clock = Clock::new_frozen();
        let telemetry = TelemetryConfig::new().build();
        CacheWrapper::new("test_primary", mock, clock, None, telemetry)
    }

    fn build_mock_fallback_cache(
        primary: MockWrapper,
        fallback: MockCache<String, i32>,
        policy: FallbackPromotionPolicy<i32>,
    ) -> FallbackCache<String, i32, MockWrapper, MockCache<String, i32>> {
        let clock = Clock::new_frozen();
        let telemetry = TelemetryConfig::new().build();
        FallbackCache::new("test", primary, fallback, policy, clock, None, telemetry)
    }

    #[test]
    fn do_refresh_no_refresh_configured() {
        let primary = make_wrapper(MockCache::new());
        let fallback = MockCache::<String, i32>::new();
        let fc = build_mock_fallback_cache(primary, fallback, FallbackPromotionPolicy::always());
        // do_refresh with no refresh configured should be a no-op
        fc.do_refresh(&"key".to_string());
    }

    /// Exercises the early return when a refresh is already in-flight for the
    /// same key (line `if !refresh.try_start_refresh(key) { return; }`).
    #[tokio::test]
    async fn do_refresh_already_in_flight_returns_early() {
        let primary = MockCache::<String, i32>::new();
        let fallback = MockCache::<String, i32>::new();
        let clock = Clock::new_frozen();
        let telemetry = TelemetryConfig::new().build();
        let refresh = TimeToRefresh::new(Duration::from_secs(60), Spawner::new_tokio());

        let primary_wrapper = CacheWrapper::new("primary", primary, clock.clone(), None, telemetry.clone());
        let fc = FallbackCache::new(
            "test",
            primary_wrapper,
            fallback,
            FallbackPromotionPolicy::always(),
            clock,
            Some(refresh),
            telemetry,
        );

        let key = "key".to_string();
        // First refresh adds key to in_flight set
        fc.do_refresh(&key);
        // Second refresh should hit early return since key is already in-flight
        fc.do_refresh(&key);

        // Give spawned tasks time to complete
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[test]
    fn drop_guard_runs_on_drop() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = Arc::clone(&ran);
        {
            let _guard = DropGuard(move || {
                ran_clone.store(true, Ordering::SeqCst);
            });
        }
        assert!(ran.load(Ordering::SeqCst));
    }
}
