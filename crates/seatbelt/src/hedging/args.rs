// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use tick::Clock;

use crate::Attempt;

/// Provides data to the [`clone_input_with`][super::HedgingLayer::clone_input_with] callback.
///
/// Use this data when you clone input for a hedged attempt.
#[derive(Debug)]
pub struct CloneArgs {
    pub(super) attempt: Attempt,
}

impl CloneArgs {
    /// Returns the current attempt information.
    ///
    /// Index 0 is the original request. Index 1 is the first hedged attempt.
    #[must_use]
    pub fn attempt(&self) -> Attempt {
        self.attempt
    }
}

/// Provides data to the [`recovery_with`][super::HedgingLayer::recovery_with] callback.
///
/// Use this data to classify output from a hedged attempt.
#[derive(Debug)]
pub struct RecoveryArgs<'a> {
    pub(super) clock: &'a Clock,
    pub(super) attempt: Attempt,
}

impl RecoveryArgs<'_> {
    /// Returns the clock used for time-related operations.
    #[must_use]
    pub fn clock(&self) -> &Clock {
        self.clock
    }

    /// Returns the attempt that produced the result being classified.
    ///
    /// Index 0 is the original request. Index 1 is the first hedged attempt.
    #[must_use]
    pub fn attempt(&self) -> Attempt {
        self.attempt
    }
}

/// Provides data to the [`on_execute`][super::HedgingLayer::on_execute] callback.
///
/// Use this data before an original or hedged request starts.
#[derive(Debug)]
pub struct OnExecuteArgs {
    pub(super) attempt: Attempt,
    pub(super) delay: Duration,
}

impl OnExecuteArgs {
    /// Returns the current attempt information.
    ///
    /// Index 0 is the original request. Index 1 is the first hedged attempt.
    #[must_use]
    pub fn attempt(&self) -> Attempt {
        self.attempt
    }

    /// Returns the delay before this attempt started.
    ///
    /// For the original request (attempt 0) or when a zero delay is configured, this is
    /// [`Duration::ZERO`].
    #[must_use]
    pub fn delay(&self) -> Duration {
        self.delay
    }
}

/// Provides data to the [`hedging_delay_with`][super::HedgingLayer::hedging_delay_with] callback.
///
/// Use this data to calculate the delay before the next hedged attempt.
#[derive(Debug)]
pub struct HedgingDelayArgs {
    pub(super) attempt: Attempt,
}

impl HedgingDelayArgs {
    /// Returns the attempt information for the hedging attempt about to be launched.
    ///
    /// Index 1 is the first hedged attempt. Index 2 is the second.
    #[must_use]
    pub fn attempt(&self) -> Attempt {
        self.attempt
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_args() {
        let args = CloneArgs {
            attempt: Attempt::new(2, true),
        };
        assert_eq!(args.attempt().index(), 2);
        assert!(args.attempt().is_last());
    }

    #[test]
    fn recovery_args() {
        let clock = Clock::new_frozen();
        let args = RecoveryArgs {
            clock: &clock,
            attempt: Attempt::new(1, false),
        };
        let _clock = args.clock();
        assert_eq!(args.attempt().index(), 1);
        assert!(!args.attempt().is_last());
    }

    #[test]
    fn on_execute_args() {
        let args = OnExecuteArgs {
            attempt: Attempt::new(1, false),
            delay: Duration::from_millis(200),
        };
        assert_eq!(args.attempt().index(), 1);
        assert_eq!(args.delay(), Duration::from_millis(200));
    }

    #[test]
    fn hedging_delay_args() {
        let args = HedgingDelayArgs {
            attempt: Attempt::new(1, false),
        };
        assert_eq!(args.attempt().index(), 1);
        assert!(!args.attempt().is_last());
    }
}
