// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{Display, Formatter, Result};

/// Error returned when all owners of a clock have been dropped.
///
/// All [`Clock`](crate::Clock) values for the
/// [`ClockDriver`](crate::runtime::ClockDriver) were dropped. The driver has no
/// more timers to advance.
///
/// A runtime can stop its timer loop after this error.
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
