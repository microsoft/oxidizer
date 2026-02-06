// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache telemetry integration with OpenTelemetry.
//!
//! This module provides telemetry recording for cache operations using
//! OpenTelemetry metrics and logs. Use [`TelemetryConfig`] to configure
//! logging and metrics for your cache.

pub(crate) mod attributes;
pub(crate) mod cache;
pub(crate) mod config;
pub(crate) mod ext;
#[cfg(any(feature = "metrics", test))]
pub(crate) mod metrics;

pub use cache::CacheTelemetry;
pub(crate) use cache::{CacheActivity, CacheOperation};
pub(crate) use config::TelemetryConfig;
