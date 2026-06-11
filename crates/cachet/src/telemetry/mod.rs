// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache telemetry integration.
//!
//! This module provides telemetry recording for cache operations via the
//! `tracing` crate and the [`handler`] callback API. Enable structured logging
//! through the cache builder's `enable_logs()` method.
//!
//! Consumers can subscribe to cache events using a custom
//! `tracing_subscriber::Layer` and the public constants in [`attributes`], or
//! register a [`handler::CacheEventHandler`] with the cache builder.
//! See the `telemetry_subscriber` example for a complete demonstration.

pub mod attributes;
pub(crate) mod cache;
/// Callback-based telemetry handlers.
pub mod handler;

#[doc(inline)]
pub use cache::CacheTelemetry;
