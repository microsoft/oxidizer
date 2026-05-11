// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache telemetry integration with OpenTelemetry.
//!
//! This module provides telemetry recording for cache operations using
//! OpenTelemetry metrics and logs. Use [`TelemetryConfig`] to configure
//! logging and metrics for your cache.

pub mod attributes;
pub(crate) mod cache;
pub(crate) mod config;
pub(crate) mod ext;

#[doc(inline)]
pub use cache::CacheTelemetry;
pub(crate) use config::TelemetryConfig;
