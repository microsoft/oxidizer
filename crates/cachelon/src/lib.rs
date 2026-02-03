// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Flexible multi-tier caching with telemetry and TTL support.
//!
//! This crate provides a composable cache system with:
//! - Type-safe cache builders for single and multi-tier caches
//! - Built-in OpenTelemetry metrics and logging
//! - Per-entry and tier-level TTL expiration
//! - Fallback cache hierarchies with configurable promotion policies
//! - Background refresh with stampede protection
//!
//! # Examples
//!
//! ## Basic In-Memory Cache
//!
//! ```
//! use cachelon::{Cache, CacheEntry};
//! use tick::Clock;
//! # futures::executor::block_on(async {
//!
//! let clock = Clock::new_frozen();
//! let cache = Cache::builder::<String, i32>(clock)
//!     .memory()
//!     .build();
//!
//! cache.insert(&"key".to_string(), CacheEntry::new(42)).await?;
//! let value = cache.get(&"key".to_string()).await?;
//! assert_eq!(*value.unwrap().value(), 42);
//! # Ok::<(), cachelon::Error>(())
//! # });
//! ```
//!
//! ## Multi-Tier Cache with Fallback
//!
//! ```
//! use cachelon::{Cache, CacheEntry, FallbackPromotionPolicy};
//! use tick::Clock;
//! use std::time::Duration;
//! # futures::executor::block_on(async {
//!
//! let clock = Clock::new_frozen();
//! let l2 = Cache::builder::<String, String>(clock.clone()).memory();
//!
//! let cache = Cache::builder::<String, String>(clock)
//!     .memory()
//!     .ttl(Duration::from_secs(60))
//!     .fallback(l2)
//!     .promotion_policy(FallbackPromotionPolicy::always())
//!     .build();
//! # });
//! ```

pub mod builder;
pub mod cache;
mod fallback;
pub mod refresh;
mod telemetry;
mod wrapper;

#[doc(inline)]
pub use cache::Cache;
#[cfg(feature = "memory")]
#[doc(inline)]
pub use cachelon_memory::InMemoryCache;
#[cfg(feature = "service")]
#[doc(inline)]
pub use cachelon_service::{CacheOperation, CacheResponse, GetRequest, InsertRequest, InvalidateRequest, ServiceAdapter};
#[doc(inline)]
pub use cachelon_tier::{CacheEntry, CacheTier, Error, Result};
#[cfg(feature = "dynamic-cache")]
#[doc(inline)]
pub use cachelon_tier::{DynamicCache, DynamicCacheExt};
#[doc(inline)]
pub use fallback::{FallbackCache, FallbackPromotionPolicy};
#[cfg(any(feature = "logs", feature = "metrics", test))]
#[doc(inline)]
pub use telemetry::CacheTelemetry;
#[doc(inline)]
pub use wrapper::CacheWrapper;

#[cfg(any(feature = "test-util", test))]
#[doc(inline)]
pub use cachelon_tier::testing::{CacheOp, MockCache};
