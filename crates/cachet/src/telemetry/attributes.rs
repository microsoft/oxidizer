// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Tracing target prefix for all cachet telemetry events.
///
/// All cachet telemetry events use the module path as the tracing target (e.g.,
/// `cachet::telemetry::cache`), which starts with this prefix. Consumers can
/// filter for all cachet events using prefix matching with `tracing_subscriber`:
/// ```ignore
/// use tracing_subscriber::filter;
/// let filter = filter::Targets::new()
///     .with_target(cachet::telemetry::attributes::TARGET, tracing::Level::DEBUG);
/// ```
pub const TARGET: &str = "cachet";

#[cfg(any(feature = "logs", feature = "metrics", test))]
pub(crate) const CACHE_NAME: &str = "cache.name";

#[cfg(any(feature = "logs", feature = "metrics", test))]
pub(crate) const CACHE_OPERATION_NAME: &str = "cache.operation";

#[cfg(any(feature = "logs", feature = "metrics", test))]
pub(crate) const CACHE_ACTIVITY_NAME: &str = "cache.activity";
