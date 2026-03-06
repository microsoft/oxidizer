// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Redis-backed cache tier for the cachet caching framework.
//!
//! This crate provides [`RedisCache`], a cache tier backed by Redis using the
//! [`redis`] crate's [`ConnectionManager`](redis::aio::ConnectionManager) for
//! automatic reconnection. Values are serialized as JSON via `serde_json`.
//!
//! # Quick Start
//!
//! ```ignore
//! use cachet_redis::RedisCache;
//! use cachet_tier::{CacheEntry, CacheTier};
//!
//! let client = redis::Client::open("redis://127.0.0.1/")?;
//! let conn = redis::aio::ConnectionManager::new(client).await?;
//!
//! let cache = RedisCache::<String, i32>::builder(conn)
//!     .key_prefix("myapp:")
//!     .build();
//!
//! cache.insert(&"key".into(), CacheEntry::new(42)).await?;
//! let entry = cache.get(&"key".into()).await?;
//! assert_eq!(entry.map(|e| *e.value()), Some(42));
//! ```
//!
//! # Service Integration
//!
//! `RedisCache` also implements [`Service<CacheOperation>`](layered::Service) so it
//! can be composed with middleware (retry, timeout, circuit breakers) via
//! [`ServiceAdapter`](cachet_service::ServiceAdapter).
//!
//! # Key and Value Serialization
//!
//! Keys are serialized to JSON strings via `serde_json::to_string` and optionally
//! prefixed. Values (as [`CacheEntry`](cachet_tier::CacheEntry)) are serialized to
//! JSON. When a [`CacheEntry`](cachet_tier::CacheEntry) has a TTL, Redis `SETEX` is
//! used for server-side expiration.

mod builder;
mod cache;

#[doc(inline)]
pub use builder::RedisCacheBuilder;
#[doc(inline)]
pub use cache::RedisCache;
