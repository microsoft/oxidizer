// Copyright (c) Microsoft Corporation.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Core cache tier abstractions for building cache backends.
//!
//! This crate defines the [`CacheTier`] trait that all cache implementations must satisfy,
//! along with [`CacheEntry`] for storing values with metadata and [`Error`] types for
//! fallible operations.
//!
//! # Overview
//!
//! The cache tier abstraction separates storage concerns from caching features. Implement
//! [`CacheTier`] for your storage backend, then use `cachelon` to add telemetry, TTL,
//! multi-tier fallback, and other features on top.
//!
//! # Implementing a Cache Tier
//!
//! Implement all required methods of [`CacheTier`]:
//!
//! ```
//! use cachelon_tier::{CacheEntry, CacheTier, Error};
//! use std::collections::HashMap;
//! use std::sync::RwLock;
//!
//! struct SimpleCache<K, V>(RwLock<HashMap<K, CacheEntry<V>>>);
//!
//! impl<K, V> CacheTier<K, V> for SimpleCache<K, V>
//! where
//!     K: Clone + Eq + std::hash::Hash + Send + Sync,
//!     V: Clone + Send + Sync,
//! {
//!     async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
//!         Ok(self.0.read().unwrap().get(key).cloned())
//!     }
//!
//!     async fn insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
//!         self.0.write().unwrap().insert(key.clone(), entry);
//!         Ok(())
//!     }
//!
//!     async fn invalidate(&self, key: &K) -> Result<(), Error> {
//!         self.0.write().unwrap().remove(key);
//!         Ok(())
//!     }
//!
//!     async fn clear(&self) -> Result<(), Error> {
//!         self.0.write().unwrap().clear();
//!         Ok(())
//!     }
//! }
//! ```
//!
//! # Dynamic Dispatch
//!
//! Enable the `dynamic-cache` feature for [`DynamicCache`], which wraps any `CacheTier`
//! in a type-erased container. This is useful for multi-tier caches with heterogeneous
//! storage backends.

mod entry;
pub mod error;
#[cfg(any(feature = "test-util", test))]
pub mod testing;
pub(crate) mod tier;

#[cfg(any(test, feature = "dynamic-cache"))]
mod dynamic;

#[cfg(any(test, feature = "dynamic-cache"))]
#[doc(inline)]
pub use dynamic::{DynamicCache, DynamicCacheExt};
#[doc(inline)]
pub use entry::CacheEntry;
#[doc(inline)]
pub use error::{Error, Result};
#[doc(inline)]
pub use tier::CacheTier;
