// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Benchmarks for core cache operations and wrapper overhead.

#![allow(missing_docs, reason = "Benchmark code")]

use std::{hint::black_box, sync::Arc, time::Instant};

use cachelon::{Cache, CacheEntry, CacheTelemetry, CacheTier};
use cachelon_memory::InMemoryCache;
use cachelon_tier::testing::MockCache;
use criterion::{Criterion, criterion_group, criterion_main};
use opentelemetry_sdk::{logs::SdkLoggerProvider, metrics::SdkMeterProvider};
use tick::Clock;
use tokio::runtime::Runtime;

fn rt() -> Runtime {
    Runtime::new().expect("failed to create runtime")
}

fn telemetry(clock: Clock) -> CacheTelemetry {
    let logger = SdkLoggerProvider::builder().build();
    let meter = SdkMeterProvider::builder().build();
    CacheTelemetry::new(logger, &meter, clock)
}

// =============================================================================
// Cache Operations (get hit, get miss, insert)
// =============================================================================

fn bench_cache_operations(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("cache_operations");

    group.bench_function("get_hit", |b| {
        let cache = Arc::new(InMemoryCache::<String, String>::new());
        rt.block_on(async {
            for i in 0..1000 {
                cache.insert(&format!("key_{i}"), CacheEntry::new(format!("value_{i}"))).await;
            }
        });

        b.iter_custom(|iters| {
            let cache = Arc::clone(&cache);
            rt.block_on(async move {
                let start = Instant::now();
                for i in 0..iters {
                    let key = format!("key_{}", i % 1000);
                    black_box(cache.get(&key).await);
                }
                start.elapsed()
            })
        });
    });

    group.bench_function("get_miss", |b| {
        let cache = Arc::new(InMemoryCache::<String, String>::new());

        b.iter_custom(|iters| {
            let cache = Arc::clone(&cache);
            rt.block_on(async move {
                let start = Instant::now();
                for i in 0..iters {
                    let key = format!("missing_{i}");
                    black_box(cache.get(&key).await);
                }
                start.elapsed()
            })
        });
    });

    group.bench_function("insert", |b| {
        let cache = Arc::new(InMemoryCache::<String, String>::new());

        b.iter_custom(|iters| {
            let cache = Arc::clone(&cache);
            rt.block_on(async move {
                let start = Instant::now();
                for i in 0..iters {
                    cache.insert(&format!("key_{i}"), CacheEntry::new(format!("value_{i}"))).await;
                }
                start.elapsed()
            })
        });
    });

    group.finish();
}

// =============================================================================
// Wrapper Overhead (direct vs wrapped vs features)
// =============================================================================

fn bench_wrapper_overhead(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("wrapper_overhead");

    // Baseline: MockCache direct
    group.bench_function("direct", |b| {
        let cache = MockCache::<String, String>::new();
        let key = "key".to_string();

        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for _ in 0..iters {
                    black_box(cache.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    // Wrapped in Cache builder
    group.bench_function("wrapped", |b| {
        let cache = rt.block_on(async {
            let clock = Clock::new_tokio();
            Cache::builder(clock).storage(MockCache::<String, String>::new()).build()
        });
        let key = "key".to_string();

        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for _ in 0..iters {
                    black_box(cache.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    // With telemetry
    group.bench_function("with_telemetry", |b| {
        let cache = rt.block_on(async {
            let clock = Clock::new_tokio();
            Cache::builder(clock.clone())
                .storage(MockCache::<String, String>::new())
                .telemetry(telemetry(clock), "bench")
                .build()
        });
        let key = "key".to_string();

        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for _ in 0..iters {
                    black_box(cache.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    // With fallback tier
    group.bench_function("with_fallback", |b| {
        let cache = rt.block_on(async {
            let clock = Clock::new_tokio();
            Cache::builder(clock.clone())
                .storage(MockCache::<String, String>::new())
                .fallback(Cache::builder(clock).storage(MockCache::<String, String>::new()))
                .build()
        });
        let key = "key".to_string();

        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for _ in 0..iters {
                    black_box(cache.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    // With stampede protection
    group.bench_function("with_stampede_protection", |b| {
        let cache = rt.block_on(async {
            let clock = Clock::new_tokio();
            Cache::builder(clock)
                .storage(MockCache::<String, String>::new())
                .stampede_protection()
                .build()
        });
        let key = "key".to_string();

        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for _ in 0..iters {
                    black_box(cache.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    group.finish();
}

criterion_group!(benches, bench_cache_operations, bench_wrapper_overhead);
criterion_main!(benches);
