// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! NUMA-aware cache implementation.
//!
//! This module provides the main [`NumaCache`] type and its builder.

use std::hash::Hash;
use std::sync::Arc;

use hashbrown::DefaultHashBuilder;
use thread_aware::ThreadRegistry;

use crate::bloom::BloomFilter;
use crate::shard::NumaShard;

/// A high-performance, NUMA-aware in-memory cache with SIEVE eviction.
///
/// The cache partitions data across multiple shards, with routing based on
/// thread affinity to minimize cross-NUMA traffic. Each shard is explicitly
/// associated with a [`thread_aware::PinnedAffinity`].
///
/// A shared Bloom filter optimizes cross-shard lookups by quickly identifying
/// keys that definitely don't exist in any shard, avoiding expensive O(n) searches.
///
/// # Type Parameters
///
/// * `K` - The key type, must implement `Eq + Hash + Clone`.
/// * `V` - The value type, must implement `Clone`.
/// * `S` - The hash builder type, defaults to `DefaultHashBuilder`.
///
/// # Examples
///
/// ```
/// use thread_aware_cache::NumaCache;
/// use thread_aware::create_manual_pinned_affinities;
///
/// let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);
/// let cache = NumaCache::<String, i32>::builder()
///     .affinities(&affinities)
///     .capacity_per_shard(1000)
///     .build();
///
/// cache.insert(affinities[0], "hello".to_string(), 42);
/// assert_eq!(cache.get(affinities[0], &"hello".to_string()), Some(42));
/// ```
pub struct NumaCache<K, V, S = DefaultHashBuilder> {
    /// The shards, one per affinity.
    shards: Arc<[NumaShard<K, V, S>]>,
    /// The affinities corresponding to each shard.
    affinities: Arc<[thread_aware::PinnedAffinity]>,
    /// Shared Bloom filter for fast negative lookups across all shards.
    bloom_filter: Arc<BloomFilter<S>>,
}

impl<K, V, S> std::fmt::Debug for NumaCache<K, V, S>
where
    K: Eq + Hash + Clone,
    V: Clone,
    S: std::hash::BuildHasher,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NumaCache")
            .field("num_shards", &self.shards.len())
            .field("shards", &self.shards)
            .field("bloom_filter", &self.bloom_filter)
            .finish_non_exhaustive()
    }
}

impl<K, V> NumaCache<K, V, DefaultHashBuilder>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    /// Creates a new builder for configuring a `NumaCache`.
    #[must_use]
    pub fn builder() -> NumaCacheBuilder<K, V, DefaultHashBuilder> {
        NumaCacheBuilder::new()
    }
}

