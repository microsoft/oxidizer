// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]

//! High-performance in-memory cache tier.
//!
//! This crate provides [`InMemoryCache`], a concurrent in-memory cache with
//! configurable eviction policies (`TinyLFU` by default) for excellent hit rates.
//! Use [`InMemoryCacheBuilder`] to configure capacity, TTL, TTI, and eviction policy.
//!
//! # Quick Start
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use cachet_memory::InMemoryCacheBuilder;
//! use cachet_tier::{CacheEntry, CacheTier};
//!
//! # async {
//!
//! let cache = InMemoryCacheBuilder::<String, i32>::new()
//!     .max_capacity(1000)
//!     .time_to_live(Duration::from_secs(300))
//!     .build()
//!     .expect("Failed to build cache");
//!
//! cache
//!     .insert("key".to_string(), CacheEntry::new(42))
//!     .await
//!     .unwrap();
//! let value = cache.get(&"key".to_string()).await.unwrap();
//! assert_eq!(*value.unwrap().value(), 42);
//! # };
//! ```
//!
//! # Features
//!
//! - **Capacity limits**: Set maximum entry count with automatic eviction
//! - **Eviction policies**: Choose between `TinyLFU` (default) and LRU via
//!   [`EvictionPolicy`](policy::EvictionPolicy)
//! - **TTL/TTI**: Configure time-to-live and time-to-idle expiration
//! - **Per-entry TTL**: Honors [`CacheEntry::expires_after`][cachet_tier::CacheEntry::expires_after]
//!   for per-entry expiration
//! - **Eviction notifications**: Observe removals via
//!   [`InMemoryCacheBuilder::on_eviction`] or opt into host-side telemetry with
//!   [`InMemoryCacheBuilder::with_eviction_telemetry`]
//! - **Thread-safe**: Safe for concurrent access from multiple tasks
//! - **Zero external types**: Builder API keeps implementation details private
//!
//! # Eviction Notifications
//!
//! Two complementary hooks are available for observing entry removals:
//!
//! - [`InMemoryCacheBuilder::on_eviction`] takes a closure invoked with a
//!   [`RemovalCause`] for every removal (capacity, expiry, explicit, or replace).
//!   Use this for custom side effects.
//! - [`InMemoryCacheBuilder::with_eviction_telemetry`] is a marker that the host
//!   crate (`cachet`) recognizes via `CacheBuilder::memory_with` and uses to
//!   install a built-in listener that emits `cache.eviction` for capacity
//!   removals and `cache.expired` for background TTL/TTI expiry. Explicit and
//!   replaced removals are intentionally not surfaced — they are already covered
//!   by the host's `cache.invalidated` and `cache.inserted` events. The marker
//!   has no effect when [`InMemoryCache`] is built directly without a host.
//!
//! # Expiration Behavior
//!
//! This tier supports three independent expiration mechanisms. When multiple are
//! active, the **shortest duration wins** - an entry is evicted at the earliest of:
//!
//! 1. The per-entry TTL from [`CacheEntry::expires_after`][cachet_tier::CacheEntry::expires_after]
//! 2. The cache-wide TTL from [`InMemoryCacheBuilder::time_to_live`]
//! 3. The cache-wide TTI from [`InMemoryCacheBuilder::time_to_idle`]
//!
//! This means the builder-level TTL/TTI acts as an **upper bound** on per-entry
//! TTL. A per-entry TTL longer than the builder TTL will be silently clamped to the
//! builder value. To give per-entry TTL full control, either leave the builder-level
//! TTL/TTI unset or set them to a sufficiently high ceiling.

mod builder;
pub mod notification;
pub mod policy;
mod tier;

#[doc(inline)]
pub use builder::InMemoryCacheBuilder;
#[doc(inline)]
pub use notification::RemovalCause;
#[doc(inline)]
pub use tier::InMemoryCache;
