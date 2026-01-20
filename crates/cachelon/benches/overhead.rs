// Copyright (c) Microsoft Corporation.

//! Benchmark suite measuring the overhead of various cache wrapper layers.

#![allow(missing_docs)]

use std::{hint::black_box, time::Instant};

use alloc_tracker::{Allocator, Session};
use cachelon::{Cache, CacheEntry, CacheTelemetry, CacheTier};
use cachelon_tier::testing::MockCache;
use criterion::{Criterion, criterion_group, criterion_main};
use opentelemetry_sdk::{logs::SdkLoggerProvider, metrics::SdkMeterProvider};
use tick::Clock;
use tokio::runtime::Runtime;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn create_runtime() -> Runtime {
    Runtime::new().unwrap()
}

fn setup_telemetry(clock: Clock) -> CacheTelemetry {
    let logger_provider = SdkLoggerProvider::builder().build();
    let meter_provider = SdkMeterProvider::builder().build();
    CacheTelemetry::new(logger_provider, &meter_provider, clock)
}

fn bench_pure_overhead(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("pure_overhead");
    let session = Session::new();

    let noop_get_name = "noop_get";
    group.bench_function(noop_get_name, |b| {
        b.iter_custom(|iters| {
            let noop_operation = session.operation(noop_get_name);
            rt.block_on(async {
                let noop_cache: MockCache<String, String> = MockCache::new();

                let _span = noop_operation.measure_thread();
                let key = "key".to_string();
                let start = Instant::now();
                for _ in 0..iters {
                    let _: Option<CacheEntry<String>> = black_box(noop_cache.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    let noop_wrapped_get_name = "noop_wrapped_get";
    group.bench_function(noop_wrapped_get_name, |b| {
        b.iter_custom(|iters| {
            let noop_wrapped_operation = session.operation(noop_wrapped_get_name);
            rt.block_on(async move {
                let clock = Clock::new_tokio();
                let noop_wrapped = Cache::builder(clock).storage(MockCache::<String, String>::new()).build();

                let _span = noop_wrapped_operation.measure_thread();
                let key = "key".to_string();
                let start = Instant::now();
                for _ in 0..iters {
                    let _: Option<CacheEntry<String>> = black_box(noop_wrapped.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    let noop_with_telemetry_get_name = "noop_with_telemetry_get (no destination)";
    group.bench_function(noop_with_telemetry_get_name, |b| {
        b.iter_custom(|iters| {
            let noop_with_telemetry_operation = session.operation(noop_with_telemetry_get_name);
            rt.block_on(async move {
                let clock = Clock::new_tokio();
                let cachelon_telemetry = setup_telemetry(clock.clone());
                let noop_with_telemetry = Cache::builder(clock)
                    .storage(MockCache::<String, String>::new())
                    .telemetry(cachelon_telemetry, "noop-telemetry")
                    .build();

                let _span = noop_with_telemetry_operation.measure_thread();
                let key = "key".to_string();
                let start = Instant::now();
                for _ in 0..iters {
                    let _: Option<CacheEntry<String>> = black_box(noop_with_telemetry.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    let noop_fallback_get_name = "noop_fallback_get";
    group.bench_function(noop_fallback_get_name, |b| {
        b.iter_custom(|iters| {
            let noop_fallback_operation = session.operation(noop_fallback_get_name);
            rt.block_on(async move {
                let clock = Clock::new_tokio();
                let noop_fallback = Cache::builder(clock.clone())
                    .storage(MockCache::<String, String>::new())
                    .fallback(Cache::builder(clock).storage(MockCache::<String, String>::new()))
                    .build();

                let _span = noop_fallback_operation.measure_thread();
                let key = "key".to_string();
                let start = Instant::now();
                for _ in 0..iters {
                    let _: Option<CacheEntry<String>> = black_box(noop_fallback.get(black_box(&key)).await);
                }
                start.elapsed()
            })
        });
    });

    group.finish();

    println!("{session}");
}

criterion_group!(benches, bench_pure_overhead);
criterion_main!(benches);
