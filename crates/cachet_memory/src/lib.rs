// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! High-performance in-memory cache tier.
//!
//! This crate provides [`InMemoryCache`], a concurrent in-memory cache with a
//! `TinyLFU` eviction algorithm for excellent hit rates. Use [`InMemoryCacheBuilder`]
//! to configure capacity, TTL, and TTI.
//!
//! # Quick Start
//!
//! ```no_run
//! use cachet_memory::InMemoryCacheBuilder;
//! use cachet_tier::{CacheEntry, CacheTier};
//! use std::time::Duration;
//!
//! let cache = InMemoryCacheBuilder::<String, i32>::new()
//!     .max_capacity(1000)
//!     .time_to_live(Duration::from_secs(300))
//!     .build();
//!
//! cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
//! let value = cache.get(&"key".to_string()).await.unwrap();
//! assert_eq!(*value.unwrap().value(), 42);
//! ```
//!
//! # Features
//!
//! - **Capacity limits**: Set maximum entry count with automatic eviction
//! - **TTL/TTI**: Configure time-to-live and time-to-idle expiration
//! - **Thread-safe**: Safe for concurrent access from multiple tasks
//! - **Zero external types**: Builder API keeps implementation details private

mod builder;
mod tier;

#[doc(inline)]
pub use builder::InMemoryCacheBuilder;
#[doc(inline)]
pub use tier::InMemoryCache;
