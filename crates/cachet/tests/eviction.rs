// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for eviction telemetry emitted by the in-memory tier's
//! eviction listener.

#![cfg(feature = "memory")]

// The capture bridge asserts the fallback was installed at process start, so install it
// here. Integration binaries do not run the crate-root `#[cfg(test)]` constructor. See
// docs/tracing-tests.md.
#[ctor::ctor(unsafe)]
fn init_test_tracing() {
    testing_aids::tracing::initialize();
}

use std::time::Duration;

use cachet::{Cache, CacheEntry};
use serial_test::serial;
use testing_aids::TEST_TIMEOUT;
use testing_aids::tracing::write_to_stdout_and_buffer;
use tick::Clock;

/// Inserting past the configured `max_capacity` of the underlying moka cache
/// must eventually emit a `cache.eviction` event for the size-based removals.
#[cfg_attr(miri, ignore)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn memory_size_eviction_emits_telemetry() {
    // moka runs the eviction listener on its own background task / worker thread,
    // so a thread-local subscriber (`set_default`) won't see those events. The
    // global capture bridge routes every thread's events into one buffer; this
    // test owns its own test binary, so process-global capture is safe here.
    let capture = write_to_stdout_and_buffer();

    let clock = Clock::new_tokio();
    let cache: Cache<String, i32> = Cache::builder::<String, i32>(clock)
        .name("eviction-test")
        .enable_logs()
        .memory_with(|b| b.max_capacity(2).with_eviction_telemetry())
        .build();

    // Drive enough churn to force size-based evictions. Moka's housekeeping
    // runs periodically (and as a side effect of cache operations), so we keep
    // exercising the cache while waiting for an eviction event to surface.
    let deadline = std::time::Instant::now() + TEST_TIMEOUT;
    let mut i: i32 = 0;
    while std::time::Instant::now() < deadline {
        for _ in 0..256 {
            cache.insert(format!("k{i}"), CacheEntry::new(i)).await.unwrap();
            i += 1;
        }
        if capture
            .snapshot()
            .iter()
            .any(|line| line.contains(cachet::telemetry::attributes::EVENT_EVICTION))
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "expected `{}` event after exceeding max_capacity; captured output:\n{}",
        cachet::telemetry::attributes::EVENT_EVICTION,
        capture.snapshot().join("\n")
    );
}
