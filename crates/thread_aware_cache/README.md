<div align="center">

# Thread-Aware Cache

[![crate.io](https://img.shields.io/crates/v/thread_aware_cache.svg)](https://crates.io/crates/thread_aware_cache)
[![docs.rs](https://docs.rs/thread_aware_cache/badge.svg)](https://docs.rs/thread_aware_cache)
[![MSRV](https://img.shields.io/crates/msrv/thread_aware_cache)](https://crates.io/crates/thread_aware_cache)
[![CI](https://github.com/microsoft/oxidizer/workflows/main/badge.svg)](https://github.com/microsoft/oxidizer/actions)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

</div>

* [Summary](#summary)
* [Architecture](#architecture)
* [Performance Characteristics](#performance-characteristics)
* [Pros and Cons](#pros-and-cons)
* [Example](#example)
* [Cross-Shard Behavior](#cross-shard-behavior)
* [Bloom Filter Optimization](#bloom-filter-optimization)
* [SIEVE Algorithm](#sieve-algorithm)
* [References](#references)

## Summary

<!-- cargo-rdme start -->

A high-performance, NUMA-aware in-memory cache with SIEVE eviction.

This crate provides [`NumaCache`], a cache designed for high read-throughput and low-latency
on multi-socket architectures. It combines several techniques:

1. **Topology-Aware Sharding:** Data is partitioned by physical NUMA nodes to minimize
   QPI/UPI interconnect traffic.
2. **Swiss Table Storage:** Utilizes [`hashbrown`] for SIMD-accelerated lookups.
3. **SIEVE Eviction Policy:** A scan-resistant, efficient eviction algorithm that outperforms
   LRU in concurrent environments by minimizing metadata writes on reads.
4. **Thread-Aware Integration:** Built on top of [`thread_aware`] for true NUMA locality
   via [`PinnedAffinity`]-based shard routing.
5. **Read-Through Replication:** Automatic cross-shard cloning promotes hot data to the
   local shard, improving locality for subsequent accesses.
6. **Bloom Filter Optimization:** A lock-free Bloom filter provides fast negative lookups,
   avoiding expensive cross-shard scans when a key definitely doesn't exist.
7. **NUMA-Aware Memory Allocation:** When a `ThreadRegistry` is provided, each shard is
   allocated while pinned to its corresponding NUMA node, leveraging the OS's first-touch
   memory policy for optimal memory locality.

## Architecture

The cache operates on a **Shared-Nothing-per-Node** model with **automatic locality promotion**.
Rather than sharding by key hash (which causes random memory access across sockets),
we shard by **Thread Affinity**.

When a key is requested from a shard where it doesn't exist, but exists in another shard,
the value is automatically cloned to the local shard. This "read-through with local caching"
approach ensures:

- **Fast path:** Local lookups are O(1) with zero interconnect traffic
- **Automatic locality:** Hot data migrates to where it's being accessed
- **Independent eviction:** Each shard manages its own capacity via SIEVE

Each shard is cache-line aligned (64 bytes) to prevent false sharing between locks.

### NUMA-Aware Memory Allocation

For optimal performance on multi-socket systems, you can provide a `ThreadRegistry` to
ensure shard memory is allocated on the correct NUMA node:

```rust,ignore
use std::sync::Arc;
use thread_aware_cache::{NumaCache, ThreadRegistry, ProcessorCount};

// Create a registry with all available processors
let registry = Arc::new(ThreadRegistry::new(&ProcessorCount::All));
let affinities: Vec<_> = registry.affinities().collect();

// Build cache with NUMA-aware allocation
let cache = NumaCache::<String, i32>::builder()
    .affinities(&affinities)
    .registry(Arc::clone(&registry))
    .capacity_per_shard(10000)
    .build();
```

When a registry is provided, the builder pins the current thread to each affinity before
allocating that shard's memory. This leverages the operating system's **first-touch memory
policy**, ensuring each shard's data structures are physically allocated on the correct
NUMA node.

## Performance Characteristics

| Metric | Complexity | Notes |
| :--- | :--- | :--- |
| **Lookup (Local Hit)** | O(1) | Zero-interconnect traffic. |
| **Lookup (Remote Hit)** | O(n) shards | Clones to local shard for future O(1) access. |
| **Insertion** | Amortized O(1) | Includes potential eviction scan. |
| **Eviction** | Amortized O(1) | SIEVE hand movement is minimal in practice. |
| **Removal** | O(n) shards | Removes from all shards (due to replication). |
| **Concurrency** | Sharded `RwLock` | No false sharing due to explicit padding. |

## Pros and Cons

### Pros

* **Excellent NUMA Locality:** Minimizes cross-socket traffic (QPI/UPI) by keeping data local to the thread that accesses it.
* **High Read Throughput:** Local hits are fast and contention-free across sockets.
* **Automatic Hot Data Migration:** Frequently accessed data naturally moves to the shard where it is needed via read-through replication.
* **Scan-Resistant Eviction:** The SIEVE algorithm handles scan workloads better than LRU and requires less locking overhead on reads.
* **False Sharing Prevention:** Explicit padding ensures locks on different shards reside on different cache lines.

### Cons

* **Higher Memory Usage:** Because data is replicated to the local shard on access, the same key-value pair can exist in multiple shards simultaneously. This trades memory capacity for latency.
* **Expensive Removes:** Removing a key is an $O(N)$ operation because it requires acquiring write locks on *all* shards to ensure the value is removed from every replica.
* **Bloom Filter Saturation:** In workloads with very high churn (continuous inserts and deletes over a long period), the shared Bloom filter may eventually saturate (fill with 1s), reducing the effectiveness of negative lookups.
* **Not Ideal for Write-Heavy Workloads:** The overhead of replication and multi-shard consistency checks makes this cache less suitable for write-heavy or delete-heavy scenarios compared to a simple partitioned cache.

## Example

Use the `affinities()` builder method to create a cache with one shard per affinity,
ensuring each shard is explicitly associated with a specific affinity (e.g., NUMA node):

```rust
use thread_aware_cache::{NumaCache, PinnedAffinity};
use thread_aware::create_manual_pinned_affinities;

// Create affinities representing 4 NUMA nodes with 1 processor each
let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);

// Create a cache with one shard per affinity
let cache = NumaCache::<String, i32>::builder()
    .affinities(&affinities)
    .capacity_per_shard(10000)
    .build();

// The cache now has exactly 4 shards, one per affinity
assert_eq!(cache.num_shards(), 4);

// Insert data from NUMA node 0
cache.insert(affinities[0], "key".to_string(), 42);

// Access from node 0 (local hit - fast path)
assert_eq!(cache.get(affinities[0], &"key".to_string()), Some(42));

// Access from node 1 (cross-shard, clones locally for future access)
assert_eq!(cache.get(affinities[1], &"key".to_string()), Some(42));

// Future accesses from node 1 are now local hits
assert_eq!(cache.get(affinities[1], &"key".to_string()), Some(42));
```

## Cross-Shard Behavior

The cache automatically handles cross-shard access patterns:

- **get()**: Checks local shard first, then consults the Bloom filter. If the key might exist,
  searches other shards. On remote hit, clones to local.
- **insert()**: Always inserts to the specified affinity's shard and adds the key to the
  shared Bloom filter.
- **remove()**: Removes from ALL shards (since values may be replicated). The Bloom filter
  retains the key (stale positive), which is safe but may cause unnecessary cross-shard
  lookups for removed keys.

This design is ideal for read-heavy workloads where data naturally becomes "hot" on certain
nodes and should migrate there for optimal NUMA locality.

## Bloom Filter Optimization

The cache uses a **lock-free Bloom filter** to optimize cross-shard lookups. Before scanning
all shards for a key that's not in the local shard, the cache consults the Bloom filter:

- **Negative result (definitely not in cache):** Returns `None` immediately, avoiding
  expensive cross-shard scans. This is the fast path for cache misses.
- **Positive result (might be in cache):** Proceeds with cross-shard search. False positives
  are possible but safe—they just result in an unnecessary scan.

### Implementation Details

| Parameter | Value | Notes |
| :--- | :--- | :--- |
| **Bits per item** | 10 | Provides ~1% false positive rate |
| **Hash functions** | 7 | Optimal for 10 bits/item |
| **Storage** | `AtomicU64` array | Lock-free, cache-friendly |
| **Hash derivation** | Kirsch-Mitzenmacher | Two hashes → k probes via `h1 + i * h2` |

### Trade-offs

The Bloom filter does **not** support removal. When a key is removed from the cache, it
remains in the Bloom filter as a "stale positive." This design choice keeps the implementation
simple and lock-free, with the only cost being occasional unnecessary cross-shard scans for
removed keys. For typical cache workloads where reads vastly outnumber removes, this is an
acceptable trade-off.

### Future Improvement

For long-running cache instances with high churn (many inserts and removes over time),
stale positives can accumulate and degrade lookup performance. An improved Bloom filter
could address this by tracking insertion locations per bit position, enabling proper removal
support at the cost of increased memory usage (bit per shard instead of 1 bit).

## SIEVE Algorithm

SIEVE is superior to LRU because it does not require pointer manipulation on reads,
only a boolean flag update. This makes it particularly efficient in concurrent environments.

On access, we simply set a `visited` flag to `true`. On eviction, we scan from the "hand"
position, clearing `visited` flags until we find an unvisited entry to evict.

<!-- cargo-rdme end -->

## When to Use

Use `thread_aware_cache` when you need a concurrent cache on multi-socket (NUMA) hardware
where minimizing cross-node memory access is critical for performance.

## References

- [SIEVE: NSDI '24](https://www.usenix.org/conference/nsdi24/presentation/zhang-yazhuo) –
  *SIEVE is Simpler than LRU: an Efficient Turn-Key Eviction Algorithm for Web Caches*
- [Swiss Table](https://abseil.io/about/design/swisstables) – Google Abseil `flat_hash_map`
- [False Sharing](https://www.intel.com/content/www/us/en/developer/articles/technical/avoiding-and-identifying-false-sharing-among-threads.html) – Intel Developer Guide on Cache Line definitions (64 bytes)

<div style="font-size: 75%" ><hr/>

This crate was developed as part of [The Oxidizer Project](https://github.com/microsoft/oxidizer).

</div>