impl<K, V, S> NumaCache<K, V, S>
where
    K: Eq + Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
    S: std::hash::BuildHasher + Default + Send + Sync,
{
    /// Creates a new cache with the specified shards, affinities, and Bloom filter.
    fn from_shards(
        shards: Vec<NumaShard<K, V, S>>,
        affinities: Vec<thread_aware::PinnedAffinity>,
        bloom_filter: BloomFilter<S>,
    ) -> Self {
        debug_assert_eq!(shards.len(), affinities.len(), "shards and affinities must have the same length");
        Self {
            shards: shards.into(),
            affinities: affinities.into(),
            bloom_filter: Arc::new(bloom_filter),
        }
    }

    /// Returns the number of shards (one per affinity).
    #[must_use]
    pub fn num_shards(&self) -> usize {
        self.shards.len()
    }

    /// Returns the affinities associated with this cache.
    ///
    /// Each affinity corresponds to a shard at the same index.
    #[must_use]
    pub fn affinities(&self) -> &[thread_aware::PinnedAffinity] {
        &self.affinities
    }

    /// Returns the total number of entries across all shards.
    ///
    /// Note: This requires acquiring read locks on all shards, so it may not
    /// be suitable for high-frequency calls.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shards.iter().map(NumaShard::len).sum()
    }

    /// Returns `true` if the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shards.iter().all(NumaShard::is_empty)
    }

    /// Returns the total capacity across all shards.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shards.iter().map(NumaShard::capacity).sum()
    }

    /// Clears all entries from the cache.
    pub fn clear(&self) {
        for shard in self.shards.iter() {
            shard.clear();
        }
    }

    /// Gets a reference to a specific shard by index.
    ///
    /// # Panics
    ///
    /// Panics if `index >= num_shards()`.
    #[must_use]
    pub fn shard(&self, index: usize) -> &NumaShard<K, V, S> {
        &self.shards[index]
    }

    /// Returns the shard for the given affinity.
    ///
    /// This method looks up the shard that was explicitly associated with
    /// the given affinity during cache construction.
    fn select_shard_for_affinity(&self, affinity: thread_aware::PinnedAffinity) -> &NumaShard<K, V, S> {
        // Find the shard index by looking up the affinity in our stored affinities
        let shard_index = self
            .affinities
            .iter()
            .position(|a| *a == affinity)
            .unwrap_or_else(|| {
                // Fallback to memory region index if affinity not found
                affinity.memory_region_index() % self.shards.len()
            });
        &self.shards[shard_index]
    }

    /// Looks up a key in the cache using affinity-based shard selection.
    ///
    /// This method first checks the local shard (associated with the given affinity)
    /// for maximum NUMA locality. If the key is not found locally, it consults the
    /// shared Bloom filter before searching other shards:
    ///
    /// - If the Bloom filter says the key definitely doesn't exist, return `None` immediately
    /// - If the Bloom filter says the key might exist, search other shards
    ///
    /// When found in a remote shard, the value is automatically cloned to the local
    /// shard, promoting future NUMA-local access.
    ///
    /// This "read-through with local caching" approach provides:
    /// - Fast path: O(1) lookup when data is already local
    /// - Bloom filter optimization: O(1) for definite misses (no cross-shard search)
    /// - Automatic locality promotion: Hot data migrates to where it's accessed
    /// - Shard-local eviction: Each shard independently manages its capacity
    ///
    /// # Examples
    ///
    /// ```
    /// use thread_aware_cache::NumaCache;
    /// use thread_aware::create_manual_pinned_affinities;
    ///
    /// let affinities = create_manual_pinned_affinities(&[1, 1]);
    /// let cache = NumaCache::<String, i32>::builder()
    ///     .affinities(&affinities)
    ///     .build();
    ///
    /// // Insert on shard 0
    /// cache.insert(affinities[0], "key".to_string(), 42);
    ///
    /// // Get from shard 0 (local hit)
    /// assert_eq!(cache.get(affinities[0], &"key".to_string()), Some(42));
    ///
    /// // Get from shard 1 (cross-shard clone)
    /// assert_eq!(cache.get(affinities[1], &"key".to_string()), Some(42));
    ///
    /// // Now the value is also in shard 1 (future accesses are local)
    /// ```
    #[must_use]
    pub fn get(&self, affinity: thread_aware::PinnedAffinity, key: &K) -> Option<V> {
        let local_shard = self.select_shard_for_affinity(affinity);

        // Fast path: check local shard first (NUMA-local access)
        if let Some(value) = local_shard.get(key) {
            return Some(value);
        }

        // Bloom filter check: if key definitely doesn't exist anywhere, skip cross-shard search
        if !self.bloom_filter.might_contain(key) {
            return None;
        }

        // Slow path: search other shards for the key
        for shard in self.shards.iter() {
            // Skip the local shard (already checked)
            if std::ptr::eq(shard, local_shard) {
                continue;
            }

            if let Some(value) = shard.get(key) {
                // Found in remote shard - clone to local shard for NUMA locality
                // This promotes hot data to be local to where it's being accessed
                local_shard.insert(key.clone(), value.clone());
                return Some(value);
            }
        }

        None
    }

    /// Inserts a key-value pair using affinity-based shard selection.
    ///
    /// This method routes the insertion to the shard corresponding to the given
    /// affinity, ensuring data is stored locally to that affinity's NUMA node.
    /// The key is also added to the shared Bloom filter for cross-shard lookup
    /// optimization.
    ///
    /// Returns the previous value if the key already existed.
    ///
    /// # Examples
    ///
    /// ```
    /// use thread_aware_cache::NumaCache;
    /// use thread_aware::create_manual_pinned_affinities;
    ///
    /// let affinities = create_manual_pinned_affinities(&[1, 1]);
    /// let cache = NumaCache::<String, i32>::builder()
    ///     .affinities(&affinities)
    ///     .build();
    /// assert!(cache.insert(affinities[0], "key".to_string(), 42).is_none());
    /// assert_eq!(cache.insert(affinities[0], "key".to_string(), 100), Some(42));
    /// ```
    pub fn insert(&self, affinity: thread_aware::PinnedAffinity, key: K, value: V) -> Option<V> {
        // Add to Bloom filter for cross-shard lookup optimization
        self.bloom_filter.insert(&key);
        self.select_shard_for_affinity(affinity).insert(key, value)
    }

    /// Removes a key from the cache.
    ///
    /// Since values may be replicated across multiple shards (due to cross-shard
    /// gets promoting data locality), this method removes the key from ALL shards
    /// where it exists, returning the value from the first shard that contained it.
    ///
    /// # Examples
    ///
    /// ```
    /// use thread_aware_cache::NumaCache;
    /// use thread_aware::create_manual_pinned_affinities;
    ///
    /// let affinities = create_manual_pinned_affinities(&[1, 1]);
    /// let cache = NumaCache::<String, i32>::builder()
    ///     .affinities(&affinities)
    ///     .build();
    ///
    /// // Insert on shard 0
    /// cache.insert(affinities[0], "key".to_string(), 42);
    ///
    /// // Access from shard 1 (clones to shard 1)
    /// let _ = cache.get(affinities[1], &"key".to_string());
    ///
    /// // Remove - clears from ALL shards
    /// assert_eq!(cache.remove(affinities[0], &"key".to_string()), Some(42));
    ///
    /// // Key is gone from both shards
    /// assert!(cache.get(affinities[0], &"key".to_string()).is_none());
    /// assert!(cache.get(affinities[1], &"key".to_string()).is_none());
    /// ```
    pub fn remove(&self, affinity: thread_aware::PinnedAffinity, key: &K) -> Option<V> {
        let local_shard = self.select_shard_for_affinity(affinity);
        let mut result = local_shard.remove(key);

        // Remove from all other shards as well (value may have been replicated)
        for shard in self.shards.iter() {
            if std::ptr::eq(shard, local_shard) {
                continue;
            }
            if let Some(value) = shard.remove(key) {
                // Keep the first value we found if we haven't found one yet
                if result.is_none() {
                    result = Some(value);
                }
            }
        }

        result
    }

    /// Returns the shard index for a given affinity.
    ///
    /// This looks up the affinity in the cache's stored affinities to find
    /// the corresponding shard index.
    #[must_use]
    pub fn shard_index_for_affinity(&self, affinity: thread_aware::PinnedAffinity) -> usize {
        self.affinities
            .iter()
            .position(|a| *a == affinity)
            .unwrap_or_else(|| {
                // Fallback to memory region index if affinity not found
                affinity.memory_region_index() % self.shards.len()
            })
    }
}

