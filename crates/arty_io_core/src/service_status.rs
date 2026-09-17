// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Instant;

/// The scheduling needs remaining after a bounded service turn.
///
/// An idle result does not establish that notifications are armed. The runtime still calls
/// the participant's `prepare_wait` operation before sleeping. A deadline is expressed in the
/// same monotonic `Instant` time domain used by the runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceStatus {
    runnable: bool,
    deadline: Option<Instant>,
}

impl ServiceStatus {
    /// Reports no immediate work or required timed service.
    #[must_use]
    pub const fn idle() -> Self {
        Self {
            runnable: false,
            deadline: None,
        }
    }

    /// Requests another service turn without waiting for another notification.
    #[must_use]
    pub const fn runnable() -> Self {
        Self {
            runnable: true,
            deadline: None,
        }
    }

    /// Requests service no later than `deadline`, unless a notification arrives first.
    ///
    /// A deadline reached by the runtime requires immediate service.
    #[must_use]
    pub const fn at(deadline: Instant) -> Self {
        Self {
            runnable: false,
            deadline: Some(deadline),
        }
    }

    /// Returns whether another turn is required without an intervening wait.
    ///
    /// The runtime also treats an expired [`deadline`](Self::deadline) as runnable.
    #[must_use]
    pub const fn is_runnable(&self) -> bool {
        self.runnable
    }

    /// Returns the latest time at which service must resume without a notification.
    #[must_use]
    pub const fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

/// The result of arming a participant immediately before a possible wait.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitStatus {
    /// Work was found, or preparation needs budgeted service before sleeping is safe.
    WorkReady,
    /// Notifications are armed and the participant rechecked that no immediate work remains.
    Armed,
}
