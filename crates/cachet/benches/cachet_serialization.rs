// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serialization through public cache operations with no process or network I/O.

#![allow(missing_docs, clippy::unwrap_used, reason = "benchmark code")]

use std::hint::black_box;

use alloc_tracker::{Allocator, Session};
use benchmarking::time_sample;
use bytesbuf::BytesView;
use bytesbuf::mem::{GlobalPool, OpaqueMemory};
use cachet::{Cache, CacheEntry, CacheTier, Error, InsertOutcome};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

/// Forces serialization at the fallback boundary without accumulating primary entries.
struct MissTier;

impl CacheTier<String, String> for MissTier {
    async fn get(&self, _key: &String) -> Result<Option<CacheEntry<String>>, Error> {
        Ok(None)
    }

    async fn insert(&self, key: String, entry: CacheEntry<String>) -> Result<InsertOutcome, Error> {
        black_box((key, entry));
        Ok(InsertOutcome::Accepted)
    }

    async fn invalidate(&self, _key: &String) -> Result<(), Error> {
        Ok(())
    }

    async fn clear(&self) -> Result<(), Error> {
        Ok(())
    }
}

/// Keeps cache construction outside measurements and replaces one serialized entry.
fn serialization(c: &mut Criterion) {
    let session = Session::new();
    let mut group = c.benchmark_group("cachet_serialization/steady_state");
    // Match HTTP benchmark sizes to expose fixed costs and larger payload copying.
    for size in [64, 64 * 1024] {
        let clock = Clock::new_frozen();
        let cache = Cache::builder(clock.clone())
            .storage(MissTier)
            .serialize(OpaqueMemory::new(GlobalPool::new()))
            .fallback(Cache::builder::<BytesView, BytesView>(clock).memory())
            .build();
        let key = String::from("key");
        let payload = "x".repeat(size);
        block_on(cache.insert(key.clone(), payload.clone())).unwrap();

        let allocations = session.operation(format!("insert/{size}"));
        group.bench_with_input(BenchmarkId::new("insert", size), &size, |b, _| {
            b.iter_custom(|iters| {
                let _measurement = allocations.measure_thread().iterations(iters);
                time_sample(iters, || {
                    block_on(cache.insert(black_box(key.clone()), black_box(payload.clone()))).unwrap()
                })
            });
        });
        let allocations = session.operation(format!("get/{size}"));
        group.bench_with_input(BenchmarkId::new("get", size), &size, |b, _| {
            b.iter_custom(|iters| {
                let _measurement = allocations.measure_thread().iterations(iters);
                time_sample(iters, || block_on(cache.get(black_box(&key))).unwrap().unwrap())
            });
        });
    }
    group.finish();
}

criterion_group!(benches, serialization);
criterion_main!(benches);
