// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Background cache refresh with time-to-refresh (TTR) support.
//!
//! This module provides background refresh capabilities for fallback caches,
//! allowing stale entries to be refreshed from the fallback tier without
//! blocking the primary cache path.

use std::{
    collections::HashSet,
    fmt::Debug,
    hash::Hash,
    sync::Arc,
    time::{Duration, Instant},
};

use anyspawn::Spawner;
use parking_lot::Mutex;

use crate::{
    fallback::{FallbackCache, FallbackCacheInner},
    telemetry::{
        CacheActivity, CacheOperation,
        ext::{CacheTelemetryExt, ClockExt},
    },
};

use cachelon_tier::{CacheEntry, CacheTier};

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
    /// Creates a new `TimeToRefresh` instance.
    ///
    /// # Arguments
    ///
    /// * `duration` - The time period after which cached entries should be refreshed.
    /// * `spawner` - The spawner used to run background refresh tasks.
    #[must_use]
    pub fn new(duration: Duration, spawner: Spawner) -> Self {
        Self {
            duration,
            spawner,
            in_flight: Mutex::new(HashSet::new()),
        }
    }

    pub(crate) fn should_refresh(&self, cached_at: Instant) -> bool {
        cached_at.elapsed() >= self.duration
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

            // Fire-and-forget: spawn the refresh task in the background, drop the JoinHandle
            drop(refresh.spawner.spawn(async move {
                inner.fetch_and_promote(key.clone()).await;

                // Mark as no longer in-flight
                if let Some(refresh) = &inner.refresh {
                    refresh.finish_refresh(&key);
                }
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
        let timed = self.clock.timed_async(self.primary.insert(&key, value)).await;

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
    use super::*;

    fn create_refresh() -> TimeToRefresh<String> {
        TimeToRefresh::new(Duration::from_secs(60), Spawner::new_tokio())
    }

    #[test]
    fn time_to_refresh_debug() {
        let refresh = create_refresh();

        let debug_str = format!("{refresh:?}");
        assert!(debug_str.contains("TimeToRefresh"));
        assert!(debug_str.contains("duration"));
    }

    #[test]
    fn time_to_refresh_new() {
        let refresh = create_refresh();

        assert_eq!(refresh.duration, Duration::from_secs(60));
    }

    #[test]
    fn time_to_refresh_should_refresh_false_when_recent() {
        let refresh = create_refresh();

        // An instant from now should not need refresh yet
        let cached_at = Instant::now();
        assert!(!refresh.should_refresh(cached_at));
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
    fn time_to_refresh_should_refresh_true_after_duration() {
        // Set a very short refresh duration for testing
        let refresh: TimeToRefresh<String> = TimeToRefresh::new(Duration::from_nanos(1), Spawner::new_tokio());

        // Create an instant and wait slightly
        let cached_at = Instant::now();
        // Use a small spin to allow some time to pass
        std::thread::sleep(Duration::from_millis(1));

        // Now it should need refresh
        assert!(refresh.should_refresh(cached_at));
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
}
