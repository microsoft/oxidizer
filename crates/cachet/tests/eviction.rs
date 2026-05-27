// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for eviction telemetry emitted by the in-memory tier's
//! eviction listener.

#![cfg(feature = "memory")]

use std::time::Duration;

use cachet::{Cache, CacheEntry};
use testing_aids::LogCapture;
use tick::Clock;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;

/// Inserting past the configured `max_capacity` of the underlying moka cache
/// must eventually emit a `cache.eviction` event for the size-based removals.
#[cfg_attr(miri, ignore)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_size_eviction_emits_telemetry() {
    let capture = LogCapture::new();
    // moka runs the eviction listener on its own background task / worker thread,
    // so a thread-local subscriber (`set_default`) won't see those events. This
    // test owns its own test binary, so we can safely install a process-global
    // subscriber.
    let subscriber = Registry::default().with(tracing_subscriber::fmt::layer().with_writer(capture.clone()).with_ansi(false));
    tracing::subscriber::set_global_default(subscriber).expect("no other global subscriber should be installed in this test binary");

    let clock = Clock::new_tokio();
    let cache: Cache<String, i32> = Cache::builder::<String, i32>(clock)
        .name("eviction-test")
        .enable_logs()
        .with_eviction_telemetry()
        .memory_with(|b| b.max_capacity(2))
        .build();

    // Drive enough churn to force size-based evictions. Moka's housekeeping
    // runs periodically (and as a side effect of cache operations), so we keep
    // exercising the cache while waiting for an eviction event to surface.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut i: i32 = 0;
    while std::time::Instant::now() < deadline {
        for _ in 0..256 {
            cache.insert(format!("k{i}"), CacheEntry::new(i)).await.unwrap();
            i += 1;
        }
        if capture.output().contains(cachet::telemetry::attributes::EVENT_EVICTION) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "expected `{}` event after exceeding max_capacity; captured output:\n{}",
        cachet::telemetry::attributes::EVENT_EVICTION,
        capture.output()
    );
}
