// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Benchmarks comparing uniflight against singleflight-async.

#![allow(
    clippy::items_after_statements,
    clippy::unwrap_used,
    missing_docs,
    reason = "Benchmarks have relaxed requirements"
)]

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use criterion::{Criterion, criterion_group, criterion_main};

// Benchmark 1: Single call (no contention)
fn bench_single_call(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("single_call");

    // Use atomic counter for unique keys
    static COUNTER1: AtomicU64 = AtomicU64::new(0);

    // Our implementation - pre-create the merger
    let our_merger = Arc::new(uniflight::Merger::<String, String>::new());
    group.bench_function("uniflight", |b| {
        b.to_async(&rt).iter(|| {
            let merger = Arc::clone(&our_merger);
            async move {
                let key = format!("key_{}", COUNTER1.fetch_add(1, Ordering::Relaxed));
                merger.execute(&key, || async { "value".to_string() }).await
            }
        });
    });

    // singleflight-async - pre-create the group
    let their_group = Arc::new(singleflight_async::SingleFlight::<String, String>::new());
    group.bench_function("singleflight_async", |b| {
        b.to_async(&rt).iter(|| {
            let group = Arc::clone(&their_group);
            async move {
                let key = format!("key_{}", COUNTER1.fetch_add(1, Ordering::Relaxed));
                group.work(key, || async { "value".to_string() }).await
            }
        });
    });

    group.finish();
}

// Benchmark 2: Concurrent calls (10 tasks, same key)
fn bench_concurrent_10(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("concurrent_10_tasks");

    // Use atomic counter for unique keys per iteration
    static COUNTER2: AtomicU64 = AtomicU64::new(0);

    // Our implementation - pre-create the merger
    let our_merger = Arc::new(uniflight::Merger::<String, String>::new());
    group.bench_function("uniflight", |b| {
        b.to_async(&rt).iter(|| {
            let merger = Arc::clone(&our_merger);
            async move {
                let key = format!("key_{}", COUNTER2.fetch_add(1, Ordering::Relaxed));
                let handles: Vec<_> = (0..10)
                    .map(|_| {
                        let merger = Arc::clone(&merger);
                        let key = key.clone();
                        tokio::spawn(async move { merger.execute(&key, || async { "value".to_string() }).await })
                    })
                    .collect();

                for handle in handles {
                    handle.await.unwrap();
                }
            }
        });
    });

    // singleflight-async - pre-create the group
    let their_group = Arc::new(singleflight_async::SingleFlight::<String, String>::new());
    group.bench_function("singleflight_async", |b| {
        b.to_async(&rt).iter(|| {
            let group = Arc::clone(&their_group);
            async move {
                let key = format!("key_{}", COUNTER2.fetch_add(1, Ordering::Relaxed));
                let handles: Vec<_> = (0..10)
                    .map(|_| {
                        let group = Arc::clone(&group);
                        let key = key.clone();
                        tokio::spawn(async move { group.work(key, || async { "value".to_string() }).await })
                    })
                    .collect();

                for handle in handles {
                    handle.await.unwrap();
                }
            }
        });
    });

    group.finish();
}

// Benchmark 3: High contention (100 tasks, same key)
fn bench_concurrent_100(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("concurrent_100_tasks");

    // Use atomic counter for unique keys per iteration
    static COUNTER3: AtomicU64 = AtomicU64::new(0);

    // Our implementation - pre-create the merger
    let our_merger = Arc::new(uniflight::Merger::<String, String>::new());
    group.bench_function("uniflight", |b| {
        b.to_async(&rt).iter(|| {
            let merger = Arc::clone(&our_merger);
            async move {
                let key = format!("key_{}", COUNTER3.fetch_add(1, Ordering::Relaxed));
                let handles: Vec<_> = (0..100)
                    .map(|_| {
                        let merger = Arc::clone(&merger);
                        let key = key.clone();
                        tokio::spawn(async move { merger.execute(&key, || async { "value".to_string() }).await })
                    })
                    .collect();

                for handle in handles {
                    handle.await.unwrap();
                }
            }
        });
    });

    // singleflight-async - pre-create the group
    let their_group = Arc::new(singleflight_async::SingleFlight::<String, String>::new());
    group.bench_function("singleflight_async", |b| {
        b.to_async(&rt).iter(|| {
            let group = Arc::clone(&their_group);
            async move {
                let key = format!("key_{}", COUNTER3.fetch_add(1, Ordering::Relaxed));
                let handles: Vec<_> = (0..100)
                    .map(|_| {
                        let group = Arc::clone(&group);
                        let key = key.clone();
                        tokio::spawn(async move { group.work(key, || async { "value".to_string() }).await })
                    })
                    .collect();

                for handle in handles {
                    handle.await.unwrap();
                }
            }
        });
    });

    group.finish();
}

