// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Benchmarks for the NUMA cache.

#![expect(missing_docs, reason = "Benchmark code does not require documentation")]

use std::hint::black_box;
use std::sync::Arc;
use std::thread;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;
use thread_aware::create_manual_pinned_affinities;
use thread_aware_cache::NumaCache;

criterion_group!(benches, bench_basic, bench_concurrent, bench_cross_shard, bench_bloom_filter);
criterion_main!(benches);

const CACHE_CAPACITY: usize = 10_000;
const NUM_SHARDS: usize = 4;
const TOTAL_CAPACITY: usize = CACHE_CAPACITY * NUM_SHARDS;

fn bench_basic(c: &mut Criterion) {
    let mut group = c.benchmark_group("NumaCache");

    // Create affinities for benchmarks
    let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);

    // Benchmark cache creation
    group.bench_function("new", |b| {
        b.iter(|| {
            black_box(
                NumaCache::<String, i32>::builder()
                    .affinities(&affinities)
                    .capacity_per_shard(CACHE_CAPACITY)
                    .build(),
            )
        });
    });

    // Benchmark single-threaded insert
    group.throughput(Throughput::Elements(1));
    group.bench_function("insert_single", |b| {
        let cache = NumaCache::<usize, usize>::builder()
            .affinities(&affinities)
            .capacity_per_shard(CACHE_CAPACITY)
            .build();
        let affinity = affinities[0];

        let mut key = 0usize;
        b.iter(|| {
            cache.insert(affinity, key, key);
            key = (key + 1) % TOTAL_CAPACITY;
        });
    });

    // Benchmark single-threaded get (local hit)
    group.bench_function("get_local_hit", |b| {
        let cache = NumaCache::<usize, usize>::builder()
            .affinities(&affinities)
            .capacity_per_shard(CACHE_CAPACITY)
            .build();
        let affinity = affinities[0];

        // Pre-populate the cache on the same affinity
        for i in 0..CACHE_CAPACITY {
            cache.insert(affinity, i, i);
        }

        let mut rng = StdRng::seed_from_u64(42);
        b.iter(|| {
            let key = rng.random_range(0..CACHE_CAPACITY);
            black_box(cache.get(affinity, &key));
        });
    });

    // Benchmark single-threaded get (miss)
    group.bench_function("get_miss", |b| {
        let cache = NumaCache::<usize, usize>::builder()
            .affinities(&affinities)
            .capacity_per_shard(CACHE_CAPACITY)
            .build();
        let affinity = affinities[0];

        let mut key = TOTAL_CAPACITY; // Start outside the range
        b.iter(|| {
            key += 1;
            black_box(cache.get(affinity, &key));
        });
    });

    // Benchmark eviction
    group.bench_function("insert_with_eviction", |b| {
        let single_affinity = create_manual_pinned_affinities(&[1]);
        b.iter_batched(
            || {
                let cache =
                    NumaCache::<usize, usize>::builder().affinities(&single_affinity).capacity_per_shard(100).build();
                let affinity = single_affinity[0];

                // Fill the cache
                for i in 0..100 {
                    cache.insert(affinity, i, i);
                }
                (cache, affinity)
            },
            |(cache, affinity)| {
                // Insert causing eviction
                cache.insert(affinity, 1000, 1000);
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

#[expect(clippy::too_many_lines, reason = "Benchmark function naturally groups related concurrent benchmarks")]
fn bench_concurrent(c: &mut Criterion) {
    let mut concurrent_group = c.benchmark_group("NumaCache_Concurrent");
    let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);

    for num_threads in [2, 4, 8] {
        concurrent_group.throughput(Throughput::Elements(1000));
        concurrent_group.bench_with_input(
            BenchmarkId::new("concurrent_insert", num_threads),
            &num_threads,
            |b, &num_threads| {
                b.iter(|| {
                    let cache = Arc::new(
                        NumaCache::<usize, usize>::builder()
                            .affinities(&affinities)
                            .capacity_per_shard(CACHE_CAPACITY)
                            .build(),
                    );
                    let affinities = Arc::new(affinities.clone());

                    let handles: Vec<_> = (0..num_threads)
                        .map(|t| {
                            let cache = Arc::clone(&cache);
                            let affinities = Arc::clone(&affinities);
                            thread::spawn(move || {
                                let affinity = affinities[t % affinities.len()];
                                for i in 0usize..1000 {
                                    let key = t * 10000 + i;
                                    cache.insert(affinity, key, key);
                                }
                            })
                        })
                        .collect();

                    for handle in handles {
                        handle.join().expect("thread panicked");
                    }
                });
            },
        );

        concurrent_group.bench_with_input(BenchmarkId::new("concurrent_read_local", num_threads), &num_threads, |b, &num_threads| {
            // Pre-populate the cache - each shard with its own data
            let cache = Arc::new(
                NumaCache::<usize, usize>::builder()
                    .affinities(&affinities)
                    .capacity_per_shard(CACHE_CAPACITY)
                    .build(),
            );
            let affinities = Arc::new(affinities.clone());

            // Each thread's data goes to its assigned shard
            for t in 0..NUM_SHARDS {
                let affinity = affinities[t];
                for i in 0..CACHE_CAPACITY {
                    let key = t * CACHE_CAPACITY + i;
                    cache.insert(affinity, key, key);
                }
            }

            b.iter(|| {
                let handles: Vec<_> = (0..num_threads)
                    .map(|t| {
                        let cache = Arc::clone(&cache);
                        let affinities = Arc::clone(&affinities);
                        thread::spawn(move || {
                            let shard_idx = t % NUM_SHARDS;
                            let affinity = affinities[shard_idx];
                            let mut rng = StdRng::seed_from_u64(u64::try_from(t).unwrap_or(0));
                            for _ in 0..1000 {
                                // Read from local shard's key range
                                let key = shard_idx * CACHE_CAPACITY + rng.random_range(0..CACHE_CAPACITY);
                                black_box(cache.get(affinity, &key));
                            }
                        })
                    })
                    .collect();

                for handle in handles {
                    handle.join().expect("thread panicked");
                }
            });
        });

        concurrent_group.bench_with_input(
            BenchmarkId::new("concurrent_mixed", num_threads),
            &num_threads,
            |b, &num_threads| {
                b.iter(|| {
                    let cache = Arc::new(
                        NumaCache::<usize, usize>::builder()
                            .affinities(&affinities)
                            .capacity_per_shard(CACHE_CAPACITY)
                            .build(),
                    );
                    let affinities = Arc::new(affinities.clone());

                    let handles: Vec<_> = (0..num_threads)
                        .map(|t| {
                            let cache = Arc::clone(&cache);
                            let affinities = Arc::clone(&affinities);
                            thread::spawn(move || {
                                let affinity = affinities[t % affinities.len()];
                                let mut rng = StdRng::seed_from_u64(u64::try_from(t).unwrap_or(0));
                                for i in 0..1000 {
                                    let key = rng.random_range(0usize..10000);
                                    if i % 10 == 0 {
                                        // 10% writes
                                        cache.insert(affinity, key, key);
                                    } else {
                                        // 90% reads
                                        black_box(cache.get(affinity, &key));
                                    }
                                }
                            })
                        })
                        .collect();

                    for handle in handles {
                        handle.join().expect("thread panicked");
                    }
                });
            },
        );
    }

    concurrent_group.finish();
}

/// Benchmark cross-shard access patterns to measure the read-through replication overhead.
fn bench_cross_shard(c: &mut Criterion) {
    let mut group = c.benchmark_group("NumaCache_CrossShard");
    let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);

    // Benchmark cross-shard get (first access - causes replication)
    group.bench_function("get_cross_shard_first", |b| {
        b.iter_batched(
            || {
                let cache = NumaCache::<usize, usize>::builder()
                    .affinities(&affinities)
                    .capacity_per_shard(CACHE_CAPACITY)
                    .build();

                // Insert on shard 0
                cache.insert(affinities[0], 42, 42);
                cache
            },
            |cache| {
                // Get from shard 1 (cross-shard, triggers replication)
                black_box(cache.get(affinities[1], &42));
            },
            BatchSize::SmallInput,
        );
    });

    // Benchmark cross-shard get (after replication - local hit)
    group.bench_function("get_cross_shard_after_replication", |b| {
        let cache = NumaCache::<usize, usize>::builder()
            .affinities(&affinities)
            .capacity_per_shard(CACHE_CAPACITY)
            .build();

        // Insert on shard 0
        cache.insert(affinities[0], 42, 42);
        // Trigger replication to shard 1
        let _ = cache.get(affinities[1], &42);

        b.iter(|| {
            // Now it's a local hit on shard 1
            black_box(cache.get(affinities[1], &42));
        });
    });

    // Benchmark worst case: all-miss cross-shard search
    group.bench_function("get_all_miss", |b| {
        let cache = NumaCache::<usize, usize>::builder()
            .affinities(&affinities)
            .capacity_per_shard(CACHE_CAPACITY)
            .build();

        // Don't insert anything - all gets will search all shards and miss
        let mut key = 0usize;
        b.iter(|| {
            key += 1;
            black_box(cache.get(affinities[0], &key));
        });
    });

    group.finish();
}

/// Benchmark the Bloom filter's impact on performance.
fn bench_bloom_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("NumaCache_BloomFilter");
    let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);

    // Benchmark Bloom filter negative lookup (key never existed)
    group.bench_function("bloom_negative_lookup", |b| {
        let cache = NumaCache::<usize, usize>::builder()
            .affinities(&affinities)
            .capacity_per_shard(CACHE_CAPACITY)
            .build();

        // Pre-populate with some data (but not the keys we'll query)
        let affinity = affinities[0];
        for i in 0..1000 {
            cache.insert(affinity, i, i);
        }

        // Query keys that were never inserted - Bloom filter should return None fast
        let mut key = 100_000usize;
        b.iter(|| {
            key += 1;
            black_box(cache.get(affinity, &key));
        });
    });

    // Benchmark Bloom filter positive lookup (key exists in another shard)
    group.bench_function("bloom_positive_cross_shard", |b| {
        let cache = NumaCache::<usize, usize>::builder()
            .affinities(&affinities)
            .capacity_per_shard(CACHE_CAPACITY)
            .build();

        // Insert on shard 0
        for i in 0..1000 {
            cache.insert(affinities[0], i, i);
        }

        // Query from shard 1 - Bloom filter says "maybe", then we find it cross-shard
        let mut rng = StdRng::seed_from_u64(42);
        b.iter(|| {
            let key = rng.random_range(0..1000);
            black_box(cache.get(affinities[1], &key));
        });
    });

    // Benchmark heavy miss workload (where Bloom filter shines)
    group.throughput(Throughput::Elements(1000));
    group.bench_function("heavy_miss_workload", |b| {
        let cache = NumaCache::<usize, usize>::builder()
            .affinities(&affinities)
            .capacity_per_shard(CACHE_CAPACITY)
            .build();
        let affinity = affinities[0];

        // Insert only 100 items
        for i in 0..100 {
            cache.insert(affinity, i, i);
        }

        // 90% miss rate workload
        let mut rng = StdRng::seed_from_u64(42);
        b.iter(|| {
            for _ in 0..1000 {
                let key = rng.random_range(0usize..1000); // Only 100 keys exist
                black_box(cache.get(affinity, &key));
            }
        });
    });

    group.finish();
}
