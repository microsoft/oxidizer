// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use tick::Clock;

use crate::RecoveryInfo;
use crate::attempt::Attempt;

/// Provides data to the [`clone_input_with`][super::RetryLayer::clone_input_with] callback.
///
/// Use this data when you clone input.
#[derive(Debug)]
pub struct CloneArgs {
    pub(super) attempt: Attempt,
    pub(super) previous_recovery: Option<RecoveryInfo>,
}

impl CloneArgs {
    /// Returns the current attempt information.
    #[must_use]
    pub fn attempt(&self) -> Attempt {
        self.attempt
    }

    /// Returns recovery data from the previous attempt, if it exists.
    #[must_use]
    pub fn previous_recovery(&self) -> Option<&RecoveryInfo> {
        self.previous_recovery.as_ref()
    }
}

/// Provides data to the [`recovery_with`][super::RetryLayer::recovery_with] callback.
///
/// Use this data to classify output for recovery.
#[derive(Debug)]
pub struct RecoveryArgs<'a> {
    pub(super) attempt: Attempt,
    pub(super) clock: &'a Clock,
}

impl RecoveryArgs<'_> {
    /// Returns the current attempt information.
    #[must_use]
    pub fn attempt(&self) -> Attempt {
        self.attempt
    }

    /// Returns the clock used for time-related operations.
    #[must_use]
    pub fn clock(&self) -> &Clock {
        self.clock
    }
}

/// Provides data to the [`on_retry`][super::RetryLayer::on_retry] callback.
///
/// Use this data when a retry starts.
#[derive(Debug)]
pub struct OnRetryArgs {
    pub(super) attempt: Attempt,
    pub(super) retry_delay: Duration,
    pub(super) recovery: RecoveryInfo,
}

impl OnRetryArgs {
    /// Returns the current attempt information.
    #[must_use]
    pub fn attempt(&self) -> Attempt {
        self.attempt
    }

    /// Returns the delay before the next retry attempt.
    #[must_use]
    pub fn retry_delay(&self) -> Duration {
        self.retry_delay
    }

    /// Returns the recovery information that triggered this retry.
    #[must_use]
    pub fn recovery(&self) -> &RecoveryInfo {
        &self.recovery
    }
}

/// Provides data to the [`restore_input`][super::RetryLayer::restore_input] callback.
///
/// Use this data to restore input when cloning is not available.
#[derive(Debug)]
pub struct RestoreInputArgs {
    pub(super) attempt: Attempt,
    pub(super) recovery: RecoveryInfo,
}

impl RestoreInputArgs {
    /// Returns the current attempt information.
    #[must_use]
    pub fn attempt(&self) -> Attempt {
        self.attempt
    }

    /// Returns the recovery information that triggered this restoration attempt.
    #[must_use]
    pub fn recovery(&self) -> &RecoveryInfo {
        &self.recovery
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_recover_args() {
        let clock = Clock::new_frozen();

        let args = RecoveryArgs {
            attempt: Attempt::new(3, true),
            clock: &clock,
        };

        assert_eq!(args.attempt(), Attempt::new(3, true));
        let _clock = args.clock();
    }

    #[test]
    fn on_retry_args() {
        let args = OnRetryArgs {
            attempt: Attempt::new(2, false),
            retry_delay: Duration::from_secs(5),
            recovery: RecoveryInfo::retry(),
        };

        assert_eq!(args.attempt(), Attempt::new(2, false));
        assert_eq!(args.retry_delay(), Duration::from_secs(5));
        assert_eq!(*args.recovery(), RecoveryInfo::retry());
    }

    #[test]
    fn clone_args() {
        let args = CloneArgs {
            attempt: Attempt::new(1, false),
            previous_recovery: Some(RecoveryInfo::retry()),
        };

        assert_eq!(args.attempt(), Attempt::new(1, false));
        assert_eq!(args.previous_recovery(), Some(&RecoveryInfo::retry()));
    }

    #[test]
    fn restore_input_args() {
        let args = RestoreInputArgs {
            attempt: Attempt::new(2, true),
            recovery: RecoveryInfo::retry(),
        };

        assert_eq!(args.attempt(), Attempt::new(2, true));
        assert_eq!(*args.recovery(), RecoveryInfo::retry());
    }
}
