// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{Display, Formatter, Result};

/// Error returned when all owners of a clock have been dropped.
///
/// This error indicates that all [`Clock`](crate::Clock) instances associated with a
/// [`ClockDriver`](crate::runtime::ClockDriver) have been dropped, meaning there are
/// no more timers to advance and advancing timers is no longer necessary.
///
/// Runtime integrations should use this error to determine when to stop the timer
/// advancement loop.
#[derive(Debug)]
#[non_exhaustive]
pub struct ClockGone;

impl ClockGone {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Display for ClockGone {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "all clock owners have been dropped")
    }
}

impl std::error::Error for ClockGone {}