// Benchmark 4: Multiple different keys (10 keys, 10 tasks each)
fn bench_multiple_keys(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("multiple_keys_10x10");

    // Use atomic counter for unique key prefix per iteration
    static COUNTER4: AtomicU64 = AtomicU64::new(0);

    // Our implementation - pre-create the merger
    let our_merger = Arc::new(uniflight::Merger::<String, String>::new());
    group.bench_function("uniflight", |b| {
        b.to_async(&rt).iter(|| {
            let merger = Arc::clone(&our_merger);
            async move {
                let iteration = COUNTER4.fetch_add(1, Ordering::Relaxed);
                let handles: Vec<_> = (0..10)
                    .flat_map(|key_id| {
                        let merger = Arc::clone(&merger);
                        (0..10).map(move |_| {
                            let merger = Arc::clone(&merger);
                            let key = format!("key_{iteration}_{key_id}");
                            tokio::spawn(async move { merger.execute(&key, || async { "value".to_string() }).await })
                        })
                    })
                    .collect();

                for handle in handles {
                    handle.await.unwrap();
                }
            }
        });
    });

    // singleflight-async - pre-create the group
    let their_group = Arc::new(singleflight_async::SingleFlight::<String, String>::new());
    group.bench_function("singleflight_async", |b| {
        b.to_async(&rt).iter(|| {
            let group = Arc::clone(&their_group);
            async move {
                let iteration = COUNTER4.fetch_add(1, Ordering::Relaxed);
                let handles: Vec<_> = (0..10)
                    .flat_map(|key_id| {
                        let group = Arc::clone(&group);
                        (0..10).map(move |_| {
                            let group = Arc::clone(&group);
                            let key = format!("key_{iteration}_{key_id}");
                            tokio::spawn(async move { group.work(key, || async { "value".to_string() }).await })
                        })
                    })
                    .collect();

                for handle in handles {
                    handle.await.unwrap();
                }
            }
        });
    });

    group.finish();
}

// Benchmark 5: Reuse existing group (pre-created, multiple operations)
fn bench_reuse_group(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("reuse_group");

    // Use atomic counter for unique keys
    static COUNTER5: AtomicU64 = AtomicU64::new(0);

    // Our implementation - pre-create the merger
    let our_merger = Arc::new(uniflight::Merger::<String, String>::new());
    group.bench_function("uniflight", |b| {
        b.to_async(&rt).iter(|| {
            let merger = Arc::clone(&our_merger);
            async move {
                // Each iteration uses a unique key to avoid caching effects
                let key = format!("key_{}", COUNTER5.fetch_add(1, Ordering::Relaxed));
                merger.execute(&key, || async { "value".to_string() }).await
            }
        });
    });

    // singleflight-async - pre-create the group
    let their_group = Arc::new(singleflight_async::SingleFlight::<String, String>::new());
    group.bench_function("singleflight_async", |b| {
        b.to_async(&rt).iter(|| {
            let group = Arc::clone(&their_group);
            async move {
                let key = format!("key_{}", COUNTER5.fetch_add(1, Ordering::Relaxed));
                group.work(key, || async { "value".to_string() }).await
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_single_call,
    bench_concurrent_10,
    bench_concurrent_100,
    bench_multiple_keys,
    bench_reuse_group,
);

criterion_main!(benches);
