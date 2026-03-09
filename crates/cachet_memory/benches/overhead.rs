// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Benchmarks comparing raw moka performance against `InMemoryCache` wrapper overhead.

#![allow(missing_docs, reason = "Benchmark code")]

use std::{hint::black_box, sync::Arc, time::Instant};

use cachet_memory::InMemoryCache;
use cachet_tier::{CacheEntry, CacheTier};
use criterion::{Criterion, criterion_group, criterion_main};
use moka::future::Cache as MokaCache;
use tokio::runtime::Runtime;

fn rt() -> Runtime {
    Runtime::new().expect("failed to create runtime")
}

fn bench_get_hit(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("get_hit");

    group.bench_function("moka", |b| {
        let cache: MokaCache<String, String> = MokaCache::builder().max_capacity(10_000).build();
        rt.block_on(async {
            for i in 0..1000 {
                cache.insert(format!("key_{i}"), format!("value_{i}")).await;
            }
        });

        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for i in 0..iters {
                    let key = format!("key_{}", i % 1000);
                    let _ = black_box(cache.get(&key).await);
                }
                start.elapsed()
            })
        });
    });

    group.bench_function("cachet_memory", |b| {
        let cache = Arc::new(InMemoryCache::<String, String>::new());
        rt.block_on(async {
            for i in 0..1000 {
                let _ = cache
                    .insert(&format!("key_{i}"), CacheEntry::new(format!("value_{i}")))
                    .await;
            }
        });

        b.iter_custom(|iters| {
            let cache = Arc::clone(&cache);
            rt.block_on(async move {
                let start = Instant::now();
                for i in 0..iters {
                    let key = format!("key_{}", i % 1000);
                    let _ = black_box(cache.get(&key).await);
                }
                start.elapsed()
            })
        });
    });

    group.finish();
}

fn bench_get_miss(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("get_miss");

    group.bench_function("moka", |b| {
        let cache: MokaCache<String, String> = MokaCache::builder().max_capacity(10_000).build();

        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for i in 0..iters {
                    let key = format!("missing_{i}");
                    let _ = black_box(cache.get(&key).await);
                }
                start.elapsed()
            })
        });
    });

    group.bench_function("cachet_memory", |b| {
        let cache = Arc::new(InMemoryCache::<String, String>::new());

        b.iter_custom(|iters| {
            let cache = Arc::clone(&cache);
            rt.block_on(async move {
                let start = Instant::now();
                for i in 0..iters {
                    let key = format!("missing_{i}");
                    let _ = black_box(cache.get(&key).await);
                }
                start.elapsed()
            })
        });
    });

    group.finish();
}

fn bench_insert(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("insert");

    group.bench_function("moka", |b| {
        let cache: MokaCache<String, String> = MokaCache::builder().max_capacity(10_000).build();

        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for i in 0..iters {
                    cache.insert(format!("key_{i}"), format!("value_{i}")).await;
                }
                start.elapsed()
            })
        });
    });

    group.bench_function("cachet_memory", |b| {
        let cache = Arc::new(InMemoryCache::<String, String>::new());

        b.iter_custom(|iters| {
            let cache = Arc::clone(&cache);
            rt.block_on(async move {
                let start = Instant::now();
                for i in 0..iters {
                    let _ = cache
                        .insert(&format!("key_{i}"), CacheEntry::new(format!("value_{i}")))
                        .await;
                }
                start.elapsed()
            })
        });
    });

    group.finish();
}

criterion_group!(benches, bench_get_hit, bench_get_miss, bench_insert);
criterion_main!(benches);
