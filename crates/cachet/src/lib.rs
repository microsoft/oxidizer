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
//! use cachet::{Cache, CacheEntry};
//! use tick::Clock;
//! # if cfg!(miri) { return; } // moka is incompatible with Miri
//! # futures::executor::block_on(async {
//!
//! let clock = Clock::new_frozen();
//! let cache = Cache::builder::<String, i32>(clock)
//!     .memory()
//!     .build();
//!
//! cache.insert("key", CacheEntry::new(42)).await?;
//! let value = cache.get("key").await?;
//! assert_eq!(*value.unwrap().value(), 42);
//! # Ok::<(), cachet::Error>(())
//! # });
//! ```
//!
//! ## Multi-Tier Cache with Fallback
//!
//! ```
//! use cachet::{Cache, CacheEntry, FallbackPromotionPolicy};
//! use tick::Clock;
//! use std::time::Duration;
//! # if cfg!(miri) { return; } // moka is incompatible with Miri
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
//!
//! # Telemetry
//!
//! Enable with `metrics` and/or `logs` features. Configure via `.use_metrics()` and `.use_logs()`.
//!
//! ## Metrics (OpenTelemetry)
//!
//! | Metric | Type | Unit | Description |
//! |--------|------|------|-------------|
//! | `cache.event.count` | Counter | event | Cache operation events |
//! | `cache.operation.duration_ns` | Histogram | s | Operation latency |
//! | `cache.size` | Gauge | entry | Current entry count |
//!
//! **Attributes:** `cache.name`, `cache.operation`, `cache.activity`
//!
//! **Operations:** `cache.get`, `cache.insert`, `cache.invalidate`, `cache.clear`
//!
//! **Activities:** `cache.hit`, `cache.miss`, `cache.expired`, `cache.inserted`,
//! `cache.invalidated`, `cache.refresh_hit`, `cache.refresh_miss`,
//! `cache.fallback_promotion`, `cache.error`, `cache.ok`
//!
//! ## Logs (tracing)
//!
//! Event name: `cache.event` with fields `cache.name`, `cache.operation`,
//! `cache.activity`, `cache.duration_ns`.
//!
//! **Levels:** DEBUG (hit/miss/ok), INFO (expired/inserted/invalidated/refresh), ERROR (error)

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
pub use cachet_memory::InMemoryCache;
#[cfg(feature = "service")]
#[doc(inline)]
pub use cachet_service::{CacheOperation, CacheResponse, CacheServiceExt, GetRequest, InsertRequest, InvalidateRequest, ServiceAdapter};
#[doc(inline)]
pub use cachet_tier::{CacheEntry, CacheTier, Error, Result};
#[cfg(feature = "dynamic-cache")]
#[doc(inline)]
pub use cachet_tier::{DynamicCache, DynamicCacheExt};
#[doc(inline)]
pub use fallback::FallbackPromotionPolicy;

#[cfg(any(feature = "test-util", test))]
#[doc(inline)]
pub use cachet_tier::testing::{CacheOp, MockCache};
