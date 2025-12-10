// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! A high-performance, NUMA-aware in-memory cache with SIEVE eviction.
//!
//! This crate provides [`NumaCache`], a cache designed for high read-throughput and low-latency
//! on multi-socket architectures. It combines several techniques:
//!
//! 1. **Topology-Aware Sharding:** Data is partitioned by physical NUMA nodes to minimize
//!    QPI/UPI interconnect traffic.
//! 2. **Swiss Table Storage:** Utilizes [`hashbrown`] for SIMD-accelerated lookups.
//! 3. **SIEVE Eviction Policy:** A scan-resistant, efficient eviction algorithm that outperforms
//!    LRU in concurrent environments by minimizing metadata writes on reads.
//! 4. **Thread-Aware Integration:** Built on top of [`thread_aware`] for true NUMA locality
//!    via [`PinnedAffinity`]-based shard routing.
//! 5. **Read-Through Replication:** Automatic cross-shard cloning promotes hot data to the
//!    local shard, improving locality for subsequent accesses.
//! 6. **NUMA-Aware Memory Allocation:** When a [`ThreadRegistry`] is provided, shard memory
//!    is allocated while pinned to the correct NUMA node via first-touch policy.
//!
//! # Architecture
//!
//! The cache operates on a **Shared-Nothing-per-Node** model with **automatic locality promotion**.
//! Rather than sharding by key hash (which causes random memory access across sockets),
//! we shard by **Thread Affinity**.
//!
//! When a key is requested from a shard where it doesn't exist, but exists in another shard,
//! the value is automatically cloned to the local shard. This "read-through with local caching"
//! approach ensures:
//!
//! - **Fast path:** Local lookups are O(1) with zero interconnect traffic
//! - **Automatic locality:** Hot data migrates to where it's being accessed
//! - **Independent eviction:** Each shard manages its own capacity via SIEVE
//!
//! Each shard is cache-line aligned (64 bytes) to prevent false sharing between locks.
//!
//! # Performance Characteristics
//!
//! | Metric | Complexity | Notes |
//! | :--- | :--- | :--- |
//! | **Lookup (Local Hit)** | $O(1)$ | Zero-interconnect traffic. |
//! | **Lookup (Remote Hit)** | $O(n)$ shards | Clones to local shard for future O(1) access. |
//! | **Insertion** | Amortized $O(1)$ | Includes potential eviction scan. |
//! | **Eviction** | Amortized $O(1)$ | SIEVE hand movement is minimal in practice. |
//! | **Removal** | $O(n)$ shards | Removes from all shards (due to replication). |
//! | **Concurrency** | Sharded `RwLock` | No false sharing due to explicit padding. |
//!
//! # Example
//!
//! Use the `affinities()` builder method to create a cache with one shard per affinity,
//! ensuring each shard is explicitly associated with a specific affinity (e.g., NUMA node):
//!
//! ```
//! use thread_aware_cache::{NumaCache, PinnedAffinity};
//! use thread_aware::create_manual_pinned_affinities;
//!
//! // Create affinities representing 4 NUMA nodes with 1 processor each
//! let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);
//!
//! // Create a cache with one shard per affinity
//! let cache = NumaCache::<String, i32>::builder()
//!     .affinities(&affinities)
//!     .capacity_per_shard(10000)
//!     .build();
//!
//! // The cache now has exactly 4 shards, one per affinity
//! assert_eq!(cache.num_shards(), 4);
//!
//! // Insert data from NUMA node 0
//! cache.insert(affinities[0], "key".to_string(), 42);
//!
//! // Access from node 0 (local hit - fast path)
//! assert_eq!(cache.get(affinities[0], &"key".to_string()), Some(42));
//!
//! // Access from node 1 (cross-shard, clones locally for future access)
//! assert_eq!(cache.get(affinities[1], &"key".to_string()), Some(42));
//!
//! // Future accesses from node 1 are now local hits
//! assert_eq!(cache.get(affinities[1], &"key".to_string()), Some(42));
//! ```
//!
//! # Cross-Shard Behavior
//!
//! The cache automatically handles cross-shard access patterns:
//!
//! - **`get()`**: Checks local shard first, then searches others. On remote hit, clones to local.
//! - **`insert()`**: Always inserts to the specified affinity's shard.
//! - **`remove()`**: Removes from ALL shards (since values may be replicated).
//!
//! This design is ideal for read-heavy workloads where data naturally becomes "hot" on certain
//! nodes and should migrate there for optimal NUMA locality.
//!
//! # SIEVE Algorithm
//!
//! SIEVE is superior to LRU because it does not require pointer manipulation on reads,
//! only a boolean flag update. This makes it particularly efficient in concurrent environments.
//!
//! On access, we simply set a `visited` flag to `true`. On eviction, we scan from the "hand"
//! position, clearing `visited` flags until we find an unvisited entry to evict.
//!
//! # Bloom Filter Optimization
//!
//! The cache uses a shared lock-free Bloom filter to optimize cross-shard lookups. Before
//! searching all shards for a key, the cache queries the Bloom filter:
//!
//! - **Negative result:** Key definitely doesn't exist → skip cross-shard search (O(1))
//! - **Positive result:** Key might exist → proceed with cross-shard search
//!
//! The Bloom filter is sized for ~1% false positive rate and uses atomic operations for
//! lock-free concurrent access. Note that removals don't clear bits from the filter, so
//! stale positives may occur (safe, just slower), but false negatives never occur.
//!
//! # References
//!
//! 1. **Swiss Table:** Google Abseil `flat_hash_map`.
//! 2. **SIEVE:** *SIEVE is Simpler than LRU: an Efficient Turn-Key Eviction Algorithm for Web
//!    Caches* (NSDI '24).
//! 3. **False Sharing:** Intel Developer Guide on Cache Line definitions (64 bytes).
//! 4. **NUMA Replication:** Similar to OS page migration strategies for hot data.
//! 5. **Bloom Filter:** Kirsch-Mitzenmacher optimization for deriving multiple hash functions.

mod bloom;
mod cache;
mod shard;
mod sieve;

pub use cache::{NumaCache, NumaCacheBuilder};

// Re-export thread_aware types for convenience
pub use thread_aware::{PinnedAffinity, ProcessorCount, ThreadRegistry};

#[cfg(test)]
mod tests;
