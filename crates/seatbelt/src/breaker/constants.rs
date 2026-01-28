// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

/// Minimum allowed duration for the circuit breaker's sampling window.
pub(crate) const MIN_SAMPLING_DURATION: Duration = Duration::from_secs(1);

/// Default minimum throughput (number of requests) in the sampling window before
/// the circuit breaker can evaluate the failure rate and potentially trip the circuit.
pub(crate) const DEFAULT_MIN_THROUGHPUT: u32 = 100;

/// Default duration of the circuit breaker's sampling window.
pub(crate) const DEFAULT_SAMPLING_DURATION: Duration = Duration::from_secs(30);

/// Default failure threshold (percentage of failed requests) in the sampling window
/// that will trip the circuit breaker.
pub(crate) const DEFAULT_FAILURE_THRESHOLD: f32 = 0.1;

/// Default duration that the circuit breaker remains open (broken) before
/// transitioning to half-open to test if the service has recovered.
pub(crate) const DEFAULT_BREAK_DURATION: Duration = Duration::from_secs(5);

pub(crate) const ERR_POISONED_LOCK: &str =
    "poisoned lock - cannot continue execution because security and privacy guarantees can no longer be upheld";
