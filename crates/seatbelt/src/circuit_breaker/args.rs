// Copyright (c) Microsoft Corporation.

use std::time::Duration;

use tick::Clock;

use crate::circuit_breaker::PartitionKey;

/// Arguments for the [`recovery_with`][super::CircuitBreakerLayer::recovery_with] callback function.
///
/// Provides context for recovery classification in the circuit breaker.
#[derive(Debug)]
#[non_exhaustive]
pub struct RecoveryArgs<'a> {
    pub(crate) partition_key: &'a PartitionKey,
    pub(crate) clock: &'a Clock,
}

impl RecoveryArgs<'_> {
    /// Returns the partition key associated with the recovery evaluation.
    #[must_use]
    pub fn partition_key(&self) -> &PartitionKey {
        self.partition_key
    }

    /// Returns a reference to the clock use by the circuit breaker.
    #[must_use]
    pub fn clock(&self) -> &Clock {
        self.clock
    }
}

/// Arguments for the [`rejected_input`][super::CircuitBreakerLayer::rejected_input] callback function.
///
/// Provides context for generating outputs when the inputs are rejected by the circuit breaker.
#[derive(Debug)]
pub struct RejectedInputArgs<'a> {
    pub(crate) partition_key: &'a PartitionKey,
}

impl RejectedInputArgs<'_> {
    /// Returns the partition key associated with the rejected input.
    #[must_use]
    pub fn partition_key(&self) -> &PartitionKey {
        self.partition_key
    }
}

/// Arguments for the [`on_probing`][super::CircuitBreakerLayer::on_probing] callback function.
///
/// Provides context when the circuit breaker enters the probing state to test if the service has recovered.
#[derive(Debug)]
#[non_exhaustive]
pub struct OnProbingArgs<'a> {
    pub(crate) partition_key: &'a PartitionKey,
}

impl OnProbingArgs<'_> {
    /// Returns the partition key associated with the probing execution.
    #[must_use]
    pub fn partition_key(&self) -> &PartitionKey {
        self.partition_key
    }
}

/// Arguments for the [`on_closed`][super::CircuitBreakerLayer::on_closed] callback function.
///
/// Provides context when the circuit breaker transitions to the closed state, allowing normal operation.
#[derive(Debug)]
#[non_exhaustive]
pub struct OnClosedArgs<'a> {
    pub(crate) partition_key: &'a PartitionKey,
    pub(crate) open_duration: std::time::Duration,
}

impl OnClosedArgs<'_> {
    /// Returns the partition key associated with this event.
    #[must_use]
    pub fn partition_key(&self) -> &PartitionKey {
        self.partition_key
    }

    /// Returns the duration the circuit was open before closing.
    #[must_use]
    pub fn open_duration(&self) -> Duration {
        self.open_duration
    }
}

/// Arguments for the [`on_opened`][super::CircuitBreakerLayer::on_opened] callback function.
///
/// Provides context when the circuit breaker transitions to the open state, blocking requests due to failures.
#[derive(Debug)]
#[non_exhaustive]
pub struct OnOpenedArgs<'a> {
    pub(crate) partition_key: &'a PartitionKey,
}

impl OnOpenedArgs<'_> {
    /// Returns the partition key associated with this event.
    #[must_use]
    pub fn partition_key(&self) -> &PartitionKey {
        self.partition_key
    }
}
