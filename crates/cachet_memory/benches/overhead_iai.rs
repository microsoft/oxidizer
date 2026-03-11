// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Instruction-count benchmarks comparing raw moka performance against `InMemoryCache` wrapper
//! overhead. Uses gungraun (Valgrind/Callgrind) for deterministic, noise-free measurements.
//!
//! Requires Valgrind — Linux/macOS only. On Windows, use WSL2.

#![allow(missing_docs, reason = "Benchmark code")]

#[cfg(unix)]
use std::hint::black_box;

#[cfg(unix)]
use cachet_memory::InMemoryCache;
#[cfg(unix)]
use cachet_tier::{CacheEntry, CacheTier};
#[cfg(unix)]
use gungraun::{library_benchmark, library_benchmark_group, main};
#[cfg(unix)]
use moka::future::Cache as MokaCache;
#[cfg(unix)]
use tokio::runtime::Runtime;

#[cfg(unix)]
fn rt() -> Runtime {
    Runtime::new().expect("failed to create runtime")
}

#[cfg(unix)]
fn setup_moka() -> MokaCache<String, String> {
    let rt = rt();
    let cache: MokaCache<String, String> = MokaCache::builder().build();
    rt.block_on(async {
        for i in 0..1000 {
            cache.insert(format!("key_{i}"), format!("value_{i}")).await;
        }
    });
    cache
}

#[cfg(unix)]
fn setup_cachet() -> InMemoryCache<String, String> {
    let rt = rt();
    let cache = InMemoryCache::<String, String>::new();
    rt.block_on(async {
        for i in 0..1000 {
            let _ = cache.insert(&format!("key_{i}"), CacheEntry::new(format!("value_{i}"))).await;
        }
    });
    cache
}

// -- get_hit benchmarks --

#[cfg(unix)]
#[library_benchmark]
#[bench::moka(setup_moka())]
fn get_hit_moka(cache: MokaCache<String, String>) {
    let rt = rt();
    rt.block_on(async {
        for i in 0..100 {
            let key = format!("key_{}", i % 1000);
            let _ = black_box(cache.get(&key).await);
        }
    });
}

#[cfg(unix)]
#[library_benchmark]
#[bench::cachet(setup_cachet())]
fn get_hit_cachet(cache: InMemoryCache<String, String>) {
    let rt = rt();
    rt.block_on(async {
        for i in 0..100 {
            let key = format!("key_{}", i % 1000);
            let _ = black_box(cache.get(&key).await);
        }
    });
}

// -- get_miss benchmarks --

#[cfg(unix)]
#[library_benchmark]
#[bench::moka(setup_moka())]
fn get_miss_moka(cache: MokaCache<String, String>) {
    let rt = rt();
    rt.block_on(async {
        for i in 0..100 {
            let key = format!("missing_{i}");
            let _ = black_box(cache.get(&key).await);
        }
    });
}

#[cfg(unix)]
#[library_benchmark]
#[bench::cachet(setup_cachet())]
fn get_miss_cachet(cache: InMemoryCache<String, String>) {
    let rt = rt();
    rt.block_on(async {
        for i in 0..100 {
            let key = format!("missing_{i}");
            let _ = black_box(cache.get(&key).await);
        }
    });
}

// -- insert benchmarks --

#[cfg(unix)]
#[library_benchmark]
#[bench::moka(setup_moka())]
fn insert_moka(cache: MokaCache<String, String>) {
    let rt = rt();
    rt.block_on(async {
        for i in 0..100 {
            cache.insert(format!("new_key_{i}"), format!("new_value_{i}")).await;
        }
    });
}

#[cfg(unix)]
#[library_benchmark]
#[bench::cachet(setup_cachet())]
fn insert_cachet(cache: InMemoryCache<String, String>) {
    let rt = rt();
    rt.block_on(async {
        for i in 0..100 {
            let _ = cache
                .insert(&format!("new_key_{i}"), CacheEntry::new(format!("new_value_{i}")))
                .await;
        }
    });
}

#[cfg(unix)]
library_benchmark_group!(name = get_hit; benchmarks = get_hit_moka, get_hit_cachet);
#[cfg(unix)]
library_benchmark_group!(name = get_miss; benchmarks = get_miss_moka, get_miss_cachet);
#[cfg(unix)]
library_benchmark_group!(name = insert; benchmarks = insert_moka, insert_cachet);

#[cfg(unix)]
main!(library_benchmark_groups = get_hit, get_miss, insert);

#[cfg(not(unix))]
fn main() {
    eprintln!("gungraun benchmarks require Valgrind (Linux/macOS). Skipping.");
}
