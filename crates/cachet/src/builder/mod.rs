// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache builder types for constructing single and multi-tier caches.
//!
//! This module provides the builder pattern infrastructure for creating
//! caches with configurable storage, TTL, telemetry, and fallback tiers.

mod buildable;
mod cache;
#[cfg(feature = "encrypt")]
mod encrypt;
mod fallback;
mod sealed;
#[cfg(any(feature = "serialize", test))]
mod serialize;
mod transform;

pub use cache::CacheBuilder;
#[cfg(feature = "encrypt")]
pub use encrypt::ProtectedTransformBuilder;
pub use fallback::FallbackBuilder;
pub use sealed::CacheTierBuilder;
pub use transform::TransformBuilder;
