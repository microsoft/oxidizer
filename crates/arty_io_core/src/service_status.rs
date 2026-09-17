// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Instant;

/// The scheduling needs remaining after a bounded service turn.
///
/// [`Idle`](Self::Idle) does not establish that notifications are armed. The runtime still asks
/// the participant to prepare for a wait before sleeping. A deadline uses the same monotonic
/// [`Instant`] time domain as the runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceStatus {
    /// No immediate work and no required timed service.
    ///
    /// Later activity must signal the participant's readiness waker.
    Idle,
    /// Another service turn is required without an intervening wait.
    ///
    /// Work remaining after an exhausted [`CompletionBudget`](crate::CompletionBudget) reports
    /// this, even when no further native notification will arrive.
    Runnable,
    /// Service is required no later than this instant, unless a notification arrives first.
    ///
    /// A deadline the runtime has already reached requires immediate service.
    Deadline(Instant),
}

/// The result of arming a participant immediately before a possible wait.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitStatus {
    /// Work was found, or preparation needs budgeted service before sleeping is safe.
    WorkReady,
    /// Notifications are armed and the participant rechecked that no immediate work remains.
    Armed,
}