impl<K, V, S: Clone> Clone for NumaCache<K, V, S> {
    fn clone(&self) -> Self {
        Self {
            shards: Arc::clone(&self.shards),
            affinities: Arc::clone(&self.affinities),
            bloom_filter: Arc::clone(&self.bloom_filter),
        }
    }
}

// Safety: NumaCache is Send if K, V, and S are Send + Sync
// The shards use RwLock internally which provides the synchronization
unsafe impl<K, V, S> Send for NumaCache<K, V, S>
where
    K: Send + Sync,
    V: Send + Sync,
    S: Send + Sync,
{
}

// Safety: NumaCache is Sync if K, V, and S are Send + Sync
// The shards use RwLock internally which provides the synchronization
unsafe impl<K, V, S> Sync for NumaCache<K, V, S>
where
    K: Send + Sync,
    V: Send + Sync,
    S: Send + Sync,
{
}

/// Builder for configuring a [`NumaCache`].
///
/// # Examples
///
/// ```
/// use thread_aware_cache::NumaCache;
/// use thread_aware::create_manual_pinned_affinities;
///
/// let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);
/// let cache = NumaCache::<String, i32>::builder()
///     .affinities(&affinities)
///     .capacity_per_shard(10000)
///     .build();
/// ```
#[derive(Debug)]
pub struct NumaCacheBuilder<K, V, S = DefaultHashBuilder> {
    affinities: Vec<thread_aware::PinnedAffinity>,
    capacity_per_shard: usize,
    registry: Option<Arc<ThreadRegistry>>,
    _marker: std::marker::PhantomData<(K, V, S)>,
}

impl<K, V, S> Default for NumaCacheBuilder<K, V, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, S> NumaCacheBuilder<K, V, S> {
    /// Creates a new builder with default settings.
    ///
    /// You must call [`affinities()`](Self::affinities) before [`build()`](Self::build)
    /// to specify which affinities the cache shards will be associated with.
    ///
    /// Defaults:
    /// - `capacity_per_shard`: 1024
    #[must_use]
    pub fn new() -> Self {
        Self {
            affinities: Vec::new(),
            capacity_per_shard: 1024,
            registry: None,
            _marker: std::marker::PhantomData,
        }
    }

