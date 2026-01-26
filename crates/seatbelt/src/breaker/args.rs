// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use tick::Clock;

use crate::breaker::BreakerId;

/// Arguments for the [`recovery_with`][super::BreakerLayer::recovery_with] callback function.
///
/// Provides context for recovery classification in the circuit breaker.
#[derive(Debug)]
#[non_exhaustive]
pub struct RecoveryArgs<'a> {
    pub(crate) breaker_id: &'a BreakerId,
    pub(crate) clock: &'a Clock,
}

impl RecoveryArgs<'_> {
    /// Returns the breaker ID associated with the recovery evaluation.
    #[must_use]
    pub fn breaker_id(&self) -> &BreakerId {
        self.breaker_id
    }

    /// Returns a reference to the clock use by the circuit breaker.
    #[must_use]
    pub fn clock(&self) -> &Clock {
        self.clock
    }
}

/// Arguments for the [`rejected_input`][super::BreakerLayer::rejected_input] callback function.
///
/// Provides context for generating outputs when the inputs are rejected by the circuit breaker.
#[derive(Debug)]
pub struct RejectedInputArgs<'a> {
    pub(crate) breaker_id: &'a BreakerId,
}

impl RejectedInputArgs<'_> {
    /// Returns the breaker ID associated with the rejected input.
    #[must_use]
    pub fn breaker_id(&self) -> &BreakerId {
        self.breaker_id
    }
}

/// Arguments for the [`on_probing`][super::BreakerLayer::on_probing] callback function.
///
/// Provides context when the circuit breaker enters the probing state to test if the service has recovered.
#[derive(Debug)]
#[non_exhaustive]
pub struct OnProbingArgs<'a> {
    pub(crate) breaker_id: &'a BreakerId,
}

impl OnProbingArgs<'_> {
    /// Returns the breaker ID associated with the probing execution.
    #[must_use]
    pub fn breaker_id(&self) -> &BreakerId {
        self.breaker_id
    }
}

/// Arguments for the [`on_closed`][super::BreakerLayer::on_closed] callback function.
///
/// Provides context when the circuit breaker transitions to the closed state, allowing normal operation.
#[derive(Debug)]
#[non_exhaustive]
pub struct OnClosedArgs<'a> {
    pub(crate) breaker_id: &'a BreakerId,
    pub(crate) open_duration: std::time::Duration,
}

impl OnClosedArgs<'_> {
    /// Returns the breaker ID associated with this event.
    #[must_use]
    pub fn breaker_id(&self) -> &BreakerId {
        self.breaker_id
    }

    /// Returns the duration the circuit was open before closing.
    #[must_use]
    pub fn open_duration(&self) -> Duration {
        self.open_duration
    }
}

/// Arguments for the [`on_opened`][super::BreakerLayer::on_opened] callback function.
///
/// Provides context when the circuit breaker transitions to the open state, blocking inputs due to failures.
#[derive(Debug)]
#[non_exhaustive]
pub struct OnOpenedArgs<'a> {
    pub(crate) breaker_id: &'a BreakerId,
}

impl OnOpenedArgs<'_> {
    /// Returns the breaker ID associated with this event.
    #[must_use]
    pub fn breaker_id(&self) -> &BreakerId {
        self.breaker_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_args_accessors() {
        let key = BreakerId::from("test");
        let clock = Clock::new_frozen();
        let args = RecoveryArgs {
            breaker_id: &key,
            clock: &clock,
        };
        assert_eq!(args.breaker_id(), &key);
        let _ = args.clock();
        assert!(format!("{args:?}").contains("RecoveryArgs"));
    }

    #[test]
    fn rejected_input_args_accessors() {
        let key = BreakerId::from("rejected");
        let args = RejectedInputArgs { breaker_id: &key };
        assert_eq!(args.breaker_id(), &key);
        assert!(format!("{args:?}").contains("RejectedInputArgs"));
    }

    #[test]
    fn on_probing_args_accessors() {
        let key = BreakerId::from("probing");
        let args = OnProbingArgs { breaker_id: &key };
        assert_eq!(args.breaker_id(), &key);
        assert!(format!("{args:?}").contains("OnProbingArgs"));
    }

    #[test]
    fn on_closed_args_accessors() {
        let key = BreakerId::from("closed");
        let duration = Duration::from_secs(5);
        let args = OnClosedArgs {
            breaker_id: &key,
            open_duration: duration,
        };
        assert_eq!(args.breaker_id(), &key);
        assert_eq!(args.open_duration(), duration);
        assert!(format!("{args:?}").contains("OnClosedArgs"));
    }

    #[test]
    fn on_opened_args_accessors() {
        let key = BreakerId::from("opened");
        let args = OnOpenedArgs { breaker_id: &key };
        assert_eq!(args.breaker_id(), &key);
        assert!(format!("{args:?}").contains("OnOpenedArgs"));
    }
}
