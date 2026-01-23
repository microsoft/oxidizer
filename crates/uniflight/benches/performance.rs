// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Performance benchmarks for uniflight.
//!
//! Run with: cargo bench -p uniflight
//! Save baseline: cargo bench -p uniflight -- --save-baseline main
//! Compare to baseline: cargo bench -p uniflight -- --baseline main

#![allow(missing_docs, reason = "benchmark code")]

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use criterion::{Criterion, criterion_group, criterion_main};
use uniflight::Merger;

static KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_key() -> String {
    format!("key_{}", KEY_COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Baseline: single call, no contention.
/// This measures the fixed overhead of the merger.
fn bench_single_call(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");
    let merger = Arc::new(Merger::<String, String>::new());

    c.bench_function("single_call", |b| {
        b.to_async(&rt).iter(|| {
            let merger = Arc::clone(&merger);
            async move { merger.execute(&unique_key(), || async { "value".to_string() }).await }
        });
    });
}

/// Stress test: 100 concurrent tasks on the same key.
/// This hammers the synchronization primitives.
fn bench_high_contention(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");
    let merger = Arc::new(Merger::<String, String>::new());

    c.bench_function("high_contention_100", |b| {
        b.to_async(&rt).iter(|| {
            let merger = Arc::clone(&merger);
            async move {
                let key = unique_key();
                let tasks: Vec<_> = (0..100)
                    .map(|_| {
                        let merger = Arc::clone(&merger);
                        let key = key.clone();
                        tokio::spawn(async move { merger.execute(&key, || async { "value".to_string() }).await })
                    })
                    .collect();

                for task in tasks {
                    task.await.expect("Task panicked");
                }
            }
        });
    });
}

/// Distributed load: 10 keys with 10 concurrent tasks each.
/// This exercises the hash map under concurrent access.
fn bench_distributed_keys(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");
    let merger = Arc::new(Merger::<String, String>::new());

    c.bench_function("distributed_10x10", |b| {
        b.to_async(&rt).iter(|| {
            let merger = Arc::clone(&merger);
            async move {
                let prefix = KEY_COUNTER.fetch_add(1, Ordering::Relaxed);
                let tasks: Vec<_> = (0..10)
                    .flat_map(|key_id| {
                        let merger = Arc::clone(&merger);
                        (0..10).map(move |_| {
                            let merger = Arc::clone(&merger);
                            let key = format!("key_{prefix}_{key_id}");
                            tokio::spawn(async move { merger.execute(&key, || async { "value".to_string() }).await })
                        })
                    })
                    .collect();

                for task in tasks {
                    task.await.expect("Task panicked");
                }
            }
        });
    });
}

criterion_group!(benches, bench_single_call, bench_high_contention, bench_distributed_keys,);

criterion_main!(benches);
