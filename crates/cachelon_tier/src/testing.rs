// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Mock cache implementation for testing.
//!
//! This module provides `MockCache`, a configurable in-memory cache that
//! records all operations and supports failure injection for testing error paths.

use std::{collections::HashMap, hash::Hash, sync::Arc};

use parking_lot::Mutex;

use crate::{CacheEntry, CacheTier, Error};

/// Recorded cache operation with full context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheOp<K, V> {
    /// A get operation was performed with the given key.
    Get(K),
    /// An insert operation was performed with the given key and entry.
    Insert {
        /// The key that was inserted.
        key: K,
        /// The cache entry that was inserted.
        entry: CacheEntry<V>,
    },
    /// An invalidate operation was performed with the given key.
    Invalidate(K),
    /// A clear operation was performed.
    Clear,
}

type FailPredicate<K, V> = Box<dyn Fn(&CacheOp<K, V>) -> bool + Send + Sync>;

/// A configurable mock cache for testing.
///
/// This cache stores values in memory and can be configured to fail
/// operations on demand, making it useful for testing error handling paths.
/// All operations are recorded for later verification.
///
/// # Examples
///
/// ```no_run
/// use cachelon_tier::{testing::{MockCache, CacheOp}, CacheTier, CacheEntry};
///
/// # async fn example() {
/// let cache = MockCache::<String, i32>::new();
///
/// // Insert and retrieve
/// cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
/// let value = cache.get(&"key".to_string()).await.unwrap();
/// assert_eq!(*value.unwrap().value(), 42);
///
/// // Verify operations
/// assert_eq!(cache.operations(), vec![
///     CacheOp::Insert { key: "key".to_string(), entry: CacheEntry::new(42) },
///     CacheOp::Get("key".to_string()),
/// ]);
/// # }
/// ```
///
/// # Failure Injection
///
/// ```no_run
/// use cachelon_tier::{testing::{MockCache, CacheOp}, CacheTier, CacheEntry};
///
/// # async fn example() {
/// let cache: MockCache<String, i32> = MockCache::new();
///
/// // Fail all get operations
/// cache.fail_when(|op| matches!(op, CacheOp::Get(_)));
/// assert!(cache.get(&"key".to_string()).await.is_err());
///
/// // Fail only specific keys
/// cache.fail_when(|op| matches!(op, CacheOp::Get(k) if k == "forbidden"));
/// assert!(cache.get(&"forbidden".to_string()).await.is_err());
/// assert!(cache.get(&"allowed".to_string()).await.is_ok());
/// # }
/// ```
pub struct MockCache<K, V> {
    data: Arc<Mutex<HashMap<K, CacheEntry<V>>>>,
    operations: Arc<Mutex<Vec<CacheOp<K, V>>>>,
    fail_when: Arc<Mutex<Option<FailPredicate<K, V>>>>,
}

impl<K, V> std::fmt::Debug for MockCache<K, V>
where
    K: std::fmt::Debug,
    V: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockCache")
            .field("data", &self.data)
            .field("operations", &self.operations)
            .field("fail_when", &self.fail_when.lock().is_some())
            .finish()
    }
}

impl<K, V> Clone for MockCache<K, V> {
    fn clone(&self) -> Self {
        Self {
            data: Arc::clone(&self.data),
            operations: Arc::clone(&self.operations),
            fail_when: Arc::clone(&self.fail_when),
        }
    }
}

impl<K, V> Default for MockCache<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> MockCache<K, V> {
    /// Creates a new empty mock cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(HashMap::new())),
            operations: Arc::new(Mutex::new(Vec::new())),
            fail_when: Arc::new(Mutex::new(None)),
        }
    }
}

impl<K, V> MockCache<K, V>
where
    K: Eq + Hash,
{
    /// Creates a mock cache with pre-populated data.
    #[must_use]
    pub fn with_data(data: HashMap<K, CacheEntry<V>>) -> Self {
        Self {
            data: Arc::new(Mutex::new(data)),
            operations: Arc::new(Mutex::new(Vec::new())),
            fail_when: Arc::new(Mutex::new(None)),
        }
    }

    /// Returns the number of entries in the cache.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.data.lock().len()
    }

    /// Returns true if the cache contains the given key.
    #[must_use]
    pub fn contains_key(&self, key: &K) -> bool {
        self.data.lock().contains_key(key)
    }
}

impl<K, V> MockCache<K, V>
where
    K: Clone,
    V: Clone,
{
    /// Sets a predicate that determines when `try_*` operations should fail.
    ///
    /// The predicate receives the operation and returns `true` if it should fail.
    /// This only affects the fallible `try_*` methods; infallible methods like
    /// `get()` and `insert()` always succeed.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon_tier::testing::{MockCache, CacheOp};
    ///
    /// let cache: MockCache<String, i32> = MockCache::new();
    ///
    /// // Fail all operations
    /// cache.fail_when(|_| true);
    ///
    /// // Fail only gets
    /// cache.fail_when(|op| matches!(op, CacheOp::Get(_)));
    ///
    /// // Fail gets for a specific key
    /// cache.fail_when(|op| matches!(op, CacheOp::Get(k) if k == "bad_key"));
    /// ```
    pub fn fail_when<F>(&self, predicate: F)
    where
        F: Fn(&CacheOp<K, V>) -> bool + Send + Sync + 'static,
    {
        *self.fail_when.lock() = Some(Box::new(predicate));
    }

    /// Clears the failure predicate, allowing all operations to succeed.
    pub fn clear_failures(&self) {
        *self.fail_when.lock() = None;
    }

    /// Returns a clone of all recorded operations.
    #[must_use]
    pub fn operations(&self) -> Vec<CacheOp<K, V>> {
        self.operations.lock().clone()
    }

    /// Clears all recorded operations.
    pub fn clear_operations(&self) {
        self.operations.lock().clear();
    }

    fn record(&self, op: CacheOp<K, V>) {
        self.operations.lock().push(op);
    }

    fn should_fail(&self, op: &CacheOp<K, V>) -> bool {
        self.fail_when.lock().as_ref().is_some_and(|predicate| predicate(op))
    }
}

impl<K, V> CacheTier<K, V> for MockCache<K, V>
where
    K: Clone + Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        let op = CacheOp::Get(key.clone());
        if self.should_fail(&op) {
            self.record(op);
            return Err(Error::caused_by("mock: get failed"));
        }
        self.record(op);
        Ok(self.data.lock().get(key).cloned())
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        let op = CacheOp::Insert {
            key: key.clone(),
            entry: entry.clone(),
        };
        if self.should_fail(&op) {
            self.record(op);
            return Err(Error::caused_by("mock: insert failed"));
        }
        self.record(op);
        self.data.lock().insert(key.clone(), entry);
        Ok(())
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        let op = CacheOp::Invalidate(key.clone());
        if self.should_fail(&op) {
            self.record(op);
            return Err(Error::caused_by("mock: invalidate failed"));
        }
        self.record(op);
        self.data.lock().remove(key);
        Ok(())
    }

    async fn clear(&self) -> Result<(), Error> {
        let op = CacheOp::Clear;
        if self.should_fail(&op) {
            self.record(op);
            return Err(Error::caused_by("mock: clear failed"));
        }
        self.record(op);
        self.data.lock().clear();
        Ok(())
    }

    fn len(&self) -> Option<u64> {
        Some(self.data.lock().len() as u64)
    }
}