    /// Sets the affinities for the cache shards.
    ///
    /// Each affinity corresponds to a shard. The number of shards will equal
    /// the number of affinities provided. This ensures that each shard is
    /// associated with a specific affinity (e.g., NUMA node), enabling true
    /// NUMA-local access patterns.
    ///
    /// # Examples
    ///
    /// ```
    /// use thread_aware_cache::NumaCache;
    /// use thread_aware::create_manual_pinned_affinities;
    ///
    /// // Create 4 affinities representing 4 NUMA nodes
    /// let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);
    ///
    /// let cache = NumaCache::<String, i32>::builder()
    ///     .affinities(&affinities)
    ///     .capacity_per_shard(10000)
    ///     .build();
    ///
    /// // Now the cache has exactly 4 shards, one per affinity
    /// assert_eq!(cache.num_shards(), 4);
    /// ```
    #[must_use]
    pub fn affinities(mut self, affinities: &[thread_aware::PinnedAffinity]) -> Self {
        self.affinities = affinities.to_vec();
        self
    }

    /// Sets the capacity per shard.
    #[must_use]
    pub const fn capacity_per_shard(mut self, capacity: usize) -> Self {
        self.capacity_per_shard = capacity;
        self
    }

    /// Sets the thread registry for NUMA-aware memory allocation.
    ///
    /// When a registry is provided, each shard will be allocated while the
    /// current thread is pinned to the corresponding affinity. This leverages
    /// the OS's first-touch memory policy to ensure shard data is allocated
    /// on the correct NUMA node.
    ///
    /// If no registry is provided, shards are allocated on whatever NUMA node
    /// the builder thread happens to be running on, which may not be optimal.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// use thread_aware_cache::NumaCache;
    /// use thread_aware::{ThreadRegistry, ProcessorCount};
    ///
    /// let registry = Arc::new(ThreadRegistry::new(&ProcessorCount::All));
    /// let affinities: Vec<_> = registry.affinities().collect();
    ///
    /// let cache = NumaCache::<String, i32>::builder()
    ///     .affinities(&affinities)
    ///     .registry(Arc::clone(&registry))
    ///     .capacity_per_shard(10000)
    ///     .build();
    /// ```
    #[must_use]
    pub fn registry(mut self, registry: Arc<ThreadRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }
}

