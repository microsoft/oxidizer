// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Service pattern integration for cache backends.
//!
//! This crate provides [`ServiceAdapter`] to convert any `Service<CacheOperation>` into
//! a [`CacheTier`](cachelon_tier::CacheTier), enabling service middleware composition
//! (retry, timeout, circuit breaker) for cache storage backends.
//!
//! # Overview
//!
//! The adapter provides bidirectional integration:
//! - **Service → Cache**: Use [`ServiceAdapter`] to wrap services as cache tiers
//! - **Cache → Service**: The main `cachelon::Cache` implements `Service<CacheOperation>`
//!
//! # Quick Start
//!
//! ```ignore
//! // Any Service<CacheOperation> can become a cache tier
//! let tier = ServiceAdapter::new(my_service);
//! ```
//!
//! # Use Cases
//!
//! - **Remote caches**: Wrap Redis, Memcached, or custom services as cache tiers
//! - **Middleware composition**: Add retry, timeout, or circuit breaker before caching
//! - **Unified abstractions**: Use the same service patterns for caching and other I/O

pub mod adapter;
pub mod request;

#[doc(inline)]
pub use adapter::ServiceAdapter;
#[doc(inline)]
pub use request::{CacheOperation, CacheResponse, GetRequest, InsertRequest, InvalidateRequest};
