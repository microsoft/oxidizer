// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use tick::Clock;

use crate::circuit::PartitionKey;

/// Arguments for the [`recovery_with`][super::CircuitLayer::recovery_with] callback function.
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

/// Arguments for the [`rejected_input`][super::CircuitLayer::rejected_input] callback function.
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

/// Arguments for the [`on_probing`][super::CircuitLayer::on_probing] callback function.
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

/// Arguments for the [`on_closed`][super::CircuitLayer::on_closed] callback function.
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

/// Arguments for the [`on_opened`][super::CircuitLayer::on_opened] callback function.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_args_accessors() {
        let key = PartitionKey::from("test");
        let clock = Clock::new_frozen();
        let args = RecoveryArgs {
            partition_key: &key,
            clock: &clock,
        };
        assert_eq!(args.partition_key(), &key);
        let _ = args.clock();
        assert!(format!("{args:?}").contains("RecoveryArgs"));
    }

    #[test]
    fn rejected_input_args_accessors() {
        let key = PartitionKey::from("rejected");
        let args = RejectedInputArgs { partition_key: &key };
        assert_eq!(args.partition_key(), &key);
        assert!(format!("{args:?}").contains("RejectedInputArgs"));
    }

    #[test]
    fn on_probing_args_accessors() {
        let key = PartitionKey::from("probing");
        let args = OnProbingArgs { partition_key: &key };
        assert_eq!(args.partition_key(), &key);
        assert!(format!("{args:?}").contains("OnProbingArgs"));
    }

    #[test]
    fn on_closed_args_accessors() {
        let key = PartitionKey::from("closed");
        let duration = Duration::from_secs(5);
        let args = OnClosedArgs {
            partition_key: &key,
            open_duration: duration,
        };
        assert_eq!(args.partition_key(), &key);
        assert_eq!(args.open_duration(), duration);
        assert!(format!("{args:?}").contains("OnClosedArgs"));
    }

    #[test]
    fn on_opened_args_accessors() {
        let key = PartitionKey::from("opened");
        let args = OnOpenedArgs { partition_key: &key };
        assert_eq!(args.partition_key(), &key);
        assert!(format!("{args:?}").contains("OnOpenedArgs"));
    }
}
