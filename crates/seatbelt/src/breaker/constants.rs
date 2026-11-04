// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

/// The minimum duration of the sampling window.
pub(crate) const MIN_SAMPLING_DURATION: Duration = Duration::from_secs(1);

/// The default number of requests required before the breaker evaluates failures.
pub(crate) const DEFAULT_MIN_THROUGHPUT: u32 = 100;

/// The default duration of the sampling window.
pub(crate) const DEFAULT_SAMPLING_DURATION: Duration = Duration::from_secs(30);

/// The default failure rate that opens the circuit.
pub(crate) const DEFAULT_FAILURE_THRESHOLD: f32 = 0.1;

/// The default time before an open circuit starts to test the service.
pub(crate) const DEFAULT_BREAK_DURATION: Duration = Duration::from_secs(5);

pub(crate) const ERR_POISONED_LOCK: &str =
    "poisoned lock - cannot continue execution because security and privacy guarantees can no longer be upheld";
