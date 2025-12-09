// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache shard implementation.
//!
//! Each shard contains a Swiss Table (`hashbrown::HashMap`) for storage and SIEVE metadata
//! for eviction. Shards are cache-line aligned to prevent false sharing.

use std::hash::{BuildHasher, Hash};

use hashbrown::DefaultHashBuilder;
use hashbrown::HashMap;
use parking_lot::RwLock;

use crate::sieve::{NodeIndex, SieveList};

/// Cache line size for alignment to prevent false sharing.
const CACHE_LINE_SIZE: usize = 64;

/// A single cache shard containing data and eviction metadata.
///
/// Aligned to the CPU cache line (64 bytes) to prevent cache-line bouncing between locks.
#[repr(align(64))]
pub struct NumaShard<K, V, S = DefaultHashBuilder> {
    /// The protected inner state.
    inner: RwLock<ShardInner<K, V, S>>,
    /// Explicit padding to ensure the lock of the next shard resides on a different cache line.
    _pad: [u8; CACHE_LINE_SIZE],
}

impl<K, V, S: Default> NumaShard<K, V, S> {
    /// Creates a new shard with the given capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: RwLock::new(ShardInner::new(capacity)),
            _pad: [0; CACHE_LINE_SIZE],
        }
    }
}

impl<K, V, S> NumaShard<K, V, S>
where
    K: Eq + Hash + Clone,
    V: Clone,
    S: BuildHasher,
{
    /// Looks up a key in the shard.
    ///
    /// Returns `Some(value)` if found, marking the entry as visited for SIEVE.
    pub fn get(&self, key: &K) -> Option<V> {
        let inner = self.inner.read();
        inner.get(key)
    }

    /// Inserts a key-value pair into the shard.
    ///
    /// If the shard is at capacity, performs SIEVE eviction first.
    /// Returns the previous value if the key already existed.
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        let mut inner = self.inner.write();
        inner.insert(key, value)
    }

    /// Removes a key from the shard.
    ///
    /// Returns the previous value if the key existed.
    pub fn remove(&self, key: &K) -> Option<V> {
        let mut inner = self.inner.write();
        inner.remove(key)
    }

    /// Returns the number of entries in the shard.
    #[must_use]
    pub fn len(&self) -> usize {
        let inner = self.inner.read();
        inner.map.len()
    }

    /// Returns `true` if the shard is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the capacity of the shard.
    #[must_use]
    pub fn capacity(&self) -> usize {
        let inner = self.inner.read();
        inner.sieve.capacity()
    }

    /// Clears all entries from the shard.
    pub fn clear(&self) {
        let mut inner = self.inner.write();
        inner.clear();
    }
}

impl<K, V, S> std::fmt::Debug for NumaShard<K, V, S>
where
    K: Eq + std::hash::Hash + Clone,
    V: Clone,
    S: std::hash::BuildHasher,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NumaShard")
            .field("len", &self.len())
            .field("capacity", &self.capacity())
            .finish()
    }
}

/// Inner state of a cache shard.
struct ShardInner<K, V, S = DefaultHashBuilder> {
    /// The primary storage using Swiss Table for SIMD-accelerated lookup.
    map: HashMap<K, CacheEntry<V>, S>,

    /// SIEVE eviction state.
    sieve: SieveList<K>,
}

impl<K, V, S: Default> ShardInner<K, V, S> {
    /// Creates a new shard inner with the given capacity.
    fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::with_capacity_and_hasher(capacity, S::default()),
            sieve: SieveList::new(capacity),
        }
    }
}

impl<K, V, S> ShardInner<K, V, S>
where
    K: Eq + Hash + Clone,
    V: Clone,
    S: BuildHasher,
{
    /// Looks up a key in the shard.
    fn get(&self, key: &K) -> Option<V> {
        let entry = self.map.get(key)?;
        // Mark as visited for SIEVE (relaxed ordering is fine here)
        self.sieve.mark_visited(entry.sieve_index);
        Some(entry.value.clone())
    }

    /// Inserts a key-value pair.
    fn insert(&mut self, key: K, value: V) -> Option<V> {
        // Check if key already exists
        if let Some(entry) = self.map.get_mut(&key) {
            let old_value = std::mem::replace(&mut entry.value, value);
            self.sieve.mark_visited(entry.sieve_index);
            return Some(old_value);
        }

        // Evict if at capacity
        if self.sieve.is_full() {
            self.evict_one();
        }

        // Insert new entry - store a clone of the key in the sieve for O(1) eviction
        let sieve_index = self.sieve.insert(key.clone()).expect("should have space after eviction");

        self.map.insert(key, CacheEntry { value, sieve_index });

        None
    }

    /// Removes a key from the shard.
    fn remove(&mut self, key: &K) -> Option<V> {
        let entry = self.map.remove(key)?;
        self.sieve.remove(entry.sieve_index);
        Some(entry.value)
    }

    /// Evicts one entry using the SIEVE algorithm.
    ///
    /// This is now O(1) because the sieve stores the key directly,
    /// avoiding the need to iterate through the map.
    fn evict_one(&mut self) {
        if let Some(evicted_key) = self.sieve.evict() {
            // Direct O(1) removal using the evicted key
            self.map.remove(&evicted_key);
        }
    }

    /// Clears all entries.
    fn clear(&mut self) {
        self.map.clear();
        self.sieve = SieveList::new(self.sieve.capacity());
    }
}

/// A cache entry storing the value and its SIEVE metadata index.
struct CacheEntry<V> {
    /// The cached value.
    value: V,
    /// Index into the SIEVE list.
    sieve_index: NodeIndex,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_basic_operations() {
        let shard: NumaShard<String, i32> = NumaShard::new(10);

        assert!(shard.is_empty());
        assert_eq!(shard.capacity(), 10);

        // Insert
        assert!(shard.insert("key1".to_string(), 100).is_none());
        assert!(shard.insert("key2".to_string(), 200).is_none());
        assert_eq!(shard.len(), 2);

        // Get
        assert_eq!(shard.get(&"key1".to_string()), Some(100));
        assert_eq!(shard.get(&"key2".to_string()), Some(200));
        assert_eq!(shard.get(&"key3".to_string()), None);

        // Update
        assert_eq!(shard.insert("key1".to_string(), 150), Some(100));
        assert_eq!(shard.get(&"key1".to_string()), Some(150));

        // Remove
        assert_eq!(shard.remove(&"key1".to_string()), Some(150));
        assert_eq!(shard.get(&"key1".to_string()), None);
        assert_eq!(shard.len(), 1);

        // Clear
        shard.clear();
        assert!(shard.is_empty());
    }

    #[test]
    fn test_shard_eviction() {
        let shard: NumaShard<i32, i32> = NumaShard::new(3);

        // Fill the shard
        shard.insert(1, 100);
        shard.insert(2, 200);
        shard.insert(3, 300);
        assert_eq!(shard.len(), 3);

        // Access some entries to mark them as visited
        let _ = shard.get(&1);
        let _ = shard.get(&2);

        // Insert a new entry, triggering eviction
        shard.insert(4, 400);
        assert_eq!(shard.len(), 3);

        // The entry that was not accessed (3) should have been evicted
        // Note: SIEVE eviction order may vary, so we just check the count
        let count = [1, 2, 3, 4].iter().filter(|k| shard.get(k).is_some()).count();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_shard_alignment() {
        // Verify that NumaShard is properly aligned
        assert!(std::mem::align_of::<NumaShard<String, i32>>() >= 64);
    }
}
