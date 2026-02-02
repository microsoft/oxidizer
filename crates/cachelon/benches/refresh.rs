// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Benchmarks for time-to-refresh overhead.

#![allow(missing_docs, reason = "Benchmark code")]

use std::{collections::HashMap, hint::black_box, time::Duration, time::Instant};

use cachelon::{Cache, CacheEntry, FallbackPromotionPolicy, refresh::TimeToRefresh};
use cachelon_tier::testing::MockCache;
use criterion::{Criterion, criterion_group, criterion_main};
use tick::Clock;
use tokio::runtime::Runtime;

fn rt() -> Runtime {
    Runtime::new().expect("failed to create runtime")
}

fn mock_with_value(key: &str, value: &str) -> MockCache<String, String> {
    let mut data = HashMap::new();
    data.insert(key.to_string(), CacheEntry::new(value.to_string()));
    MockCache::with_data(data)
}

fn bench_refresh_overhead(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("refresh_overhead");

    // Baseline: fallback without refresh
    group.bench_function("without_refresh", |b| {
        let cache = rt.block_on(async {
            let clock = Clock::new_tokio();
            Cache::builder(clock.clone())
                .storage(mock_with_value("key", "value"))
                .fallback(Cache::builder(clock).storage(MockCache::<String, String>::new()))
                .promotion_policy(FallbackPromotionPolicy::Always)
                .build()
        });
        let key = "key".to_string();

        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for _ in 0..iters {
                    let _ = black_box(cache.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    // With refresh enabled but not triggering (TTR > entry age)
    group.bench_function("with_refresh_not_triggering", |b| {
        let cache = rt.block_on(async {
            let clock = Clock::new_tokio();
            Cache::builder(clock.clone())
                .storage(mock_with_value("key", "value"))
                .fallback(Cache::builder(clock).storage(MockCache::<String, String>::new()))
                .time_to_refresh(TimeToRefresh::new_tokio(Duration::from_secs(3600)))
                .promotion_policy(FallbackPromotionPolicy::Always)
                .build()
        });
        let key = "key".to_string();

        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for _ in 0..iters {
                    let _ = black_box(cache.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    // With refresh triggering (TTR = 0, always triggers)
    group.bench_function("with_refresh_triggering", |b| {
        let cache = rt.block_on(async {
            let clock = Clock::new_tokio();
            Cache::builder(clock.clone())
                .storage(mock_with_value("key", "value"))
                .fallback(Cache::builder(clock).storage(mock_with_value("key", "refreshed")))
                .time_to_refresh(TimeToRefresh::new_tokio(Duration::from_secs(0)))
                .promotion_policy(FallbackPromotionPolicy::Always)
                .build()
        });
        let key = "key".to_string();

        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for _ in 0..iters {
                    let _ = black_box(cache.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    group.finish();
}

criterion_group!(benches, bench_refresh_overhead);
criterion_main!(benches);
