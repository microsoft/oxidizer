// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use tick::Clock;

use crate::Attempt;

/// Arguments for the [`clone_input_with`][super::HedgingLayer::clone_input_with] callback function.
///
/// Provides context for input cloning operations during hedging.
#[derive(Debug)]
pub struct CloneArgs {
    pub(super) attempt: Attempt,
}

impl CloneArgs {
    /// Returns the current attempt information.
    ///
    /// Index 0 is the original request, 1 is the first hedge, etc.
    #[must_use]
    pub fn attempt(&self) -> Attempt {
        self.attempt
    }
}

/// Arguments for the [`recovery_with`][super::HedgingLayer::recovery_with] callback function.
///
/// Provides context for recovery classification of hedging results.
#[derive(Debug)]
pub struct RecoveryArgs<'a> {
    pub(super) clock: &'a Clock,
}

impl RecoveryArgs<'_> {
    /// Returns the clock used for time-related operations.
    #[must_use]
    pub fn clock(&self) -> &Clock {
        self.clock
    }
}

/// Arguments for the [`on_hedge`][super::HedgingLayer::on_hedge] callback function.
///
/// Provides context when a new hedged request is about to be launched.
#[derive(Debug)]
pub struct OnHedgeArgs {
    pub(super) attempt: Attempt,
}

impl OnHedgeArgs {
    /// Returns the current attempt information.
    ///
    /// Attempt index 1 is the first hedge (the second overall request), etc.
    #[must_use]
    pub fn attempt(&self) -> Attempt {
        self.attempt
    }
}

/// Arguments for the [`HedgingMode::dynamic`][super::HedgingMode::dynamic] callback function.
///
/// Provides context for computing the delay before the next hedge.
#[derive(Debug)]
pub struct HedgingDelayArgs {
    pub(super) hedge_index: u32,
}

impl HedgingDelayArgs {
    /// Returns the 0-based index of the hedge about to be launched.
    ///
    /// Index 0 is the first hedge, 1 is the second hedge, etc.
    #[must_use]
    pub fn hedge_index(&self) -> u32 {
        self.hedge_index
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
        let args = RecoveryArgs { clock: &clock };
        let _clock = args.clock();
    }

    #[test]
    fn on_hedge_args() {
        let args = OnHedgeArgs {
            attempt: Attempt::new(1, false),
        };
        assert_eq!(args.attempt().index(), 1);
    }

    #[test]
    fn hedging_delay_args() {
        let args = HedgingDelayArgs { hedge_index: 0 };
        assert_eq!(args.hedge_index(), 0);
    }
}
