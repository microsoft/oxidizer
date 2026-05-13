// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache telemetry integration.
//!
//! This module provides telemetry recording for cache operations via the
//! `tracing` crate. Enable structured logging through the cache builder's
//! [`enable_logs()`](crate::CacheBuilder::enable_logs) method.
//!
//! Consumers can subscribe to cache events using a custom
//! [`tracing_subscriber::Layer`] and the public constants in [`attributes`].
//! See the `telemetry_subscriber` example for a complete demonstration.

pub mod attributes;
pub(crate) mod cache;
pub(crate) mod config;
pub(crate) mod ext;

#[doc(inline)]
pub use cache::CacheTelemetry;
pub(crate) use config::TelemetryConfig;
