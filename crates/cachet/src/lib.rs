// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! A composable, multi-tier caching library with stampede protection, background
//! refresh, and built-in OpenTelemetry telemetry.
//!
//! # Why Multi-Tier Caching?
//!
//! A single cache is a single point of failure and a capacity ceiling. Multi-tier
//! caching layers fast, small caches in front of slower, larger ones:
//!
//!  - **L1 (primary)** - an in-process memory cache: microsecond latency, bounded
//!    capacity, evicts under pressure.
//!  - **L2 (fallback)** - a remote or larger cache: millisecond latency, much larger
//!    capacity, survives process restarts.
//!
//! On a miss in L1, `cachet` transparently queries L2 and optionally *promotes* the
//! value back into L1 so the next request is fast again. The result is lower average
//! latency, reduced load on the backing store, and resilience when either tier is
//! temporarily unavailable.
//!
//! # Why Background Refresh?
//!
//! TTL-based expiration causes a *synchronous* miss every time an entry ages out:
//! the next caller blocks while the value is recomputed. Background refresh
//! (time-to-refresh, TTR) decouples freshness from latency:
//!
//! - While an entry is still within its TTR, all callers receive the cached value
//!   immediately (a "refresh hit").
//! - Once the TTR elapses, the *next* caller still receives the stale value, but a
//!   background task is spawned to pull a fresh value from the fallback tier.
//! - Subsequent callers continue to hit the cache while the refresh happens, so
//!   latency never spikes.
//!
//! Use [`TimeToRefresh`] together with a fallback tier to enable this pattern.
//!
//! # Cache Stampede Protection
//!
//! A **cache stampede** (also called a thundering herd) occurs when many concurrent
//! requests all miss the cache at the same time - for example, after a cold start or
//! after a popular entry expires. Every request independently computes the value,
//! spiking load on the backing store.
//!
//! `cachet` avoids this with *request coalescing* via the [`uniflight`] crate: when
//! stampede protection is enabled, all concurrent requests for the same key are merged
//! so that only one computes the value. The rest wait and share the result, including
//! any error. Enable it with [`CacheBuilder::stampede_protection`].
//!
//! # Flexibility
//!
//! `cachet` is designed to adapt to your infrastructure rather than the other way
//! around:
//!
//! - **Any storage backend** - implement [`CacheTier`] to plug in Redis, Memcached,
//!   a database, or any other store.
//! - **Service middleware** - with the `service` feature, any
//!   `Service<CacheOperation>` becomes a `CacheTier`, so you can compose retry,
//!   timeout, and circuit-breaker middleware around your storage using standard Tower
//!   or `layered` patterns.
//! - **Dynamic dispatch** - when a fallback tier is configured, the builder
//!   automatically type-erases both tiers into a [`DynamicCache<K, V>`] so
//!   the primary and fallback don't need to be the same concrete type.
//! - **Configurable promotion** - choose whether, and under what conditions, values
//!   found in a fallback tier are promoted back into the primary tier
//!   ([`FallbackPromotionPolicy`]).
//! - **Clock injection** - all time-based logic (TTL, TTR, timestamps) goes through
//!   a [`tick::Clock`], making caches fully controllable in tests without sleeping.
//!
//! # Why Use This Instead of Moka/Other Caches?
//!
//! Moka (and similar crates) are excellent single-tier in-process caches. `cachet`
//! builds on top of them and adds:
//!
//! | Feature | Moka | `cachet` |
//! |---|---|---|
//! | In-process memory cache | ✅ | ✅ (via `cachet_memory`) |
//! | Multi-tier / fallback | ❌ | ✅ |
//! | Stampede protection | ❌ | ✅ |
//! | Background refresh | ❌ | ✅ |
//! | Service middleware integration | ❌ | ✅ |
//! | OpenTelemetry metrics + logs | ❌ | ✅ |
//! | Pluggable storage backends | ❌ | ✅ |
//! | Clock injection for testing | ❌ | ✅ |
//!
//! If you only need a single in-process cache with no telemetry requirements,
//! `moka` directly may be simpler. If you need any of the above, `cachet` is the
//! right choice.
//!
//! # Major Types
//!
//! | Type | Description |
//! |---|---|
//! | [`Cache`] | The user-facing cache. Wraps any `CacheTier` with `get`, `insert`, `invalidate`, `clear`, `get_or_insert`, `try_get_or_insert`, and `optionally_get_or_insert`. |
//! | [`CacheBuilder`] | Builder for `Cache`. Configure storage, TTL, name, telemetry, fallback, promotion policy, stampede protection, and background refresh. |
//! | [`CacheEntry<V>`] | A value together with an optional cached-at timestamp and TTL. Returned by all `get` operations. |
//! | [`CacheTier`] | The core trait for storage backends. Implement this to add your own storage. |
//! | [`FallbackPromotionPolicy`] | Decides whether a value found in a fallback tier is promoted to the primary tier. |
//! | [`TimeToRefresh`] | Configures background refresh: how stale an entry must be before a background task refreshes it. |
//! | [`Error`] | The error type returned by all fallible cache operations. |
//!
//! # How Tiers Compose
//!
//! Tiers are composed at build time using the builder:
//!
//! ```text
//! Cache::builder::<K, V>(clock)
//!     .memory()                          // L1: fast in-process store
//!     .ttl(Duration::from_secs(30))      // entries expire from L1 after 30 s
//!     .fallback(                         // on L1 miss, consult L2
//!         Cache::builder::<K, V>(clock)
//!             .memory()                  // L2: a second in-process store (or a remote service)
//!             .ttl(Duration::from_secs(300))
//!     )
//!     .promotion_policy(FallbackPromotionPolicy::always())  // promote L2 hits into L1
//!     .time_to_refresh(TimeToRefresh::new(Duration::from_secs(20), spawner))  // refresh L1 in background
//!     .build()
//! ```
//!
//! On a `get`:
//! 1. Check L1. If hit and not stale, return immediately.
//! 2. If hit but stale (TTR elapsed), return the stale value *and* spawn a background
//!    task to fetch from L2 and repopulate L1.
//! 3. If miss or expired (TTL elapsed), check L2. If found, optionally promote to L1,
//!    then return.
//! 4. If both miss, return `Ok(None)`.
//!
//! **Note:** expired entries are not automatically removed from storage. The wrapper
//! uses lazy expiration - it returns `None` but leaves cleanup to the storage
//! backend (e.g. moka built-in eviction).
//!
//! TO-DO add an `ExpirationPolicy` that would make this configurable.
//!
//! Invalidation and clear are sent to **all** tiers concurrently.
//!
//! # Companion Crates
//!
//! `cachet` is the main entry point. The ecosystem is split into focused crates:
//!
//! | Crate | Purpose |
//! |---|---|
//! | [`cachet_tier`] | Core `CacheTier` trait, `CacheEntry`, `Error`, and `MockCache` for testing. |
//! | [`cachet_memory`] | In-process memory cache backed by [moka](https://docs.rs/moka) (`TinyLFU` eviction). |
//! | [`cachet_service`] | Adapters between the `CacheTier` trait and the `layered::Service` / Tower service patterns. |
//!
//! You rarely need to depend on companion crates directly - `cachet` re-exports the
//! most commonly used types from all of them.
//!
//! # Cargo Features
//!
//! | Feature | Default | Description |
//! |---|---|---|
//! | `memory` | ✅ | Enables `InMemoryCache` and the `.memory()` builder method via `cachet_memory`. |
//! | `metrics` | ❌ | Enables OpenTelemetry metrics (`cache.event.count`, `cache.operation.duration`, `cache.size`). |
//! | `logs` | ❌ | Enables structured `tracing` log events for every cache activity. |
//! | `service` | ❌ | Enables `ServiceAdapter`, `CacheServiceExt`, and `CacheOperation`/`CacheResponse` types for service middleware integration. |
//! | `test-util` | ❌ | Enables `MockCache`, frozen-clock utilities, and other test helpers. |
//!
//! # Examples
//!
//! ## Basic In-Memory Cache
//!
//! ```no_run
//! use cachet::{Cache, CacheEntry};
//! use tick::Clock;
//! # async {
//!
//! let clock = Clock::new_tokio();
//! let cache = Cache::builder::<String, i32>(clock).memory().build();
//!
//! cache.insert("key".to_string(), CacheEntry::new(42)).await?;
//! let value = cache.get("key").await?;
//! assert_eq!(*value.unwrap().value(), 42);
//! # Ok::<(), cachet::Error>(())
//! # };
//! ```
//!
//! ## Multi-Tier Cache with Fallback
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use cachet::{Cache, CacheEntry, FallbackPromotionPolicy};
//! use tick::Clock;
//! # async {
//!
//! let clock = Clock::new_tokio();
//! let l2 = Cache::builder::<String, String>(clock.clone()).memory();
//!
//! let cache = Cache::builder::<String, String>(clock)
//!     .memory()
//!     .ttl(Duration::from_secs(60))
//!     .fallback(l2)
//!     .promotion_policy(FallbackPromotionPolicy::always())
//!     .build();
//! # };
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
//! | `cache.operation.duration` | Histogram | s | Operation latency |
//! | `cache.size` | Gauge | entry | Current entry count |
//!
//! **Attributes:** `cache.name`, `cache.operation`, `cache.activity`
//!
//! **Operations:** `cache.get`, `cache.insert`, `cache.invalidate`, `cache.clear`
//!
//! **Activities:** `cache.hit`, `cache.miss`, `cache.expired`, `cache.inserted`,
//! `cache.invalidated`, `cache.refresh_hit`, `cache.refresh_miss`,
//! `cache.fallback`, `cache.fallback_promotion`, `cache.error`, `cache.ok`
//!
//! ## Logs (tracing)
//!
//! Event name: `cache.event` with fields `cache.name`, `cache.operation`,
//! `cache.activity`, `cache.duration_ns`.
//!
//! | Level | Activities |
//! |-------|-----------|
//! | ERROR | `cache.error` |
//! | INFO  | `cache.expired`, `cache.refresh_miss`, `cache.inserted`, `cache.invalidated`, `cache.fallback`, `cache.fallback_promotion` |
//! | DEBUG | `cache.hit`, `cache.miss`, `cache.refresh_hit`, `cache.ok` |

mod builder;
mod cache;
mod fallback;
mod refresh;
mod telemetry;
mod wrapper;

#[doc(inline)]
pub use builder::{CacheBuilder, CacheTierBuilder, FallbackBuilder};
#[doc(inline)]
pub use cache::{Cache, CacheName};
#[cfg(feature = "memory")]
#[doc(inline)]
pub use cachet_memory::InMemoryCache;
#[cfg(feature = "service")]
#[doc(inline)]
pub use cachet_service::{CacheOperation, CacheResponse, CacheServiceExt, GetRequest, InsertRequest, InvalidateRequest, ServiceAdapter};
#[doc(inline)]
pub use cachet_tier::DynamicCache;
#[doc(inline)]
pub use cachet_tier::{CacheEntry, CacheTier, Error, Result};
#[cfg(any(feature = "test-util", test))]
#[doc(inline)]
pub use cachet_tier::{CacheOp, MockCache};
#[doc(inline)]
pub use fallback::FallbackPromotionPolicy;
#[doc(inline)]
pub use refresh::TimeToRefresh;
