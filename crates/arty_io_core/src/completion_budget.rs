// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;

/// A finite allowance for one completion-service turn.
///
/// Charge a unit before processing a completion or performing another bounded progress step.
/// A driver that uses its allowance while work remains returns
/// [`ServiceStatus::runnable`](crate::ServiceStatus::runnable). The runtime gives each
/// participant a new allowance on its next turn.
///
/// A budget is deliberately neither `Clone` nor `Copy`: forwarding the same mutable budget
/// through helpers must not duplicate its remaining allowance.
///
/// ```
/// use std::num::NonZeroUsize;
///
/// use arty_io_core::CompletionBudget;
///
/// let mut budget = CompletionBudget::new(NonZeroUsize::new(2).unwrap());
/// assert!(budget.try_consume());
/// assert!(budget.try_consume());
/// assert!(!budget.try_consume());
/// assert_eq!(budget.remaining(), 0);
/// ```
#[derive(Debug)]
pub struct CompletionBudget {
    remaining: usize,
}

impl CompletionBudget {
    /// Creates an allowance of `units` bounded progress steps.
    #[must_use]
    pub const fn new(units: NonZeroUsize) -> Self {
        Self { remaining: units.get() }
    }

    /// Returns the number of steps still permitted in this turn.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.remaining
    }

    /// Consumes one unit, returning false when the allowance is exhausted.
    ///
    /// A false result leaves the budget unchanged. Do not perform the charged operation
    /// unless this method returns true.
    #[must_use]
    pub const fn try_consume(&mut self) -> bool {
        if self.remaining == 0 {
            return false;
        }
        self.remaining -= 1;
        true
    }
}
