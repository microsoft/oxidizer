// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder for configuring a Redis-backed cache.

use std::marker::PhantomData;

use redis::aio::ConnectionManager;

use crate::cache::RedisCache;

/// Builder for configuring a [`RedisCache`].
///
/// # Examples
///
/// ```ignore
/// use cachet_redis::RedisCache;
///
/// let cache = RedisCache::<String, i32>::builder(conn)
///     .key_prefix("myapp:")
///     .clear_batch_size(200)
///     .build();
/// ```
#[derive(Debug)]
pub struct RedisCacheBuilder<K, V> {
    pub(crate) connection: ConnectionManager,
    pub(crate) key_prefix: Option<String>,
    pub(crate) clear_batch_size: usize,
    _phantom: PhantomData<(K, V)>,
}

impl<K, V> RedisCacheBuilder<K, V> {
    /// Creates a new builder with the given Redis connection manager.
    #[must_use]
    pub fn new(connection: ConnectionManager) -> Self {
        Self {
            connection,
            key_prefix: None,
            clear_batch_size: 100,
            _phantom: PhantomData,
        }
    }

    /// Sets a key prefix for all Redis keys managed by this cache.
    ///
    /// When set, all keys are prefixed with this string (e.g., `"myapp:"`).
    /// This enables safe [`clear()`](cachet_tier::CacheTier::clear) via `SCAN` +
    /// `DEL` of only prefixed keys.
    ///
    /// Without a prefix, `clear()` uses `FLUSHDB`, which removes **all** keys
    /// in the current database.
    #[must_use]
    pub fn key_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.key_prefix = Some(prefix.into());
        self
    }

    /// Sets the batch size for `SCAN` during `clear()` with a key prefix.
    ///
    /// Defaults to `100`. Larger values reduce round-trips but increase
    /// per-batch memory usage.
    #[must_use]
    pub fn clear_batch_size(mut self, size: usize) -> Self {
        self.clear_batch_size = size;
        self
    }

    /// Builds the configured [`RedisCache`].
    #[must_use]
    pub fn build(self) -> RedisCache<K, V> {
        RedisCache::from_builder(self)
    }
}
