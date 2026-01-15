//! Builder overhead benchmark: InMemoryCache vs CacheBuilder
//!
//! Compares the runtime overhead of using InMemoryCache directly versus
//! wrapping it with the Cache builder API (CacheBuilder with .memory()).
//!
//! This benchmark measures whether the builder abstraction adds measurable
//! overhead to basic cache operations.

#![allow(missing_docs)]

use std::{hint::black_box, time::Instant};

use alloc_tracker::{Allocator, Session};
use cachelon::{Cache, CacheEntry, CacheTier};
use cachelon_memory::InMemoryCache;
use criterion::{Criterion, criterion_group, criterion_main};
use tick::Clock;
use tokio::runtime::Runtime;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn create_runtime() -> Runtime {
    Runtime::new().unwrap()
}

// =============================================================================
// Benchmark: Get Operations
// =============================================================================

fn bench_get_operations(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("get_operations");
    let session = Session::new();

    // Benchmark: InMemoryCache direct
    let direct_get_name = "inmemorycachelon_direct_get";
    group.bench_function(direct_get_name, |b| {
        b.iter_custom(|iters| {
            let op = session.operation(direct_get_name);
            rt.block_on(async {
                // Build cache directly
                let cache = InMemoryCache::<String, String>::new();

                // Pre-populate
                for i in 0..1000 {
                    let key = format!("key_{i}");
                    let value = format!("value_{i}");
                    cache.insert(&key, CacheEntry::new(value)).await;
                }

                let _span = op.measure_thread();
                let start = Instant::now();
                for i in 0..iters {
                    let key = format!("key_{}", i % 1000);
                    let _ = black_box(cache.get(&key).await);
                }
                start.elapsed()
            })
        });
    });

    // Benchmark: CacheBuilder with .memory()
    let builder_get_name = "cachebuilder_memory_get";
    group.bench_function(builder_get_name, |b| {
        b.iter_custom(|iters| {
            let op = session.operation(builder_get_name);
            rt.block_on(async move {
                // Build cache via builder
                let clock = Clock::new_tokio();
                let cache = Cache::builder::<String, String>(clock).memory().build();

                // Pre-populate
                for i in 0..1000 {
                    let key = format!("key_{i}");
                    let value = format!("value_{i}");
                    cache.insert(&key, CacheEntry::new(value)).await;
                }

                let _span = op.measure_thread();
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
    println!("\n{session}");
}

// =============================================================================
// Benchmark: Insert Operations
// =============================================================================

fn bench_insert_operations(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("insert_operations");
    let session = Session::new();

    // Benchmark: InMemoryCache direct
    let direct_insert_name = "inmemorycachelon_direct_insert";
    group.bench_function(direct_insert_name, |b| {
        b.iter_custom(|iters| {
            let op = session.operation(direct_insert_name);
            rt.block_on(async {
                // Build cache directly
                let cache = InMemoryCache::<String, String>::new();

                let _span = op.measure_thread();
                let start = Instant::now();
                for i in 0..iters {
                    let key = format!("bench_key_{i}");
                    let value = format!("bench_value_{i}");
                    cache.insert(&key, CacheEntry::new(value)).await;
                }
                start.elapsed()
            })
        });
    });

    // Benchmark: CacheBuilder with .memory()
    let builder_insert_name = "cachebuilder_memory_insert";
    group.bench_function(builder_insert_name, |b| {
        b.iter_custom(|iters| {
            let op = session.operation(builder_insert_name);
            rt.block_on(async move {
                // Build cache via builder
                let clock = Clock::new_tokio();
                let cache = Cache::builder::<String, String>(clock).memory().build();

                let _span = op.measure_thread();
                let start = Instant::now();
                for i in 0..iters {
                    let key = format!("bench_key_{i}");
                    let value = format!("bench_value_{i}");
                    cache.insert(&key, CacheEntry::new(value)).await;
                }
                start.elapsed()
            })
        });
    });

    group.finish();
    println!("\n{session}");
}

criterion_group!(benches, bench_get_operations, bench_insert_operations);
criterion_main!(benches);
