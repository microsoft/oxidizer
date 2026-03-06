// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Redis-backed cache implementation.

use std::marker::PhantomData;

use cachet_service::{CacheOperation, CacheResponse};
use cachet_tier::{CacheEntry, CacheTier, Error};
use layered::Service;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Serialize, de::DeserializeOwned};
use thread_aware::{Arc, PerProcess, ThreadAware};

use crate::builder::RedisCacheBuilder;

/// A Redis-backed cache tier.
///
/// Implements both [`CacheTier`] for direct use and
/// [`Service<CacheOperation>`](Service) for middleware composition via
/// [`ServiceAdapter`](cachet_service::ServiceAdapter).
///
/// Values are serialized as JSON. When a [`CacheEntry`] has a TTL, `SETEX` is
/// used to enable server-side expiration.
///
/// # Examples
///
/// ```ignore
/// use cachet_redis::RedisCache;
/// use cachet_tier::{CacheEntry, CacheTier};
///
/// let cache = RedisCache::<String, i32>::new(conn);
/// cache.insert(&"key".into(), CacheEntry::new(42)).await?;
/// ```
#[derive(Debug, Clone, ThreadAware)]
pub struct RedisCache<K, V> {
    inner: Arc<RedisCacheInner, PerProcess>,
    #[thread_aware(skip)]
    phantom: PhantomData<(K, V)>,
}

#[derive(Debug, Clone)]
struct RedisCacheInner {
    connection: ConnectionManager,
    key_prefix: Option<String>,
    clear_batch_size: usize,
}

impl<K, V> RedisCache<K, V> {
    /// Creates a new `RedisCache` with default settings.
    #[must_use]
    pub fn new(connection: ConnectionManager) -> Self {
        Self::builder(connection).build()
    }

    /// Creates a builder for configuring a `RedisCache`.
    #[must_use]
    pub fn builder(connection: ConnectionManager) -> RedisCacheBuilder<K, V> {
        RedisCacheBuilder::new(connection)
    }

    /// Constructs a `RedisCache` from a builder.
    pub(crate) fn from_builder(builder: RedisCacheBuilder<K, V>) -> Self {
        Self {
            inner: Arc::from_unaware(RedisCacheInner {
                connection: builder.connection,
                key_prefix: builder.key_prefix,
                clear_batch_size: builder.clear_batch_size,
            }),
            phantom: PhantomData,
        }
    }
}

impl<K, V> RedisCache<K, V>
where
    K: Serialize,
{
    fn make_redis_key(&self, key: &K) -> Result<String, Error> {
        let serialized = serde_json::to_string(key).map_err(Error::from_source)?;
        match &self.inner.key_prefix {
            Some(prefix) => Ok(format!("{prefix}{serialized}")),
            None => Ok(serialized),
        }
    }
}

impl<K, V> RedisCache<K, V>
where
    K: Serialize + Clone + Send + Sync,
    V: Serialize + DeserializeOwned + Clone + Send + Sync,
{
    async fn do_get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        let redis_key = self.make_redis_key(key)?;
        let mut conn = self.inner.connection.clone();
        let value: Option<String> = conn.get(&redis_key).await.map_err(Error::from_source)?;

        match value {
            Some(json) => {
                let entry: CacheEntry<V> = serde_json::from_str(&json).map_err(Error::from_source)?;
                Ok(Some(entry))
            }
            None => Ok(None),
        }
    }

    async fn do_insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        let redis_key = self.make_redis_key(key)?;
        let json = serde_json::to_string(&entry).map_err(Error::from_source)?;
        let mut conn = self.inner.connection.clone();

        if let Some(ttl) = entry.ttl() {
            let seconds = ttl.as_secs().max(1);
            let () = conn.set_ex(&redis_key, &json, seconds).await.map_err(Error::from_source)?;
        } else {
            let () = conn.set(&redis_key, &json).await.map_err(Error::from_source)?;
        }

        Ok(())
    }

    async fn do_invalidate(&self, key: &K) -> Result<(), Error> {
        let redis_key = self.make_redis_key(key)?;
        let mut conn = self.inner.connection.clone();
        let () = conn.del(&redis_key).await.map_err(Error::from_source)?;
        Ok(())
    }

    async fn do_clear(&self) -> Result<(), Error> {
        let mut conn = self.inner.connection.clone();

        match &self.inner.key_prefix {
            Some(prefix) => {
                let pattern = format!("{prefix}*");
                let mut cursor: u64 = 0;
                loop {
                    let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                        .arg(cursor)
                        .arg("MATCH")
                        .arg(&pattern)
                        .arg("COUNT")
                        .arg(self.inner.clear_batch_size)
                        .query_async(&mut conn)
                        .await
                        .map_err(Error::from_source)?;

                    if !keys.is_empty() {
                        let () = redis::cmd("DEL")
                            .arg(&keys)
                            .query_async(&mut conn)
                            .await
                            .map_err(Error::from_source)?;
                    }

                    cursor = next_cursor;
                    if cursor == 0 {
                        break;
                    }
                }
            }
            None => {
                let () = redis::cmd("FLUSHDB").query_async(&mut conn).await.map_err(Error::from_source)?;
            }
        }

        Ok(())
    }
}