impl<K, V, S> NumaCacheBuilder<K, V, S>
where
    K: Eq + Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
    S: std::hash::BuildHasher + Default + Clone + Send + Sync,
{
    /// Builds the cache with the configured settings.
    ///
    /// The cache will have one shard per affinity, with each shard associated
    /// with its corresponding affinity. A shared Bloom filter is created sized
    /// for the total capacity across all shards.
    ///
    /// If a [`ThreadRegistry`] was provided via [`registry()`](Self::registry),
    /// each shard will be allocated while pinned to its corresponding affinity,
    /// ensuring NUMA-local memory allocation via the OS's first-touch policy.
    ///
    /// # Panics
    ///
    /// Panics if no affinities have been set via [`affinities()`](Self::affinities).
    #[must_use]
    pub fn build(self) -> NumaCache<K, V, S> {
        assert!(!self.affinities.is_empty(), "affinities must be set before building the cache");

        let num_shards = self.affinities.len();
        let total_capacity = num_shards * self.capacity_per_shard;

        let shards: Vec<NumaShard<K, V, S>> = match &self.registry {
            Some(registry) => {
                // NUMA-aware allocation: allocate each shard while pinned to its affinity.
                // This leverages the OS's first-touch memory policy to ensure shard data
                // is allocated on the correct NUMA node.
                self.affinities
                    .iter()
                    .map(|affinity| {
                        registry.pin_to(*affinity);
                        NumaShard::new(self.capacity_per_shard)
                    })
                    .collect()
            }
            None => {
                // Non-NUMA-aware allocation: all shards allocated on current thread's node
                self.affinities.iter().map(|_| NumaShard::new(self.capacity_per_shard)).collect()
            }
        };

        // Create Bloom filter sized for total capacity across all shards
        let bloom_filter = BloomFilter::new(total_capacity, S::default());

        NumaCache::from_shards(shards, self.affinities, bloom_filter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thread_aware::create_manual_pinned_affinities;

    #[test]
    fn test_cache_builder_custom() {
        let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);
        let cache = NumaCache::<String, i32>::builder()
            .affinities(&affinities)
            .capacity_per_shard(100)
            .build();

        assert_eq!(cache.num_shards(), 4);
        assert_eq!(cache.capacity(), 400);
    }

    #[test]
    fn test_cache_basic_operations() {
        let affinities = create_manual_pinned_affinities(&[1, 1]);
        let cache = NumaCache::<String, i32>::builder()
            .affinities(&affinities)
            .capacity_per_shard(10)
            .build();

        assert!(cache.is_empty());

        // Insert
        assert!(cache.insert(affinities[0], "key1".to_string(), 100).is_none());
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);

        // Get
        assert_eq!(cache.get(affinities[0], &"key1".to_string()), Some(100));
        assert!(cache.get(affinities[0], &"nonexistent".to_string()).is_none());

        // Update
        assert_eq!(cache.insert(affinities[0], "key1".to_string(), 200), Some(100));
        assert_eq!(cache.get(affinities[0], &"key1".to_string()), Some(200));

        // Remove
        assert_eq!(cache.remove(affinities[0], &"key1".to_string()), Some(200));
        assert!(cache.get(affinities[0], &"key1".to_string()).is_none());

        // Clear
        cache.insert(affinities[0], "a".to_string(), 1);
        cache.insert(affinities[1], "b".to_string(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_clone() {
        let affinities = create_manual_pinned_affinities(&[1, 1]);
        let cache = NumaCache::<String, i32>::builder()
            .affinities(&affinities)
            .capacity_per_shard(10)
            .build();

        cache.insert(affinities[0], "key".to_string(), 42);

        let cache_clone = cache.clone();

        // Both should see the same data (shared Arc)
        assert_eq!(cache_clone.get(affinities[0], &"key".to_string()), Some(42));

        // Modifications through one should be visible through the other
        cache_clone.insert(affinities[1], "key2".to_string(), 100);
        assert_eq!(cache.get(affinities[1], &"key2".to_string()), Some(100));
    }

    #[test]
    fn test_cache_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NumaCache<String, i32>>();
    }

    #[test]
    fn test_affinity_shard_selection() {
        // Create 4 NUMA nodes with 1 processor each
        let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);
        let cache = NumaCache::<String, i32>::builder()
            .affinities(&affinities)
            .capacity_per_shard(100)
            .build();

        // Each affinity should map to a different shard
        let mut shard_indices = [
            cache.shard_index_for_affinity(affinities[0]),
            cache.shard_index_for_affinity(affinities[1]),
            cache.shard_index_for_affinity(affinities[2]),
            cache.shard_index_for_affinity(affinities[3]),
        ];

        // With 4 affinities from 4 different NUMA nodes and 4 shards, each should map to a unique shard
        shard_indices.sort_unstable();
        assert_eq!(shard_indices, [0, 1, 2, 3]);
    }

    #[test]
    fn test_affinity_based_operations() {
        // Create 2 NUMA nodes with 1 processor each
        let affinities = create_manual_pinned_affinities(&[1, 1]);
        let cache = NumaCache::<String, i32>::builder()
            .affinities(&affinities)
            .capacity_per_shard(100)
            .build();

        let affinity0 = affinities[0];
        let affinity1 = affinities[1];

        // Insert with affinity0
        assert!(cache.insert(affinity0, "key0".to_string(), 100).is_none());

        // Insert with affinity1
        assert!(cache.insert(affinity1, "key1".to_string(), 200).is_none());

        // Get with correct affinities
        assert_eq!(cache.get(affinity0, &"key0".to_string()), Some(100));
        assert_eq!(cache.get(affinity1, &"key1".to_string()), Some(200));

        // Remove with affinity
        assert_eq!(cache.remove(affinity0, &"key0".to_string()), Some(100));
        assert!(cache.get(affinity0, &"key0".to_string()).is_none());
    }

    #[test]
    fn test_affinity_locality() {
        // Create 4 NUMA nodes with 1 processor each
        let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);
        let cache = NumaCache::<u64, u64>::builder()
            .affinities(&affinities)
            .capacity_per_shard(100)
            .build();

        // Insert data using specific affinities
        for (i, &affinity) in affinities.iter().enumerate() {
            for j in 0u64..10 {
                let key = (i as u64) * 100 + j;
                cache.insert(affinity, key, key * 10);
            }
        }

        // Verify data is in the expected shards
        for (i, &affinity) in affinities.iter().enumerate() {
            let shard_idx = cache.shard_index_for_affinity(affinity);
            let shard = cache.shard(shard_idx);

            // Data inserted with this affinity should be in this shard
            for j in 0u64..10 {
                let key = (i as u64) * 100 + j;
                assert_eq!(shard.get(&key), Some(key * 10), "key {key} should be in shard {shard_idx}");
            }
        }
    }
}
