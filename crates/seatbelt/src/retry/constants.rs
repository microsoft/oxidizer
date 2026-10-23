// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use crate::retry::Backoff;

/// Uses exponential backoff by default.
///
/// Exponential backoff reduces request pressure after failures.
/// Jitter prevents many clients from retrying at the same time.
pub(super) const DEFAULT_BACKOFF: Backoff = Backoff::Exponential;

/// Sets the default base delay to 10 milliseconds.
///
/// This value is for low-latency service-to-service calls with short failures.
///
/// For client-to-service calls, use 1 to 2 seconds to reduce load on a failing service.
pub(super) const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(10);

/// Enables jitter by default so clients retry at different times.
///
/// Random retry delays reduce simultaneous retry bursts.
/// See [Exponential Backoff and Jitter](https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter).
pub(super) const DEFAULT_USE_JITTER: bool = true;

/// Sets the default maximum to three retry attempts.
///
/// Three retries provide time to recover without a long delay.
pub(super) const DEFAULT_RETRY_ATTEMPTS: u32 = 3;
