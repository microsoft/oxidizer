// Copyright (c) Microsoft Corporation.

use std::time::Duration;

use crate::Backoff;

/// Default backoff strategy: exponential backoff.
///
/// Exponential backoff quickly reduces request pressure after failures and
/// naturally spaces out subsequent attempts. This is the commonly recommended
/// choice for transient faults in distributed systems and pairs well with jitter
/// to avoid thundering herds.
pub(super) const DEFAULT_BACKOFF: Backoff = Backoff::Exponential;

/// Base delay for the backoff schedule; conservative 2 seconds by default.
///
/// A 2s starting delay prevents aggressive retry storms during partial outages
/// while still enabling fast recovery for short-lived failures. Workloads with
/// different needs can override this via configuration.
pub(super) const DEFAULT_BASE_DELAY: Duration = Duration::from_secs(2);

/// Enable jitter by default to desynchronize clients and reduce contention.
///
/// Randomizing retry delays mitigates correlated bursts and improves tail
/// latency under contention. See [Exponential Backoff and Jitter](https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter) for details.
pub(super) const DEFAULT_USE_JITTER: bool = true;

/// Default maximum retry attempts: 3.
///
/// The default is inherited from Polly v8 which is a widely used resilience library
/// and also uses 3 retry attempts.
pub(super) const DEFAULT_RETRY_ATTEMPTS: u32 = 3;
