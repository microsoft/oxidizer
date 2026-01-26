// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use crate::retry::Backoff;

/// Default backoff strategy: exponential backoff.
///
/// Exponential backoff quickly reduces request pressure after failures and
/// naturally spaces out subsequent attempts. This is the commonly recommended
/// choice for transient faults in distributed systems and pairs well with jitter
/// to avoid thundering herds.
pub(super) const DEFAULT_BACKOFF: Backoff = Backoff::Exponential;

/// Base delay for the backoff schedule; 10 milliseconds by default.
///
/// This default is optimized for **service-to-service** communication where low
/// latency is critical and transient failures are typically short-lived.
///
/// For **client-to-service** scenarios (e.g., mobile apps, web frontends), consider
/// increasing the base delay to 1-2 seconds to reduce load on potentially struggling
/// services and improve overall system stability.
pub(super) const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(10);

/// Enable jitter by default to de-synchronize clients and reduce contention.
///
/// Randomizing retry delays mitigates correlated bursts and improves tail
/// latency under contention. See [Exponential Backoff and Jitter](https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter) for details.
pub(super) const DEFAULT_USE_JITTER: bool = true;

/// Default maximum retry attempts: 3.
///
/// Three retry attempts is a widely used default in resilience libraries,
/// providing a balance between recovery opportunity and avoiding excessive delays.
pub(super) const DEFAULT_RETRY_ATTEMPTS: u32 = 3;