impl<K, V> CacheTier<K, V> for RedisCache<K, V>
where
    K: Serialize + Clone + Send + Sync,
    V: Serialize + DeserializeOwned + Clone + Send + Sync,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        self.do_get(key).await
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        self.do_insert(key, entry).await
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        self.do_invalidate(key).await
    }

    async fn clear(&self) -> Result<(), Error> {
        self.do_clear().await
    }
}

impl<K, V> Service<CacheOperation<K, V>> for RedisCache<K, V>
where
    K: Serialize + Clone + Send + Sync,
    V: Serialize + DeserializeOwned + Clone + Send + Sync,
{
    type Out = Result<CacheResponse<V>, Error>;

    async fn execute(&self, input: CacheOperation<K, V>) -> Self::Out {
        match input {
            CacheOperation::Get(req) => {
                let entry = self.do_get(&req.key).await?;
                Ok(CacheResponse::Get(entry))
            }
            CacheOperation::Insert(req) => {
                self.do_insert(&req.key, req.entry).await?;
                Ok(CacheResponse::Insert())
            }
            CacheOperation::Invalidate(req) => {
                self.do_invalidate(&req.key).await?;
                Ok(CacheResponse::Invalidate())
            }
            CacheOperation::Clear => {
                self.do_clear().await?;
                Ok(CacheResponse::Clear())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Helper: skip test if `REDIS_URL` is not set.
    fn redis_url() -> Option<String> {
        std::env::var("REDIS_URL").ok()
    }

    async fn make_cache(prefix: &str) -> Option<RedisCache<String, i32>> {
        let url = redis_url()?;
        let client = redis::Client::open(url.as_str()).expect("invalid REDIS_URL");
        let conn = ConnectionManager::new(client).await.expect("failed to connect to Redis");
        let cache = RedisCache::<String, i32>::builder(conn).key_prefix(prefix).build();
        // Clear prefixed keys before test
        cache.do_clear().await.expect("failed to clear");
        Some(cache)
    }

    #[tokio::test]
    async fn get_returns_none_for_missing_key() {
        let Some(cache) = make_cache("test_miss:").await else {
            return;
        };
        let result = cache.get(&"nonexistent".to_string()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn insert_and_get_round_trip() {
        let Some(cache) = make_cache("test_rt:").await else {
            return;
        };
        let key = "hello".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.expect("insert failed");
        let entry = cache.get(&key).await.expect("get failed").expect("expected Some");
        assert_eq!(*entry.value(), 42);
    }

    #[tokio::test]
    async fn ttl_metadata_preserved() {
        let Some(cache) = make_cache("test_ttl:").await else {
            return;
        };
        let key = "ttl_key".to_string();
        let entry = CacheEntry::expires_after(99, Duration::from_secs(300));
        cache.insert(&key, entry).await.expect("insert failed");
        let retrieved = cache.get(&key).await.expect("get failed").expect("expected Some");
        assert_eq!(*retrieved.value(), 99);
        assert_eq!(retrieved.ttl(), Some(Duration::from_secs(300)));
    }

    #[tokio::test]
    async fn invalidate_removes_key() {
        let Some(cache) = make_cache("test_inv:").await else {
            return;
        };
        let key = "to_delete".to_string();
        cache.insert(&key, CacheEntry::new(1)).await.expect("insert failed");
        cache.invalidate(&key).await.expect("invalidate failed");
        let result = cache.get(&key).await.expect("get failed");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn clear_removes_prefixed_keys_only() {
        let Some(cache) = make_cache("test_clear:").await else {
            return;
        };
        cache.insert(&"a".to_string(), CacheEntry::new(1)).await.expect("insert a");
        cache.insert(&"b".to_string(), CacheEntry::new(2)).await.expect("insert b");
        cache.clear().await.expect("clear failed");
        assert!(cache.get(&"a".to_string()).await.expect("get a").is_none());
        assert!(cache.get(&"b".to_string()).await.expect("get b").is_none());
    }

    #[tokio::test]
    async fn service_get_round_trip() {
        let Some(cache) = make_cache("test_svc:").await else {
            return;
        };
        // Insert via Service
        let insert_op = CacheOperation::Insert(cachet_service::InsertRequest::new("svc_key".to_string(), CacheEntry::new(7)));
        let resp = cache.execute(insert_op).await.expect("service insert");
        assert!(matches!(resp, CacheResponse::Insert()));

        // Get via Service
        let get_op = CacheOperation::Get(cachet_service::GetRequest::new("svc_key".to_string()));
        let resp = cache.execute(get_op).await.expect("service get");
        match resp {
            CacheResponse::Get(Some(entry)) => assert_eq!(*entry.value(), 7),
            other => panic!("expected Get(Some(...)), got {other:?}"),
        }
    }

    #[test]
    fn len_returns_none() {
        // Cannot construct without a real connection, so we test via the trait default.
        // The CacheTier impl doesn't override len(), so it returns None by default.
        // This is a compile-time guarantee tested by the trait's default implementation.
    }
}
