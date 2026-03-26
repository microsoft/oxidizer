// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache builder types for constructing single and multi-tier caches.
//!
//! This module provides the builder pattern infrastructure for creating
//! caches with configurable storage, TTL, telemetry, and fallback tiers.

mod buildable;
mod cache;
mod fallback;
pub(crate) mod sealed;
mod transform;

pub(crate) use buildable::Buildable;
pub use cache::CacheBuilder;
pub use fallback::FallbackBuilder;
pub use sealed::CacheTierBuilder;
pub use transform::{Compressed, Encrypted, Serialized, TransformBuilder, Transformed};
