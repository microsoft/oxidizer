// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Benchmarks for `DynamicCache` with allocation tracking.
//!
//! `DynamicCache` uses boxing for type erasure, so we track both time and allocations.

#![allow(missing_docs, reason = "Benchmark code")]

use std::hint::black_box;

use alloc_tracker::{Allocator, Session};
use benchmarking::time_sample_async;
use cachet::{Cache, CacheEntry};
use cachet_tier::{DynamicCache, MockCache};
use criterion::{Criterion, criterion_group, criterion_main};
use tick::Clock;
use tokio::runtime::Runtime;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn rt() -> Runtime {
    Runtime::new().expect("failed to create runtime")
}

fn bench_dynamic_cache(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("dynamic_cache");
    let session = Session::new();

    // Baseline: MockCache wrapped normally (no dynamic dispatch)
    let static_get_name = "static_get";
    group.bench_function(static_get_name, |b| {
        let cache = rt.block_on(async {
            let clock = Clock::new_tokio();
            Cache::builder(clock).storage(MockCache::<String, String>::new()).build()
        });
        let key = "key".to_string();

        b.iter_custom(|iters| {
            let op = session.operation(static_get_name);
            rt.block_on(async {
                let _span = op.measure_thread().iterations(iters);
                time_sample_async(iters, |_| cache.get(black_box(&key))).await
            })
        });
    });

    // DynamicCache (with boxing overhead)
    let dynamic_get_name = "dynamic_get";
    group.bench_function(dynamic_get_name, |b| {
        let cache = rt.block_on(async {
            let clock = Clock::new_tokio();
            let mock = MockCache::<String, String>::new();
            let dynamic: DynamicCache<String, String> = DynamicCache::new(mock);
            Cache::builder(clock).storage(dynamic).build()
        });
        let key = "key".to_string();

        b.iter_custom(|iters| {
            let op = session.operation(dynamic_get_name);
            rt.block_on(async {
                let _span = op.measure_thread().iterations(iters);
                time_sample_async(iters, |_| cache.get(black_box(&key))).await
            })
        });
    });

    // Static insert
    let static_insert_name = "static_insert";
    group.bench_function(static_insert_name, |b| {
        let cache = rt.block_on(async {
            let clock = Clock::new_tokio();
            Cache::builder(clock).storage(MockCache::<String, String>::new()).build()
        });

        b.iter_custom(|iters| {
            let op = session.operation(static_insert_name);
            rt.block_on(async {
                let _span = op.measure_thread().iterations(iters);
                time_sample_async(iters, |iteration| {
                    cache.insert(format!("key_{iteration}"), CacheEntry::new(format!("value_{iteration}")))
                })
                .await
            })
        });
    });

    // Dynamic insert
    let dynamic_insert_name = "dynamic_insert";
    group.bench_function(dynamic_insert_name, |b| {
        let cache = rt.block_on(async {
            let clock = Clock::new_tokio();
            let mock = MockCache::<String, String>::new();
            let dynamic: DynamicCache<String, String> = DynamicCache::new(mock);
            Cache::builder(clock).storage(dynamic).build()
        });

        b.iter_custom(|iters| {
            let op = session.operation(dynamic_insert_name);
            rt.block_on(async {
                let _span = op.measure_thread().iterations(iters);
                time_sample_async(iters, |iteration| {
                    cache.insert(format!("key_{iteration}"), CacheEntry::new(format!("value_{iteration}")))
                })
                .await
            })
        });
    });

    group.finish();
}

criterion_group!(benches, bench_dynamic_cache);
criterion_main!(benches);
