// Copyright (c) Microsoft Corporation.

//! Benchmarks for measuring cache refresh overhead and benefits.

#![allow(missing_docs)]

use std::{
    hint::black_box,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use alloc_tracker::{Allocator, Session};
use cachelon::{Cache, CacheEntry, CacheTier, Error, refresh::TimeToRefresh};
use cachelon_tier::testing::MockCache;
use criterion::{Criterion, criterion_group, criterion_main};
use tick::{Clock, Delay};
use tokio::runtime::Runtime;

/// Creates a MockCache pre-populated with a value for the given key.
fn mock_with_value<K, V>(key: K, entry: CacheEntry<V>) -> MockCache<K, V>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    let mut data = std::collections::HashMap::new();
    data.insert(key, entry);
    MockCache::with_data(data)
}

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn create_runtime() -> Runtime {
    Runtime::new().unwrap()
}

fn bench_refresh_overhead(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("refresh_overhead");
    let session = Session::new();

    let fixed_get_name = "fixed_get";
    group.bench_function(fixed_get_name, |b| {
        b.iter_custom(|iters| {
            let fixed_operation = session.operation(fixed_get_name);
            rt.block_on(async move {
                let clock = Clock::new_tokio();
                let fixed = Cache::builder(clock.clone())
                    .storage(mock_with_value("key".to_string(), CacheEntry::new("blah".to_string())))
                    .fallback(Cache::builder(clock.clone()).storage(MockCache::<String, String>::new()))
                    .build();

                let _span = fixed_operation.measure_thread();
                let key = "key".to_string();
                let start = Instant::now();
                for _ in 0..iters {
                    let _: Option<CacheEntry<String>> = black_box(fixed.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    let fixed_with_refresh_get_name = "fixed_with_refresh_get_with_refresh_task";
    group.bench_function(fixed_with_refresh_get_name, |b| {
        b.iter_custom(|iters| {
            let fixed_with_refresh_operation = session.operation(fixed_with_refresh_get_name);
            rt.block_on(async move {
                let clock = Clock::new_tokio();
                let value = CacheEntry::new("blah".to_string());
                let fixed_with_refresh = Cache::builder(clock.clone())
                    .storage(mock_with_value("key".to_string(), value.clone()))
                    .fallback(Cache::builder(clock.clone()).storage(mock_with_value("key".to_string(), value)))
                    .time_to_refresh(TimeToRefresh::new_tokio(Duration::from_secs(0)))
                    .build();

                let _span = fixed_with_refresh_operation.measure_thread();
                let key = "key".to_string();
                let start = Instant::now();
                for _ in 0..iters {
                    let _: Option<CacheEntry<String>> = black_box(fixed_with_refresh.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    let fixed_with_refresh_get_name = "fixed_with_refresh_get_no_refresh_task";
    group.bench_function(fixed_with_refresh_get_name, |b| {
        b.iter_custom(|iters| {
            let fixed_with_refresh_operation = session.operation(fixed_with_refresh_get_name);
            rt.block_on(async move {
                let clock = Clock::new_tokio();
                let value = CacheEntry::new("blah".to_string());
                let fixed_with_refresh = Cache::builder(clock.clone())
                    .storage(mock_with_value("key".to_string(), value.clone()))
                    .fallback(Cache::builder(clock.clone()).storage(mock_with_value("key".to_string(), value)))
                    .time_to_refresh(TimeToRefresh::new_tokio(Duration::from_secs(10)))
                    .build();

                let _span = fixed_with_refresh_operation.measure_thread();
                let key = "key".to_string();
                let start = Instant::now();
                for _ in 0..iters {
                    let _: Option<CacheEntry<String>> = black_box(fixed_with_refresh.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    group.finish();

    println!("{session}");
}

/// A shared counter for tracking I/O calls.
#[derive(Debug, Clone, Default)]
struct IoCounter(Arc<AtomicU64>);

impl IoCounter {
    fn new() -> Self {
        Self(Arc::new(AtomicU64::new(0)))
    }

    fn increment(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// A cache tier that simulates slow I/O by sleeping before returning a value.
struct SlowCache<V> {
    value: V,
    latency: Duration,
    clock: Clock,
    io_counter: IoCounter,
}

impl<V> std::fmt::Debug for SlowCache<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlowCache")
            .field("latency", &self.latency)
            .field("io_count", &self.io_counter.get())
            .finish_non_exhaustive()
    }
}

impl<V: Clone> SlowCache<V> {
    fn new(value: V, latency: Duration, clock: Clock, io_counter: IoCounter) -> Self {
        Self {
            value,
            latency,
            clock,
            io_counter,
        }
    }
}

impl<K: Send + Sync, V: Clone + Send + Sync> CacheTier<K, V> for SlowCache<CacheEntry<V>> {
    async fn get(&self, _key: &K) -> Option<CacheEntry<V>> {
        self.io_counter.increment();
        Delay::new(&self.clock, self.latency).await;
        Some(self.value.clone())
    }

    async fn try_get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        Ok(self.get(key).await)
    }

    async fn insert(&self, _key: &K, _value: CacheEntry<V>) {}
    async fn try_insert(&self, _key: &K, _value: CacheEntry<V>) -> Result<(), Error> {
        Ok(())
    }
    async fn invalidate(&self, _key: &K) {}
    async fn try_invalidate(&self, _key: &K) -> Result<(), Error> {
        Ok(())
    }
}

/// Benchmarks showing the benefit of background refresh with simulated I/O.
///
/// Scenario: Cache with simulated I/O latency on the fallback tier.
/// - Without refresh: When cache is stale, the get blocks for the I/O latency
/// - With refresh: Get returns immediately from stale cache, refresh happens in background
fn bench_refresh_benefit(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("refresh_benefit");
    let session = Session::new();

    // Use 100μs latency - small enough to run many iterations, large enough to measure
    let io_latency = Duration::from_micros(100);

    // Counters to track I/O calls and total iterations
    let no_refresh_io_counter = IoCounter::new();
    let with_refresh_io_counter = IoCounter::new();
    let no_refresh_iters = IoCounter::new();
    let with_refresh_iters = IoCounter::new();

    // Benchmark: No refresh - must wait for slow I/O on cache miss
    let no_refresh_name = "no_refresh_cachelon_miss";
    let no_refresh_counter = no_refresh_io_counter.clone();
    let no_refresh_iters_counter = no_refresh_iters.clone();
    group.bench_function(no_refresh_name, |b| {
        b.iter_custom(|iters| {
            no_refresh_iters_counter.0.fetch_add(iters, Ordering::Relaxed);
            let no_refresh_operation = session.operation(no_refresh_name);
            let counter = no_refresh_counter.clone();
            rt.block_on(async move {
                let clock = Clock::new_tokio();
                let slow_fallback = SlowCache::new(
                    CacheEntry::with_cached_at("value", clock.instant()),
                    io_latency,
                    clock.clone(),
                    counter,
                );

                let cache = Cache::builder(clock.clone())
                    .storage(MockCache::<&'static str, &'static str>::new()) // Primary always misses
                    .fallback(Cache::builder(clock.clone()).storage(slow_fallback))
                    .build();

                let _span = no_refresh_operation.measure_thread();
                let key = "key";
                let start = Instant::now();
                for _ in 0..iters {
                    let _: Option<CacheEntry<&'static str>> = black_box(cache.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    // Benchmark: With refresh - returns stale value immediately, refreshes in background
    let with_refresh_name = "with_refresh_stale_hit";
    let with_refresh_counter = with_refresh_io_counter.clone();
    let with_refresh_iters_counter = with_refresh_iters.clone();
    group.bench_function(with_refresh_name, |b| {
        b.iter_custom(|iters| {
            with_refresh_iters_counter.0.fetch_add(iters, Ordering::Relaxed);
            let with_refresh_operation = session.operation(with_refresh_name);
            let counter = with_refresh_counter.clone();
            rt.block_on(async move {
                let clock = Clock::new_tokio();
                let slow_fallback = SlowCache::new(
                    CacheEntry::with_cached_at("refreshed_value", clock.instant()),
                    io_latency,
                    clock.clone(),
                    counter.clone(),
                );

                // Primary has a stale value (cached_at in the past so TTR triggers)
                let stale_time = clock.instant().checked_sub(Duration::from_secs(10)).unwrap();
                let cache = Cache::builder(clock.clone())
                    .storage(mock_with_value("key", CacheEntry::with_cached_at("stale_value", stale_time)))
                    .fallback(Cache::builder(clock.clone()).storage(slow_fallback))
                    .time_to_refresh(TimeToRefresh::new_tokio(Duration::from_secs(0)))
                    .build();

                let _span = with_refresh_operation.measure_thread();
                let key = "key";
                let start = Instant::now();
                for _ in 0..iters {
                    // Returns immediately with stale value, refresh happens in background
                    let _: Option<CacheEntry<&'static str>> = black_box(cache.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    group.finish();

    // Wait for background tasks to complete before printing counts
    rt.block_on(async move {
        let clock = Clock::new_tokio();
        for _ in 0..10 {
            Delay::new(&clock, Duration::from_millis(15)).await;
        }
    });

    println!("{session}");
    println!("\nI/O call statistics:");
    println!("  no_refresh_cachelon_miss:");
    println!("    Total iterations: {}", no_refresh_iters.get());
    println!("    Total I/O calls:  {}", no_refresh_io_counter.get());
    #[allow(clippy::cast_precision_loss, reason = "precision loss acceptable for stats")]
    let no_refresh_ratio = no_refresh_io_counter.get() as f64 / no_refresh_iters.get().max(1) as f64;
    println!("    I/O calls/iter:   {no_refresh_ratio:.2}");
    println!("  with_refresh_stale_hit:");
    println!("    Total iterations: {}", with_refresh_iters.get());
    println!("    Total I/O calls:  {}", with_refresh_io_counter.get());
    #[allow(clippy::cast_precision_loss, reason = "precision loss acceptable for stats")]
    let with_refresh_ratio = with_refresh_io_counter.get() as f64 / with_refresh_iters.get().max(1) as f64;
    println!("    I/O calls/iter:   {with_refresh_ratio:.8}");
}

criterion_group!(benches, bench_refresh_overhead, bench_refresh_benefit);
criterion_main!(benches);
